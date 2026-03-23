use std::collections::{HashMap, HashSet};
use chrono::Utc;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, FromQueryResult, JoinType, PaginatorTrait, QueryFilter, QuerySelect, RelationTrait, Set};
use sea_orm::sea_query::{Alias, BinOper, Expr};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    auth::{error::AuthError, permissions::{permission_key, split_permission_key}},
    models::{
        departments,
        mcp_server_access_rules::{self, McpRuleType, McpSubjectType},
        mcp_servers,
        permissions,
        role_permissions,
        roles,
        user_role_assignments,
        users,
    },
};

use super::auth_audit::record_auth_event;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionScopeMode {
    RequireOrgWide,
    AllowAnyScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
pub enum McpAccessDecision {
    Allow,
    Deny,
}

#[derive(Debug, Clone)]
pub struct AuthorizationService<'a> {
    db: &'a DatabaseConnection,
}

#[derive(Serialize)]
struct PermissionDeniedPayload {
    reason: String,
    permission: String,
    resource_id: Option<Uuid>,
    target_department_id: Option<Uuid>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum PermissionScopeValue {
    OrgWide(String),
    Scoped(Vec<String>),
}

#[derive(Serialize)]
struct EffectivePermissionsPayload {
    permissions: HashMap<String, PermissionScopeValue>,
    mcp_access: HashMap<String, McpAccessDecision>,
    administered_departments: Vec<String>,
}

fn audit_payload<T: Serialize>(value: T) -> Option<serde_json::Value> {
    serde_json::to_value(value)
        .map_err(|e| {
            eprintln!("audit payload error: {e}");
        })
        .ok()
}

impl<'a> AuthorizationService<'a> {
    pub fn new(db: &'a DatabaseConnection) -> Self {
        Self { db }
    }

    pub async fn ensure_permission(
        &self,
        actor_id: Uuid,
        permission: &str,
        target_department_id: Option<Uuid>,
        scope_mode: PermissionScopeMode,
        resource_id: Option<Uuid>,
    ) -> Result<(), AuthError> {
        let allowed = self
            .user_has_permission(
                actor_id,
                permission,
                target_department_id,
                scope_mode,
            )
            .await?;
        if allowed {
            return Ok(());
        }

        if let Some(payload) = audit_payload(PermissionDeniedPayload {
            reason: "missing_permission".to_string(),
            permission: permission.to_string(),
            resource_id,
            target_department_id,
        }) {
            let _ = record_auth_event(
                self.db,
                "auth.permission_denied",
                Some(actor_id),
                payload,
            )
            .await;
        }

        Err(AuthError::PermissionDenied)
    }

    pub async fn user_has_permission(
        &self,
        user_id: Uuid,
        permission: &str,
        target_department_id: Option<Uuid>,
        scope_mode: PermissionScopeMode,
    ) -> Result<bool, AuthError> {
        let has_assignments = self.user_has_assignments(user_id).await?;
        if !has_assignments {
            return Ok(false);
        }

        let (domain, action) = match split_permission_key(permission) {
            Some(parts) => parts,
            None => return Ok(false),
        };

        let permission_model = permissions::Entity::find()
            .filter(permissions::Column::Domain.eq(domain))
            .filter(permissions::Column::Action.eq(action))
            .one(self.db)
            .await
            .map_err(|e| {
                eprintln!("permission lookup error: {e}");
                AuthError::DbTimeout
            })?;

        let permission_model = match permission_model {
            Some(model) => model,
            None => return Ok(false),
        };

        let assignment_rows = self
            .assignment_scopes_for_permission(user_id, permission_model.id)
            .await?;

        if assignment_rows.is_empty() {
            return Ok(false);
        }
        let target_path = if let Some(target_id) = target_department_id {
            match self.department_path(target_id).await? {
                Some(path) => Some(path),
                None => return Ok(false),
            }
        } else {
            None
        };

        Ok(evaluate_permission_assignments(
            &assignment_rows,
            permission_model.is_scopeable,
            target_path.as_deref(),
            scope_mode,
        ))
    }

    pub async fn user_has_role_name(
        &self,
        user_id: Uuid,
        role_name: &str,
    ) -> Result<bool, AuthError> {
        let count = user_role_assignments::Entity::find()
            .join(JoinType::InnerJoin, user_role_assignments::Relation::Roles.def())
            .filter(user_role_assignments::Column::UserId.eq(user_id))
            .filter(roles::Column::Name.eq(role_name))
            .count(self.db)
            .await
            .map_err(|e| {
                eprintln!("role lookup error: {e}");
                AuthError::DbTimeout
            })?;
        Ok(count > 0)
    }

    pub async fn user_roles_map(
        &self,
        user_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<String>>, AuthError> {
        if user_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = user_role_assignments::Entity::find()
            .select_only()
            .column(user_role_assignments::Column::UserId)
            .column_as(roles::Column::Name, "role_name")
            .join(JoinType::InnerJoin, user_role_assignments::Relation::Roles.def())
            .filter(user_role_assignments::Column::UserId.is_in(user_ids.iter().copied()))
            .into_tuple::<(Uuid, String)>()
            .all(self.db)
            .await
            .map_err(|e| {
                eprintln!("role assignments lookup error: {e}");
                AuthError::DbTimeout
            })?;

        let mut map: HashMap<Uuid, Vec<String>> = HashMap::new();
        for (user_id, role_name) in rows {
            map.entry(user_id).or_default().push(role_name);
        }
        Ok(map)
    }

    pub async fn recompute_effective_permissions(&self, user_id: Uuid) -> Result<(), AuthError> {
        let user = users::Entity::find_by_id(user_id)
            .one(self.db)
            .await
            .map_err(|e| {
                eprintln!("user lookup error: {e}");
                AuthError::DbTimeout
            })?
            .ok_or(AuthError::DbNotFound)?;

        let permission_rows = self.user_permission_rows(user_id).await?;
        let role_ids = self.user_role_ids(user_id).await?;

        let permissions_map = build_permissions_json(&permission_rows);

        let administered_departments = self
            .compute_administered_departments(&permission_rows)
            .await?;

        let mcp_access = self
            .compute_mcp_access(user.id, user.department_id, &role_ids)
            .await?;

        let effective_permissions = audit_payload(EffectivePermissionsPayload {
            permissions: permissions_map,
            mcp_access,
            administered_departments,
        })
        .ok_or(AuthError::ServiceTemporarilyUnavailable)?;

        let mut active: users::ActiveModel = user.into();
        active.effective_permissions = Set(Some(effective_permissions));
        active.updated_at = Set(Utc::now());
        active
            .update(self.db)
            .await
            .map_err(|e| {
                eprintln!("update effective_permissions error: {e}");
                AuthError::DbTimeout
            })?;

        Ok(())
    }

    pub async fn recompute_effective_permissions_for_role(
        &self,
        role_id: Uuid,
    ) -> Result<(), AuthError> {
        let assignments = user_role_assignments::Entity::find()
            .filter(user_role_assignments::Column::RoleId.eq(role_id))
            .select_only()
            .column(user_role_assignments::Column::UserId)
            .into_tuple::<Uuid>()
            .all(self.db)
            .await
            .map_err(|e| {
                eprintln!("assignment lookup error: {e}");
                AuthError::DbTimeout
            })?;

        for user_id in assignments {
            let _ = self.recompute_effective_permissions(user_id).await;
        }
        Ok(())
    }

    pub async fn recompute_effective_permissions_for_users(
        &self,
        user_ids: &[Uuid],
    ) -> Result<(), AuthError> {
        for user_id in user_ids {
            let _ = self.recompute_effective_permissions(*user_id).await;
        }
        Ok(())
    }

    pub async fn recompute_effective_permissions_for_all_users(&self) -> Result<(), AuthError> {
        let user_ids = users::Entity::find()
            .select_only()
            .column(users::Column::Id)
            .into_tuple::<Uuid>()
            .all(self.db)
            .await
            .map_err(|e| {
                eprintln!("users lookup error: {e}");
                AuthError::DbTimeout
            })?;

        for user_id in user_ids {
            let _ = self.recompute_effective_permissions(user_id).await;
        }
        Ok(())
    }

    pub async fn recompute_effective_permissions_for_department_scope(
        &self,
        department_id: Uuid,
    ) -> Result<(), AuthError> {
        let department_path = match self.department_path(department_id).await? {
            Some(path) => path,
            None => return Ok(()),
        };

        let scoped_users = user_role_assignments::Entity::find()
            .select_only()
            .column(user_role_assignments::Column::UserId)
            .join(JoinType::InnerJoin, user_role_assignments::Relation::ScopeDepartments.def())
            .filter(
                Expr::col(departments::Column::Path).binary(
                    BinOper::Custom("<@".into()),
                    Expr::val(department_path).cast_as(Alias::new("ltree")),
                ),
            )
            .into_tuple::<Uuid>()
            .all(self.db)
            .await
            .map_err(|e| {
                eprintln!("scoped users lookup error: {e}");
                AuthError::DbTimeout
            })?;

        for user_id in scoped_users {
            let _ = self.recompute_effective_permissions(user_id).await;
        }
        Ok(())
    }

    pub async fn resolve_mcp_access(
        &self,
        user_id: Uuid,
        user_department_id: Option<Uuid>,
        role_ids: &HashSet<Uuid>,
        server_id: Uuid,
    ) -> Result<McpAccessDecision, AuthError> {
        if let Some(rule) = mcp_server_access_rules::Entity::find()
            .filter(mcp_server_access_rules::Column::ServerId.eq(server_id))
            .filter(mcp_server_access_rules::Column::SubjectType.eq(McpSubjectType::User))
            .filter(mcp_server_access_rules::Column::SubjectId.eq(user_id))
            .one(self.db)
            .await
            .map_err(|e| {
                eprintln!("mcp user rule error: {e}");
                AuthError::DbTimeout
            })?
        {
            return Ok(match rule.rule_type {
                McpRuleType::Allow => McpAccessDecision::Allow,
                McpRuleType::Deny => McpAccessDecision::Deny,
            });
        }

        if let Some(dept_id) = user_department_id {
            if let Some(decision) =
                self.resolve_mcp_department_rule(server_id, dept_id).await?
            {
                return Ok(decision);
            }
        }

        if !role_ids.is_empty() {
            let role_rules = mcp_server_access_rules::Entity::find()
                .filter(mcp_server_access_rules::Column::ServerId.eq(server_id))
                .filter(mcp_server_access_rules::Column::SubjectType.eq(McpSubjectType::Role))
                .filter(mcp_server_access_rules::Column::SubjectId.is_in(role_ids.iter().copied()))
                .all(self.db)
                .await
                .map_err(|e| {
                    eprintln!("mcp role rules error: {e}");
                    AuthError::DbTimeout
                })?;

            let mut saw_allow = false;
            let mut saw_deny = false;
            for rule in role_rules {
                match rule.rule_type {
                    McpRuleType::Allow => saw_allow = true,
                    McpRuleType::Deny => saw_deny = true,
                }
            }
            if saw_deny {
                return Ok(McpAccessDecision::Deny);
            }
            if saw_allow {
                return Ok(McpAccessDecision::Allow);
            }
        }

        let server = mcp_servers::Entity::find_by_id(server_id)
            .one(self.db)
            .await
            .map_err(|e| {
                eprintln!("mcp server lookup error: {e}");
                AuthError::DbTimeout
            })?
            .ok_or(AuthError::DbNotFound)?;

        Ok(match server.access_default {
            mcp_servers::McpAccessDefault::Allow => McpAccessDecision::Allow,
            mcp_servers::McpAccessDefault::Deny => McpAccessDecision::Deny,
        })
    }

    async fn resolve_mcp_department_rule(
        &self,
        server_id: Uuid,
        user_department_id: Uuid,
    ) -> Result<Option<McpAccessDecision>, AuthError> {
        #[derive(Debug, FromQueryResult)]
        struct DeptPathRow {
            id: Uuid,
            #[sea_orm(from_alias = "path")]
            path: String,
            #[sea_orm(from_alias = "depth")]
            depth: i32,
        }

        let user_department = departments::Entity::find_by_id(user_department_id)
            .select_only()
            .column(departments::Column::Id)
            .column_as(Expr::cust("path::text"), "path")
            .column_as(departments::Column::Depth, "depth")
            .into_model::<DeptPathRow>()
            .one(self.db)
            .await
            .map_err(|e| {
                eprintln!("department lookup error: {e}");
                AuthError::DbTimeout
            })?;
        let user_department = match user_department {
            Some(dept) => dept,
            None => return Ok(None),
        };

        let dept_rules = mcp_server_access_rules::Entity::find()
            .filter(mcp_server_access_rules::Column::ServerId.eq(server_id))
            .filter(mcp_server_access_rules::Column::SubjectType.eq(McpSubjectType::Department))
            .all(self.db)
            .await
            .map_err(|e| {
                eprintln!("department rules lookup error: {e}");
                AuthError::DbTimeout
            })?;

        if dept_rules.is_empty() {
            return Ok(None);
        }

        let dept_ids: Vec<Uuid> = dept_rules.iter().map(|rule| rule.subject_id).collect();
        let dept_paths = departments::Entity::find()
            .select_only()
            .column(departments::Column::Id)
            .column_as(Expr::cust("path::text"), "path")
            .column_as(departments::Column::Depth, "depth")
            .filter(departments::Column::Id.is_in(dept_ids))
            .into_model::<DeptPathRow>()
            .all(self.db)
            .await
            .map_err(|e| {
                eprintln!("department paths lookup error: {e}");
                AuthError::DbTimeout
            })?;

        let mut dept_lookup: HashMap<Uuid, (String, i32)> = HashMap::new();
        for row in dept_paths {
            dept_lookup.insert(row.id, (row.path, row.depth));
        }

        let mut best_rule: Option<(McpRuleType, i32)> = None;
        for rule in dept_rules {
            let Some((path, depth)) = dept_lookup.get(&rule.subject_id) else { continue };
            if is_path_within_scope(path, &user_department.path) {
                match best_rule {
                    Some((_, current_depth)) if current_depth >= *depth => {}
                    _ => best_rule = Some((rule.rule_type, *depth)),
                }
            }
        }

        if let Some((rule_type, _)) = best_rule {
            let decision = match rule_type {
                McpRuleType::Allow => McpAccessDecision::Allow,
                McpRuleType::Deny => McpAccessDecision::Deny,
            };
            return Ok(Some(decision));
        }

        Ok(None)
    }

    async fn compute_mcp_access(
        &self,
        user_id: Uuid,
        user_department_id: Option<Uuid>,
        role_ids: &HashSet<Uuid>,
    ) -> Result<HashMap<String, McpAccessDecision>, AuthError> {
        let servers = mcp_servers::Entity::find()
            .all(self.db)
            .await
            .map_err(|e| {
                eprintln!("mcp servers lookup error: {e}");
                AuthError::DbTimeout
            })?;

        let mut mcp_access = HashMap::new();
        for server in servers {
            let decision = self
                .resolve_mcp_access(user_id, user_department_id, role_ids, server.id)
                .await?;
            mcp_access.insert(server.id.to_string(), decision);
        }

        Ok(mcp_access)
    }

    async fn compute_administered_departments(
        &self,
        permission_rows: &[UserPermissionRow],
    ) -> Result<Vec<String>, AuthError> {
        let mut scopes = HashSet::new();
        let mut org_wide = false;

        for row in permission_rows {
            if row.domain == "departments" && row.action == "manage" {
                if row.scope_department_id.is_none() {
                    org_wide = true;
                } else if let Some(scope_id) = row.scope_department_id {
                    scopes.insert(scope_id);
                }
            }
        }

        if org_wide {
            let department_ids = departments::Entity::find()
                .select_only()
                .column(departments::Column::Id)
                .into_tuple::<Uuid>()
                .all(self.db)
                .await
                .map_err(|e| {
                    eprintln!("department list error: {e}");
                    AuthError::DbTimeout
                })?;
            return Ok(department_ids
                .into_iter()
                .map(|id| id.to_string())
                .collect());
        }

        Ok(scopes.into_iter().map(|id| id.to_string()).collect())
    }

    async fn user_has_assignments(&self, user_id: Uuid) -> Result<bool, AuthError> {
        let count = user_role_assignments::Entity::find()
            .filter(user_role_assignments::Column::UserId.eq(user_id))
            .count(self.db)
            .await
            .map_err(|e| {
                eprintln!("assignment count error: {e}");
                AuthError::DbTimeout
            })?;
        Ok(count > 0)
    }

    async fn user_role_ids(&self, user_id: Uuid) -> Result<HashSet<Uuid>, AuthError> {
        let role_ids = user_role_assignments::Entity::find()
            .select_only()
            .column(user_role_assignments::Column::RoleId)
            .filter(user_role_assignments::Column::UserId.eq(user_id))
            .into_tuple::<Uuid>()
            .all(self.db)
            .await
            .map_err(|e| {
                eprintln!("role id lookup error: {e}");
                AuthError::DbTimeout
            })?;

        Ok(role_ids.into_iter().collect())
    }

    async fn assignment_scopes_for_permission(
        &self,
        user_id: Uuid,
        permission_id: Uuid,
    ) -> Result<Vec<AssignmentScopeRow>, AuthError> {
        let rows = user_role_assignments::Entity::find()
            .select_only()
            .column(user_role_assignments::Column::ScopeDepartmentId)
            .column_as(Expr::cust("departments.path::text"), "scope_path")
            .join(JoinType::LeftJoin, user_role_assignments::Relation::ScopeDepartments.def())
            .join(JoinType::InnerJoin, user_role_assignments::Relation::Roles.def())
            .join(JoinType::InnerJoin, roles::Relation::RolePermissions.def())
            .filter(user_role_assignments::Column::UserId.eq(user_id))
            .filter(role_permissions::Column::PermissionId.eq(permission_id))
            .into_model::<AssignmentScopeRow>()
            .all(self.db)
            .await
            .map_err(|e| {
                eprintln!("assignment scopes error: {e}");
                AuthError::DbTimeout
            })?;
        Ok(rows)
    }

    async fn user_permission_rows(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<UserPermissionRow>, AuthError> {
        let rows = user_role_assignments::Entity::find()
            .select_only()
            .column(user_role_assignments::Column::RoleId)
            .column(user_role_assignments::Column::ScopeDepartmentId)
            .column_as(permissions::Column::Domain, "domain")
            .column_as(permissions::Column::Action, "action")
            .column_as(permissions::Column::IsScopeable, "is_scopeable")
            .join(JoinType::InnerJoin, user_role_assignments::Relation::Roles.def())
            .join(JoinType::InnerJoin, roles::Relation::RolePermissions.def())
            .join(JoinType::InnerJoin, role_permissions::Relation::Permissions.def())
            .filter(user_role_assignments::Column::UserId.eq(user_id))
            .into_model::<UserPermissionRow>()
            .all(self.db)
            .await
            .map_err(|e| {
                eprintln!("user permission rows error: {e}");
                AuthError::DbTimeout
            })?;

        Ok(rows)
    }

    async fn department_path(&self, department_id: Uuid) -> Result<Option<String>, AuthError> {
        let path = departments::Entity::find_by_id(department_id)
            .select_only()
            .column_as(Expr::cust("path::text"), "path")
            .into_tuple::<String>()
            .one(self.db)
            .await
            .map_err(|e| {
                eprintln!("department path error: {e}");
                AuthError::DbTimeout
            })?;
        Ok(path)
    }
}

#[derive(Debug, Default)]
struct PermissionScopeAggregate {
    org_wide: bool,
    scopes: HashSet<Uuid>,
}

fn build_permissions_json(rows: &[UserPermissionRow]) -> HashMap<String, PermissionScopeValue> {
    let mut permission_map: HashMap<String, PermissionScopeAggregate> = HashMap::new();

    for row in rows {
        let key = permission_key(&row.domain, &row.action);
        let entry = permission_map.entry(key).or_default();

        if !row.is_scopeable {
            if row.scope_department_id.is_none() {
                entry.org_wide = true;
            }
            continue;
        }

        if row.scope_department_id.is_none() {
            entry.org_wide = true;
        } else if let Some(scope_id) = row.scope_department_id {
            entry.scopes.insert(scope_id);
        }
    }

    let mut permissions_json = HashMap::new();
    for (key, value) in permission_map {
        if value.org_wide {
            permissions_json.insert(key, PermissionScopeValue::OrgWide("*".to_string()));
        } else if !value.scopes.is_empty() {
            let scopes: Vec<String> = value.scopes.into_iter().map(|id| id.to_string()).collect();
            permissions_json.insert(key, PermissionScopeValue::Scoped(scopes));
        }
    }

    permissions_json
}

fn evaluate_permission_assignments(
    assignments: &[AssignmentScopeRow],
    is_scopeable: bool,
    target_path: Option<&str>,
    scope_mode: PermissionScopeMode,
) -> bool {
    if !is_scopeable {
        return assignments.iter().any(|row| row.scope_department_id.is_none());
    }

    if let Some(target_path) = target_path {
        for row in assignments {
            if row.scope_department_id.is_none() {
                return true;
            }
            if let Some(scope_path) = row.scope_path.as_deref() {
                if is_path_within_scope(scope_path, target_path) {
                    return true;
                }
            }
        }
        return false;
    }

    match scope_mode {
        PermissionScopeMode::RequireOrgWide => assignments
            .iter()
            .any(|row| row.scope_department_id.is_none()),
        PermissionScopeMode::AllowAnyScope => !assignments.is_empty(),
    }
}

#[derive(Debug, FromQueryResult)]
struct AssignmentScopeRow {
    #[sea_orm(from_alias = "scopeDepartmentId")]
    scope_department_id: Option<Uuid>,
    #[sea_orm(from_alias = "scope_path")]
    scope_path: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct UserPermissionRow {
    // #[sea_orm(from_alias = "roleId")]
    // role_id: Uuid,
    #[sea_orm(from_alias = "scopeDepartmentId")]
    scope_department_id: Option<Uuid>,
    #[sea_orm(from_alias = "domain")]
    domain: String,
    #[sea_orm(from_alias = "action")]
    action: String,
    #[sea_orm(from_alias = "is_scopeable")]
    is_scopeable: bool,
}

pub fn is_path_within_scope(scope_path: &str, target_path: &str) -> bool {
    target_path == scope_path || target_path.starts_with(&format!("{scope_path}."))
}

fn select_department_rule(
    user_department_path: &str,
    dept_rules: &[(String, i32, McpRuleType)],
) -> Option<McpRuleType> {
    let mut best: Option<(McpRuleType, i32)> = None;
    for (path, depth, rule_type) in dept_rules {
        if is_path_within_scope(path, user_department_path) {
            match best {
                Some((_, best_depth)) if best_depth >= *depth => {}
                _ => best = Some((*rule_type, *depth)),
            }
        }
    }
    best.map(|(rule_type, _)| rule_type)
}

fn resolve_mcp_access_from_components(
    user_rule: Option<McpRuleType>,
    dept_rule: Option<McpRuleType>,
    role_rules: &[McpRuleType],
    default: McpAccessDecision,
) -> McpAccessDecision {
    if let Some(rule) = user_rule {
        return match rule {
            McpRuleType::Allow => McpAccessDecision::Allow,
            McpRuleType::Deny => McpAccessDecision::Deny,
        };
    }

    if let Some(rule) = dept_rule {
        return match rule {
            McpRuleType::Allow => McpAccessDecision::Allow,
            McpRuleType::Deny => McpAccessDecision::Deny,
        };
    }

    let mut saw_allow = false;
    let mut saw_deny = false;
    for rule in role_rules {
        match rule {
            McpRuleType::Allow => saw_allow = true,
            McpRuleType::Deny => saw_deny = true,
        }
    }
    if saw_deny {
        return McpAccessDecision::Deny;
    }
    if saw_allow {
        return McpAccessDecision::Allow;
    }

    default
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     #[test]
//     fn test_is_path_within_scope() {
//         assert!(is_path_within_scope("a.b", "a.b"));
//         assert!(is_path_within_scope("a.b", "a.b.c"));
//         assert!(!is_path_within_scope("a.b", "a.bc"));
//     }

//     #[test]
//     fn test_legacy_role_allows() {
//         assert!(legacy_role_allows(users::UserRole::SuperAdmin, "roles:view"));
//         assert!(legacy_role_allows(users::UserRole::Admin, "roles:view"));
//         assert!(!legacy_role_allows(users::UserRole::Observer, "roles:view"));
//         assert!(!legacy_role_allows(users::UserRole::User, "roles:view"));
//     }

//     #[test]
//     fn test_evaluate_permission_assignments_scoped() {
//         let assignments = vec![
//             AssignmentScopeRow {
//                 scope_department_id: Some(Uuid::new_v4()),
//                 scope_path: Some("root.sales".to_string()),
//             },
//         ];
//         assert!(evaluate_permission_assignments(
//             &assignments,
//             true,
//             Some("root.sales.eu"),
//             PermissionScopeMode::RequireOrgWide
//         ));
//         assert!(!evaluate_permission_assignments(
//             &assignments,
//             true,
//             Some("root.hr"),
//             PermissionScopeMode::RequireOrgWide
//         ));
//     }

//     #[test]
//     fn test_permission_scope_mode_org_wide() {
//         let assignments = vec![AssignmentScopeRow {
//             scope_department_id: Some(Uuid::new_v4()),
//             scope_path: Some("root.eng".to_string()),
//         }];

//         assert!(!evaluate_permission_assignments(
//             &assignments,
//             true,
//             None,
//             PermissionScopeMode::RequireOrgWide
//         ));
//         assert!(evaluate_permission_assignments(
//             &assignments,
//             true,
//             None,
//             PermissionScopeMode::AllowAnyScope
//         ));
//     }

//     #[test]
//     fn test_build_permissions_json() {
//         let scope_id = Uuid::new_v4();
//         let rows = vec![
//             UserPermissionRow {
//                 role_id: Uuid::new_v4(),
//                 scope_department_id: None,
//                 domain: "analytics".to_string(),
//                 action: "view".to_string(),
//                 is_scopeable: true,
//             },
//             UserPermissionRow {
//                 role_id: Uuid::new_v4(),
//                 scope_department_id: Some(scope_id),
//                 domain: "budget".to_string(),
//                 action: "allocate".to_string(),
//                 is_scopeable: true,
//             },
//             UserPermissionRow {
//                 role_id: Uuid::new_v4(),
//                 scope_department_id: None,
//                 domain: "roles".to_string(),
//                 action: "manage".to_string(),
//                 is_scopeable: false,
//             },
//         ];

//         let json = build_permissions_json(&rows);
//         assert!(matches!(
//             json.get("analytics:view"),
//             Some(PermissionScopeValue::OrgWide(value)) if value == "*"
//         ));
//         assert!(matches!(
//             json.get("budget:allocate"),
//             Some(PermissionScopeValue::Scoped(scopes)) if !scopes.is_empty()
//         ));
//         assert!(matches!(
//             json.get("roles:manage"),
//             Some(PermissionScopeValue::OrgWide(value)) if value == "*"
//         ));
//     }

//     #[test]
//     fn test_mcp_rule_resolution() {
//         let dept_rules = vec![
//             ("root".to_string(), 0, McpRuleType::Allow),
//             ("root.sales".to_string(), 1, McpRuleType::Deny),
//         ];
//         let dept_rule = select_department_rule("root.sales.eu", &dept_rules);
//         assert_eq!(dept_rule, Some(McpRuleType::Deny));

//         let decision = resolve_mcp_access_from_components(
//             Some(McpRuleType::Allow),
//             dept_rule,
//             &[McpRuleType::Deny],
//             McpAccessDecision::Deny,
//         );
//         assert_eq!(decision, McpAccessDecision::Allow);

//         let decision = resolve_mcp_access_from_components(
//             None,
//             None,
//             &[McpRuleType::Allow, McpRuleType::Deny],
//             McpAccessDecision::Allow,
//         );
//         assert_eq!(decision, McpAccessDecision::Deny);
//     }
// }
