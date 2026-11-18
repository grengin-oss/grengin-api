// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::collections::{HashMap, HashSet};

use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set, TransactionTrait,
};
use uuid::Uuid;

use crate::{
    auth::{error::AuthError, permissions::PERMISSION_MCP_ADMIN},
    dto::mcp::{McpAccessRuleInput, McpResolvedVia},
    error::AppError,
    models::{
        departments, mcp_access_policies,
        mcp_access_policies::{McpAccessTarget, McpAccessType, McpPermission},
        mcp_servers, mcp_tools, roles, user_role_assignments, users,
    },
    services::authorization::{AuthorizationService, PermissionScopeMode, is_path_within_scope},
};

#[derive(Clone, Debug)]
pub struct McpAccessContext {
    pub user_id: Uuid,
    pub department_id: Option<Uuid>,
    pub department_path: Option<String>,
    pub department_depth: Option<i32>,
    pub role_ids: HashSet<Uuid>,
    pub role_names: HashSet<String>,
    pub is_admin: bool,
}

#[derive(Clone, Debug)]
pub struct ResolvedAccess {
    pub permission: McpPermission,
    pub resolved_via: McpResolvedVia,
    pub priority: i32,
}

#[derive(Debug, sea_orm::FromQueryResult)]
struct DepartmentPath {
    id: Uuid,
    path: String,
    depth: i32,
}

pub struct McpAccessResolver<'a> {
    context: &'a McpAccessContext,
    departments: HashMap<Uuid, DepartmentPath>,
}

impl<'a> McpAccessResolver<'a> {
    pub async fn load(
        db: &DatabaseConnection,
        context: &'a McpAccessContext,
        rules: &[mcp_access_policies::Model],
    ) -> Result<Self, AppError> {
        let ids: HashSet<Uuid> = rules
            .iter()
            .filter(|rule| rule.access_type == McpAccessType::Department)
            .filter_map(|rule| rule.department_id)
            .collect();
        let departments = if context.department_id.is_some()
            && context.department_path.is_some()
            && context.department_depth.is_some()
            && !ids.is_empty()
        {
            departments::Entity::find()
                .select_only()
                .column(departments::Column::Id)
                // SeaORM cannot deserialize PostgreSQL ltree directly into String.
                .column_as(Expr::cust("path::text"), "path")
                .column(departments::Column::Depth)
                .filter(departments::Column::Id.is_in(ids))
                .into_model::<DepartmentPath>()
                .all(db)
                .await
                .map_err(|e| {
                    eprintln!("mcp department paths lookup error: {e}");
                    AppError::DbTimeout
                })?
                .into_iter()
                .map(|department| (department.id, department))
                .collect()
        } else {
            HashMap::new()
        };
        Ok(Self {
            context,
            departments,
        })
    }

    pub fn server(
        &self,
        server: &mcp_servers::Model,
        rules: &[mcp_access_policies::Model],
    ) -> ResolvedAccess {
        if let Some(resolved) = resolve_access_from_rules(self.context, rules, &self.departments) {
            return resolved;
        }
        let permission = match server.default_access {
            mcp_servers::McpDefaultAccess::AllUsers => McpPermission::Full,
            mcp_servers::McpDefaultAccess::AdminOnly if self.context.is_admin => {
                McpPermission::Full
            }
            mcp_servers::McpDefaultAccess::AdminOnly
            | mcp_servers::McpDefaultAccess::ExplicitOnly => McpPermission::Denied,
        };
        ResolvedAccess {
            permission,
            resolved_via: McpResolvedVia::ServerDefault,
            priority: 0,
        }
    }

    pub fn tool(
        &self,
        tool: &mcp_tools::Model,
        server_access: &ResolvedAccess,
        rules: &[mcp_access_policies::Model],
    ) -> ResolvedAccess {
        if tool.inherit_access_from_server {
            return ResolvedAccess {
                permission: server_access.permission,
                resolved_via: McpResolvedVia::InheritedFromServer,
                priority: server_access.priority,
            };
        }
        resolve_access_from_rules(self.context, rules, &self.departments).unwrap_or(
            ResolvedAccess {
                permission: McpPermission::Denied,
                resolved_via: McpResolvedVia::ToolDefault,
                priority: 0,
            },
        )
    }
}

pub async fn build_access_context(
    db: &sea_orm::DatabaseConnection,
    user_id: Uuid,
) -> Result<McpAccessContext, AppError> {
    let user = users::Entity::find_by_id(user_id)
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("mcp access user lookup error: {e}");
            AppError::DbTimeout
        })?
        .ok_or(AppError::ResourceNotFound)?;

    let (department_path, department_depth) = if let Some(dept_id) = user.department_id {
        #[derive(Debug, sea_orm::FromQueryResult)]
        struct DeptPathRowDb {
            #[sea_orm(from_alias = "path")]
            path: String,
            #[sea_orm(from_alias = "depth")]
            depth: i32,
        }
        let row = departments::Entity::find_by_id(dept_id)
            .select_only()
            // SeaORM cannot deserialize PostgreSQL ltree directly into String.
            .column_as(Expr::cust("path::text"), "path")
            .column_as(departments::Column::Depth, "depth")
            .into_model::<DeptPathRowDb>()
            .one(db)
            .await
            .map_err(|e| {
                eprintln!("department lookup error: {e}");
                AppError::DbTimeout
            })?;
        match row {
            Some(row) => (Some(row.path), Some(row.depth)),
            None => (None, None),
        }
    } else {
        (None, None)
    };

    let role_ids = user_role_assignments::Entity::find()
        .select_only()
        .column(user_role_assignments::Column::RoleId)
        .filter(user_role_assignments::Column::UserId.eq(user_id))
        .into_tuple::<Uuid>()
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("role id lookup error: {e}");
            AppError::DbTimeout
        })?
        .into_iter()
        .collect::<HashSet<_>>();

    let role_names = if role_ids.is_empty() {
        HashSet::new()
    } else {
        roles::Entity::find()
            .select_only()
            .column(roles::Column::Name)
            .filter(roles::Column::Id.is_in(role_ids.iter().copied()))
            .into_tuple::<String>()
            .all(db)
            .await
            .map_err(|e| {
                eprintln!("role name lookup error: {e}");
                AppError::DbTimeout
            })?
            .into_iter()
            .collect::<HashSet<_>>()
    };

    let authz = AuthorizationService::new(db);
    let is_admin = authz
        .user_has_permission(
            user_id,
            PERMISSION_MCP_ADMIN,
            None,
            PermissionScopeMode::RequireOrgWide,
        )
        .await
        .map_err(|e| {
            eprintln!("mcp admin permission lookup error: {e:?}");
            AppError::DbTimeout
        })?;

    Ok(McpAccessContext {
        user_id,
        department_id: user.department_id,
        department_path,
        department_depth,
        role_ids,
        role_names,
        is_admin,
    })
}

fn permission_rank(permission: McpPermission) -> u8 {
    match permission {
        McpPermission::Denied => 0,
        McpPermission::ReadOnly => 1,
        McpPermission::Full => 2,
    }
}

pub fn delegation_within_ceiling(holder: McpPermission, requested: McpPermission) -> bool {
    permission_rank(requested) <= permission_rank(holder)
}

// Delegation passes on access the delegator already holds; only org-wide MCP admins may unlock a
// restricted server or lift a deny for someone else.
pub async fn ensure_delegation_allowed(
    db: &DatabaseConnection,
    actor_id: Uuid,
    server: &mcp_servers::Model,
    requested: McpPermission,
) -> Result<(), AuthError> {
    let ctx = build_access_context(db, actor_id)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    if ctx.is_admin {
        return Ok(());
    }
    let rules = load_server_rules(db, &[server.id])
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    let held = resolve_server_access_with_rules(db, &ctx, server, &rules)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    if delegation_within_ceiling(held.permission, requested) {
        Ok(())
    } else {
        Err(AuthError::PermissionDenied)
    }
}

// Role-wide rules reach every department, so they stay with org-wide MCP admins like the server rules.
pub async fn ensure_rule_types_allowed(
    db: &DatabaseConnection,
    actor_id: Uuid,
    access_types: &[McpAccessType],
) -> Result<(), AuthError> {
    if !access_types.contains(&McpAccessType::Role) {
        return Ok(());
    }
    AuthorizationService::new(db)
        .ensure_permission(
            actor_id,
            PERMISSION_MCP_ADMIN,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await
}

pub async fn ensure_rule_removal_allowed(
    db: &DatabaseConnection,
    actor_id: Uuid,
    rule: &mcp_access_policies::Model,
) -> Result<(), AuthError> {
    let ctx = build_access_context(db, actor_id)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    if ctx.is_admin || rule.created_by == Some(actor_id) {
        Ok(())
    } else {
        Err(AuthError::PermissionDenied)
    }
}

pub struct ToolAccessChange {
    pub tool_id: Uuid,
    pub inherit_from_server: Option<bool>,
    pub rules: Vec<McpAccessRuleInput>,
}

// Tool rules follow the server rules' guards: a delegator cannot grant above its own access to the
// server (turning inheritance off must not lift that ceiling), and only an admin or a rule's creator
// may remove a rule. Every change locks its tool rows first and is authorized and applied in that one
// transaction, so a rule committed concurrently is either seen by the check or written afterwards.
// With server_id, every tool must belong to that server; any unknown tool rejects the whole request.
pub async fn replace_tool_access(
    db: &DatabaseConnection,
    actor_id: Uuid,
    server_id: Option<Uuid>,
    changes: Vec<ToolAccessChange>,
) -> Result<Vec<(mcp_tools::Model, bool)>, AuthError> {
    let mut ids: Vec<Uuid> = changes.iter().map(|change| change.tool_id).collect();
    ids.sort();
    ids.dedup();
    if ids.len() != changes.len() {
        return Err(AuthError::InvalidRequest { field: "tools" });
    }
    let mut role_ids = Vec::with_capacity(changes.len());
    for change in &changes {
        let mut resolved = Vec::with_capacity(change.rules.len());
        for rule in &change.rules {
            let (role_id, _) =
                resolve_role_reference(db, rule.access_type, rule.role_id, rule.role_name.clone())
                    .await?;
            resolved.push(role_id);
        }
        role_ids.push(resolved);
    }

    let txn = db.begin().await.map_err(|_| AuthError::DbTimeout)?;
    let tools: HashMap<Uuid, mcp_tools::Model> = mcp_tools::Entity::find()
        .filter(mcp_tools::Column::Id.is_in(ids.iter().copied()))
        .order_by_asc(mcp_tools::Column::Id)
        .lock_exclusive()
        .all(&txn)
        .await
        .map_err(|_| AuthError::DbTimeout)?
        .into_iter()
        .map(|tool| (tool.id, tool))
        .collect();
    if tools.len() != ids.len()
        || server_id.is_some_and(|server| tools.values().any(|tool| tool.server_id != server))
    {
        return Err(AuthError::ResourceNotFound);
    }
    for change in &changes {
        let tool = &tools[&change.tool_id];
        let server = mcp_servers::Entity::find_by_id(tool.server_id)
            .one(&txn)
            .await
            .map_err(|_| AuthError::DbTimeout)?
            .ok_or(AuthError::ResourceNotFound)?;
        for rule in &change.rules {
            if rule.access_type != McpAccessType::Role {
                ensure_delegation_allowed(db, actor_id, &server, rule.permission).await?;
            }
        }
        let existing = mcp_access_policies::Entity::find()
            .filter(mcp_access_policies::Column::TargetType.eq(McpAccessTarget::Tool))
            .filter(mcp_access_policies::Column::ToolId.eq(tool.id))
            .all(&txn)
            .await
            .map_err(|_| AuthError::DbTimeout)?;
        for rule in &existing {
            ensure_rule_removal_allowed(db, actor_id, rule).await?;
        }
    }

    let mut applied = Vec::with_capacity(changes.len());
    for (change, role_ids) in changes.into_iter().zip(role_ids) {
        let tool = tools[&change.tool_id].clone();
        let inherit = change
            .inherit_from_server
            .unwrap_or(tool.inherit_access_from_server);
        let mut active: mcp_tools::ActiveModel = tool.clone().into();
        active.inherit_access_from_server = Set(inherit);
        active
            .update(&txn)
            .await
            .map_err(|_| AuthError::DbTimeout)?;
        mcp_access_policies::Entity::delete_many()
            .filter(mcp_access_policies::Column::TargetType.eq(McpAccessTarget::Tool))
            .filter(mcp_access_policies::Column::ToolId.eq(tool.id))
            .exec(&txn)
            .await
            .map_err(|_| AuthError::DbTimeout)?;
        for (rule, role_id) in change.rules.iter().zip(role_ids) {
            mcp_access_policies::ActiveModel {
                id: Set(Uuid::new_v4()),
                target_type: Set(McpAccessTarget::Tool),
                server_id: Set(Some(tool.server_id)),
                tool_id: Set(Some(tool.id)),
                access_type: Set(rule.access_type),
                permission: Set(rule.permission),
                role_id: Set(role_id),
                role_name: Set(None),
                department_id: Set(rule.department_id),
                user_id: Set(rule.user_id),
                inherit_departments: Set(rule.inherit_departments.unwrap_or(true)),
                inherit_from_server: Set(None),
                created_at: Set(chrono::Utc::now()),
                created_by: Set(Some(actor_id)),
            }
            .insert(&txn)
            .await
            .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
        }
        applied.push((tool, inherit));
    }
    txn.commit().await.map_err(|_| AuthError::DbTimeout)?;
    Ok(applied)
}

pub async fn load_server_rules(
    db: &sea_orm::DatabaseConnection,
    server_ids: &[Uuid],
) -> Result<Vec<mcp_access_policies::Model>, AppError> {
    if server_ids.is_empty() {
        return Ok(Vec::new());
    }
    mcp_access_policies::Entity::find()
        .filter(mcp_access_policies::Column::TargetType.eq(McpAccessTarget::Server))
        .filter(mcp_access_policies::Column::ServerId.is_in(server_ids.iter().copied()))
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("mcp server rules lookup error: {e}");
            AppError::DbTimeout
        })
}

pub async fn load_tool_rules(
    db: &sea_orm::DatabaseConnection,
    tool_ids: &[Uuid],
) -> Result<Vec<mcp_access_policies::Model>, AppError> {
    if tool_ids.is_empty() {
        return Ok(Vec::new());
    }
    mcp_access_policies::Entity::find()
        .filter(mcp_access_policies::Column::TargetType.eq(McpAccessTarget::Tool))
        .filter(mcp_access_policies::Column::ToolId.is_in(tool_ids.iter().copied()))
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("mcp tool rules lookup error: {e}");
            AppError::DbTimeout
        })
}

fn resolve_access_from_rules(
    ctx: &McpAccessContext,
    rules: &[mcp_access_policies::Model],
    dept_lookup: &HashMap<Uuid, DepartmentPath>,
) -> Option<ResolvedAccess> {
    if let Some(rule) = rules
        .iter()
        .filter(|rule| rule.access_type == McpAccessType::User)
        .filter(|rule| rule.user_id == Some(ctx.user_id))
        .max_by_key(|rule| rule.created_at)
    {
        return Some(ResolvedAccess {
            permission: rule.permission,
            resolved_via: McpResolvedVia::UserRule,
            priority: 300,
        });
    }

    if let (Some(user_dept_id), Some(user_path), Some(user_depth)) = (
        ctx.department_id,
        ctx.department_path.as_ref(),
        ctx.department_depth,
    ) {
        let dept_rules: Vec<&mcp_access_policies::Model> = rules
            .iter()
            .filter(|rule| rule.access_type == McpAccessType::Department)
            .filter(|rule| rule.department_id.is_some())
            .collect();
        if !dept_rules.is_empty() {
            let mut best_rule: Option<(&mcp_access_policies::Model, i32)> = None;
            for rule in dept_rules {
                let Some(dept_id) = rule.department_id else {
                    continue;
                };
                let Some(DepartmentPath { path, depth, .. }) = dept_lookup.get(&dept_id) else {
                    continue;
                };
                let is_direct = dept_id == user_dept_id;
                if is_direct || (rule.inherit_departments && is_path_within_scope(path, user_path))
                {
                    match best_rule {
                        Some((_, current_depth)) if current_depth >= *depth => {}
                        _ => best_rule = Some((rule, *depth)),
                    }
                }
            }

            if let Some((rule, depth)) = best_rule {
                let depth_diff = (user_depth - depth).max(0);
                let resolved_via = if rule.department_id == Some(user_dept_id) {
                    McpResolvedVia::DepartmentRule
                } else {
                    McpResolvedVia::DepartmentInherited
                };
                return Some(ResolvedAccess {
                    permission: rule.permission,
                    resolved_via,
                    priority: 200 - depth_diff,
                });
            }
        }
    }

    if !ctx.role_ids.is_empty() || !ctx.role_names.is_empty() {
        let mut saw_denied = false;
        let mut saw_full = false;
        let mut saw_read_only = false;
        for rule in rules
            .iter()
            .filter(|rule| rule.access_type == McpAccessType::Role)
        {
            if let Some(role_id) = rule.role_id {
                if !ctx.role_ids.contains(&role_id) {
                    continue;
                }
            } else if let Some(role_name) = rule.role_name.as_ref() {
                if !ctx.role_names.contains(role_name) {
                    continue;
                }
            } else {
                continue;
            }
            match rule.permission {
                McpPermission::Denied => saw_denied = true,
                McpPermission::Full => saw_full = true,
                McpPermission::ReadOnly => saw_read_only = true,
            }
        }
        if saw_denied || saw_full || saw_read_only {
            let permission = if saw_denied {
                McpPermission::Denied
            } else if saw_full {
                McpPermission::Full
            } else {
                McpPermission::ReadOnly
            };
            return Some(ResolvedAccess {
                permission,
                resolved_via: McpResolvedVia::RoleRule,
                priority: 100,
            });
        }
    }

    None
}

pub async fn resolve_server_access_with_rules(
    db: &sea_orm::DatabaseConnection,
    ctx: &McpAccessContext,
    server: &mcp_servers::Model,
    rules: &[mcp_access_policies::Model],
) -> Result<ResolvedAccess, AppError> {
    Ok(McpAccessResolver::load(db, ctx, rules)
        .await?
        .server(server, rules))
}

pub async fn resolve_tool_access_with_rules(
    db: &sea_orm::DatabaseConnection,
    ctx: &McpAccessContext,
    tool: &mcp_tools::Model,
    server_access: &ResolvedAccess,
    rules: &[mcp_access_policies::Model],
) -> Result<ResolvedAccess, AppError> {
    let load_rules = if tool.inherit_access_from_server {
        &[][..]
    } else {
        rules
    };
    Ok(McpAccessResolver::load(db, ctx, load_rules)
        .await?
        .tool(tool, server_access, rules))
}

pub async fn resolve_role_reference(
    db: &DatabaseConnection,
    access_type: mcp_access_policies::McpAccessType,
    role_id: Option<Uuid>,
    role_name: Option<String>,
) -> Result<(Option<Uuid>, Option<String>), AuthError> {
    if access_type != mcp_access_policies::McpAccessType::Role {
        return Ok((None, None));
    }

    if let Some(role_id) = role_id {
        let role = roles::Entity::find_by_id(role_id)
            .one(db)
            .await
            .map_err(|e| {
                eprintln!("role lookup error: {e}");
                AuthError::DbTimeout
            })?
            .ok_or(AuthError::ResourceNotFound)?;
        return Ok((Some(role.id), Some(role.name)));
    }

    if let Some(role_name) = role_name {
        let role = roles::Entity::find()
            .filter(roles::Column::Name.eq(role_name))
            .one(db)
            .await
            .map_err(|e| {
                eprintln!("role lookup error: {e}");
                AuthError::DbTimeout
            })?
            .ok_or(AuthError::ResourceNotFound)?;
        return Ok((Some(role.id), Some(role.name)));
    }

    Err(AuthError::ResourceNotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    // Runs on the disposable security-test database. An admin rule committed while a delegator's
    // replacement waits must be seen by the delegator's check, never deleted by it.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
    async fn a_rule_committed_while_a_delegator_waits_is_not_deleted() {
        use sea_orm::{ConnectionTrait, Database, TransactionTrait};
        let url = std::env::var("GRENGIN_SECURITY_TEST_DATABASE_URL").expect("test database URL");
        assert!(
            url.contains("/grengin_security_test"),
            "refusing a non-test database"
        );
        let db = Database::connect(&url).await.expect("test database");
        let (delegator, creator, server, tool) = (
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        );
        db.execute_unprepared(&format!(
            r#"INSERT INTO users (id, status, email, "emailVerified", "mfaEnabled", "createdAt", "updatedAt", "lastLoginAt")
                 VALUES ('{delegator}', 'active', 'delegator-{delegator}@example.invalid', true, false, now(), now(), now()),
                        ('{creator}', 'active', 'creator-{creator}@example.invalid', true, false, now(), now(), now());
               INSERT INTO mcp_servers (id, name, "transportType", "connectionConfig", enabled, "toolCount", "defaultAccess", "createdAt", "updatedAt")
                 VALUES ('{server}', 'Race fixture', 'http', '{{}}', true, 1, 'admin_only', now(), now());
               INSERT INTO mcp_tools (id, "serverId", "serverName", name, "originalName", "inputSchema", parameters, enabled, "isReadOnly", "inheritAccessFromServer", "createdAt", "updatedAt")
                 VALUES ('{tool}', '{server}', 'Race fixture', 'race_tool', 'race_tool', '{{}}', '{{}}', true, false, true, now(), now());"#
        ))
        .await
        .expect("fixture rows");

        let holder_db = Database::connect(&url).await.expect("second connection");
        let holder = holder_db.begin().await.expect("holder transaction");
        holder
            .execute_unprepared(&format!(
                "SELECT id FROM mcp_tools WHERE id = '{tool}' FOR UPDATE"
            ))
            .await
            .expect("hold the tool row");
        let replacing = {
            let db = db.clone();
            tokio::spawn(async move {
                replace_tool_access(
                    &db,
                    delegator,
                    None,
                    vec![ToolAccessChange {
                        tool_id: tool,
                        inherit_from_server: Some(false),
                        rules: vec![McpAccessRuleInput {
                            access_type: McpAccessType::User,
                            role_id: None,
                            role_name: None,
                            department_id: None,
                            user_id: Some(delegator),
                            permission: McpPermission::Denied,
                            inherit_departments: None,
                        }],
                    }],
                )
                .await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(
            !replacing.is_finished(),
            "the replacement waits for the tool lock"
        );
        holder
            .execute_unprepared(&format!(
                r#"INSERT INTO mcp_access_policies (id, "targetType", "serverId", "toolId", "accessType", permission, "userId", "inheritDepartments", "createdAt", "createdBy")
                     VALUES ('{}', 'tool', '{server}', '{tool}', 'user', 'read_only', '{creator}', true, now(), '{creator}')"#,
                Uuid::new_v4()
            ))
            .await
            .expect("protected rule");
        holder.commit().await.expect("commit the protected rule");

        let outcome = replacing.await.expect("replacement task");
        let kept = mcp_access_policies::Entity::find()
            .filter(mcp_access_policies::Column::ToolId.eq(tool))
            .all(&db)
            .await
            .expect("tool rules");
        db.execute_unprepared(&format!(
            "DELETE FROM mcp_access_policies WHERE \"toolId\" = '{tool}'; DELETE FROM mcp_tools WHERE id = '{tool}'; \
             DELETE FROM mcp_servers WHERE id = '{server}'; DELETE FROM users WHERE id IN ('{delegator}', '{creator}')"
        ))
        .await
        .expect("cleanup");
        assert!(
            matches!(outcome, Err(AuthError::PermissionDenied)),
            "{:?}",
            outcome.map(|_| ())
        );
        assert_eq!(kept.len(), 1, "the protected rule must survive");
        assert_eq!(kept[0].created_by, Some(creator));
    }

    fn context() -> McpAccessContext {
        McpAccessContext {
            user_id: Uuid::from_u128(1),
            department_id: Some(Uuid::from_u128(2)),
            department_path: Some("root.child".into()),
            department_depth: Some(1),
            role_ids: HashSet::from([Uuid::from_u128(3)]),
            role_names: HashSet::from(["User".into()]),
            is_admin: false,
        }
    }

    fn rule(access_type: McpAccessType, permission: McpPermission) -> mcp_access_policies::Model {
        mcp_access_policies::Model {
            id: Uuid::new_v4(),
            target_type: McpAccessTarget::Server,
            server_id: Some(Uuid::from_u128(4)),
            tool_id: None,
            access_type,
            permission,
            role_id: Some(Uuid::from_u128(3)),
            role_name: None,
            department_id: Some(Uuid::from_u128(2)),
            user_id: Some(Uuid::from_u128(1)),
            inherit_departments: true,
            inherit_from_server: None,
            created_at: Utc::now(),
            created_by: None,
        }
    }

    fn paths() -> HashMap<Uuid, DepartmentPath> {
        [(2, "root.child", 1), (5, "root", 0), (6, "roots", 0)]
            .into_iter()
            .map(|(id, path, depth)| {
                (
                    Uuid::from_u128(id),
                    DepartmentPath {
                        id: Uuid::from_u128(id),
                        path: path.into(),
                        depth,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn delegation_never_grants_more_than_the_delegator_holds() {
        use McpPermission::{Denied, Full, ReadOnly};
        assert!(delegation_within_ceiling(Full, Full));
        assert!(delegation_within_ceiling(Full, ReadOnly));
        assert!(delegation_within_ceiling(ReadOnly, ReadOnly));
        assert!(delegation_within_ceiling(Denied, Denied));
        assert!(delegation_within_ceiling(ReadOnly, Denied));
        assert!(!delegation_within_ceiling(ReadOnly, Full));
        assert!(!delegation_within_ceiling(Denied, ReadOnly));
        assert!(!delegation_within_ceiling(Denied, Full));
    }

    #[test]
    fn batched_rules_preserve_latest_user_override_before_department_and_role() {
        let context = context();
        let old = rule(McpAccessType::User, McpPermission::Denied);
        let mut current = old.clone();
        current.permission = McpPermission::ReadOnly;
        current.created_at += Duration::seconds(1);
        let rules = [
            rule(McpAccessType::Department, McpPermission::Denied),
            old,
            rule(McpAccessType::Role, McpPermission::Denied),
            current,
        ];
        let result = resolve_access_from_rules(&context, &rules, &paths()).unwrap();
        assert_eq!(result.permission, McpPermission::ReadOnly);
        assert!(matches!(result.resolved_via, McpResolvedVia::UserRule));
    }

    #[test]
    fn batched_departments_preserve_specificity_inheritance_and_path_boundaries() {
        let context = context();
        let mut inherited = rule(McpAccessType::Department, McpPermission::ReadOnly);
        inherited.department_id = Some(Uuid::from_u128(5));
        let direct = rule(McpAccessType::Department, McpPermission::Denied);
        let lookup = paths();
        let result = resolve_access_from_rules(&context, &[inherited.clone()], &lookup).unwrap();
        assert_eq!(result.permission, McpPermission::ReadOnly);
        assert!(matches!(
            result.resolved_via,
            McpResolvedVia::DepartmentInherited
        ));
        assert_eq!(
            resolve_access_from_rules(&context, &[inherited.clone(), direct], &lookup)
                .unwrap()
                .permission,
            McpPermission::Denied
        );
        inherited.inherit_departments = false;
        assert!(resolve_access_from_rules(&context, &[inherited.clone()], &lookup).is_none());
        inherited.inherit_departments = true;
        inherited.department_id = Some(Uuid::from_u128(6));
        assert!(resolve_access_from_rules(&context, &[inherited.clone()], &lookup).is_none());
        inherited.department_id = Some(Uuid::new_v4());
        assert!(resolve_access_from_rules(&context, &[inherited], &lookup).is_none());
    }

    #[test]
    fn batched_role_rules_keep_deny_precedence_and_ignore_unmatched_roles() {
        let context = context();
        let read = rule(McpAccessType::Role, McpPermission::ReadOnly);
        let full = rule(McpAccessType::Role, McpPermission::Full);
        let mut denied = rule(McpAccessType::Role, McpPermission::Denied);
        assert_eq!(
            resolve_access_from_rules(
                &context,
                &[read.clone(), full.clone(), denied.clone()],
                &paths()
            )
            .unwrap()
            .permission,
            McpPermission::Denied
        );
        denied.role_id = Some(Uuid::new_v4());
        assert_eq!(
            resolve_access_from_rules(&context, &[read, full, denied], &paths())
                .unwrap()
                .permission,
            McpPermission::Full
        );
    }

    #[test]
    fn no_department_does_not_match_cached_department_paths() {
        let mut context = context();
        context.department_id = None;
        assert!(
            resolve_access_from_rules(
                &context,
                &[rule(McpAccessType::Department, McpPermission::Full)],
                &paths()
            )
            .is_none()
        );
    }
}
