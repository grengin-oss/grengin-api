// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::collections::{HashMap, HashSet};

use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, JoinType, QueryFilter, QuerySelect,
    RelationTrait, sea_query::Expr,
};
use uuid::Uuid;

use crate::{
    auth::{
        error::AuthError,
        permissions::{ROLE_SUPER_ADMIN, permission_key},
    },
    models::{departments, permissions, role_permissions, roles, user_role_assignments, users},
    services::authorization::is_path_within_scope,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepartmentScope<'a> {
    OrgWide,
    Department(&'a str),
}

impl<'a> DepartmentScope<'a> {
    pub fn from_path(path: Option<&'a str>) -> Self {
        path.map_or(Self::OrgWide, Self::Department)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GrantScope {
    OrgWide,
    Department(String),
}

#[derive(Debug, Clone)]
struct PermissionGrant {
    permission: String,
    is_scopeable: bool,
    scope: GrantScope,
}

pub struct RoleAssignmentRow {
    pub role_name: String,
    pub scope_department_id: Option<Uuid>,
}

pub struct PermissionAssignmentRow {
    pub domain: String,
    pub action: String,
    pub is_scopeable: bool,
    pub scope_department_id: Option<Uuid>,
    pub scope_path: Option<String>,
}

// Mirrors AuthorizationService::user_has_permission with RequireOrgWide, evaluated in memory so one
// snapshot answers many scope questions; a scoped "Super Admin" assignment does not count as Super Admin.
#[derive(Debug, Clone)]
pub struct PermissionGrants {
    super_admin: bool,
    grants: Vec<PermissionGrant>,
}

impl PermissionGrants {
    pub fn from_rows(
        roles: &[RoleAssignmentRow],
        permissions: Vec<PermissionAssignmentRow>,
    ) -> Self {
        let super_admin = roles
            .iter()
            .any(|row| row.role_name == ROLE_SUPER_ADMIN && row.scope_department_id.is_none());
        let grants = permissions
            .into_iter()
            .filter_map(|row| {
                let scope = match (row.scope_department_id, row.scope_path) {
                    (None, _) => GrantScope::OrgWide,
                    (Some(_), Some(path)) => GrantScope::Department(path),
                    (Some(_), None) => return None,
                };
                Some(PermissionGrant {
                    permission: permission_key(&row.domain, &row.action),
                    is_scopeable: row.is_scopeable,
                    scope,
                })
            })
            .collect();
        Self {
            super_admin,
            grants,
        }
    }

    pub async fn load(db: &DatabaseConnection, user_id: Uuid) -> Result<Self, AuthError> {
        let roles = user_role_assignments::Entity::find()
            .select_only()
            .column_as(roles::Column::Name, "role_name")
            .column(user_role_assignments::Column::ScopeDepartmentId)
            .join(
                JoinType::InnerJoin,
                user_role_assignments::Relation::Roles.def(),
            )
            .filter(user_role_assignments::Column::UserId.eq(user_id))
            .into_tuple::<(String, Option<Uuid>)>()
            .all(db)
            .await
            .map_err(|e| {
                eprintln!("role grants lookup error: {e}");
                AuthError::DbTimeout
            })?
            .into_iter()
            .map(|(role_name, scope_department_id)| RoleAssignmentRow {
                role_name,
                scope_department_id,
            })
            .collect::<Vec<_>>();

        let permissions = user_role_assignments::Entity::find()
            .select_only()
            .column_as(permissions::Column::Domain, "domain")
            .column_as(permissions::Column::Action, "action")
            .column_as(permissions::Column::IsScopeable, "is_scopeable")
            .column(user_role_assignments::Column::ScopeDepartmentId)
            .column_as(Expr::cust("departments.path::text"), "scope_path")
            .join(
                JoinType::InnerJoin,
                user_role_assignments::Relation::Roles.def(),
            )
            .join(JoinType::InnerJoin, roles::Relation::RolePermissions.def())
            .join(
                JoinType::InnerJoin,
                role_permissions::Relation::Permissions.def(),
            )
            .join(
                JoinType::LeftJoin,
                user_role_assignments::Relation::ScopeDepartments.def(),
            )
            .filter(user_role_assignments::Column::UserId.eq(user_id))
            .into_tuple::<(String, String, bool, Option<Uuid>, Option<String>)>()
            .all(db)
            .await
            .map_err(|e| {
                eprintln!("permission grants lookup error: {e}");
                AuthError::DbTimeout
            })?
            .into_iter()
            .map(
                |(domain, action, is_scopeable, scope_department_id, scope_path)| {
                    PermissionAssignmentRow {
                        domain,
                        action,
                        is_scopeable,
                        scope_department_id,
                        scope_path,
                    }
                },
            )
            .collect();

        Ok(Self::from_rows(&roles, permissions))
    }

    pub fn is_super_admin(&self) -> bool {
        self.super_admin
    }

    pub fn holds(&self, permission: &str, target: DepartmentScope<'_>) -> bool {
        self.grants
            .iter()
            .filter(|grant| grant.permission == permission)
            .any(|grant| match (&grant.scope, target) {
                (GrantScope::OrgWide, _) => true,
                (GrantScope::Department(scope), DepartmentScope::Department(path)) => {
                    grant.is_scopeable && is_path_within_scope(scope, path)
                }
                (GrantScope::Department(_), DepartmentScope::OrgWide) => false,
            })
    }

    pub fn missing(&self, permissions: &[String], target: DepartmentScope<'_>) -> Vec<String> {
        let mut missing: Vec<String> = permissions
            .iter()
            .filter(|permission| !self.holds(permission, target))
            .cloned()
            .collect();
        missing.sort_unstable();
        missing.dedup();
        missing
    }
}

pub async fn load_department_paths(
    db: &DatabaseConnection,
    department_ids: &[Uuid],
) -> Result<HashMap<Uuid, String>, AuthError> {
    if department_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let unique: HashSet<Uuid> = department_ids.iter().copied().collect();
    let rows = departments::Entity::find()
        .select_only()
        .column(departments::Column::Id)
        .column_as(Expr::cust("path::text"), "path")
        .filter(departments::Column::Id.is_in(unique))
        .into_tuple::<(Uuid, String)>()
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("department paths lookup error: {e}");
            AuthError::DbTimeout
        })?;
    Ok(rows.into_iter().collect())
}

pub async fn load_department_path(
    db: &DatabaseConnection,
    department_id: Option<Uuid>,
) -> Result<Option<String>, AuthError> {
    let Some(department_id) = department_id else {
        return Ok(None);
    };
    load_department_paths(db, &[department_id])
        .await?
        .remove(&department_id)
        .map(Some)
        .ok_or(AuthError::ResourceNotFound)
}

pub async fn load_user_department_paths(
    db: &DatabaseConnection,
    user_ids: &[Uuid],
) -> Result<HashMap<Uuid, Option<String>>, AuthError> {
    if user_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let unique: HashSet<Uuid> = user_ids.iter().copied().collect();
    let user_departments = users::Entity::find()
        .select_only()
        .column(users::Column::Id)
        .column(users::Column::DepartmentId)
        .filter(users::Column::Id.is_in(unique))
        .into_tuple::<(Uuid, Option<Uuid>)>()
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("user departments lookup error: {e}");
            AuthError::DbTimeout
        })?;
    let department_ids: Vec<Uuid> = user_departments
        .iter()
        .filter_map(|(_, department_id)| *department_id)
        .collect();
    let paths = load_department_paths(db, &department_ids).await?;
    Ok(user_departments
        .into_iter()
        .map(|(user_id, department_id)| {
            (
                user_id,
                department_id.and_then(|id| paths.get(&id).cloned()),
            )
        })
        .collect())
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn grant(
        permission: &str,
        is_scopeable: bool,
        scope: Option<&str>,
    ) -> PermissionAssignmentRow {
        let (domain, action) = permission.split_once(':').unwrap_or_default();
        PermissionAssignmentRow {
            domain: domain.to_string(),
            action: action.to_string(),
            is_scopeable,
            scope_department_id: scope.map(|_| Uuid::new_v4()),
            scope_path: scope.map(str::to_string),
        }
    }

    pub fn role(name: &str, scope: Option<Uuid>) -> RoleAssignmentRow {
        RoleAssignmentRow {
            role_name: name.to_string(),
            scope_department_id: scope,
        }
    }

    #[test]
    fn org_wide_grant_covers_every_department_and_org_wide_target() {
        let grants = PermissionGrants::from_rows(&[], vec![grant("users:manage", true, None)]);
        assert!(grants.holds("users:manage", DepartmentScope::OrgWide));
        assert!(grants.holds("users:manage", DepartmentScope::Department("a.b")));
        assert!(!grants.holds("users:view", DepartmentScope::OrgWide));
    }

    #[test]
    fn scoped_grant_covers_its_subtree_only() {
        let grants =
            PermissionGrants::from_rows(&[], vec![grant("users:manage", true, Some("a.b"))]);
        assert!(grants.holds("users:manage", DepartmentScope::Department("a.b")));
        assert!(grants.holds("users:manage", DepartmentScope::Department("a.b.c")));
        assert!(!grants.holds("users:manage", DepartmentScope::Department("a")));
        assert!(!grants.holds("users:manage", DepartmentScope::Department("a.bc")));
        assert!(!grants.holds("users:manage", DepartmentScope::Department("a.x")));
        assert!(!grants.holds("users:manage", DepartmentScope::OrgWide));
    }

    #[test]
    fn scoped_grant_of_non_scopeable_permission_grants_nothing() {
        let grants =
            PermissionGrants::from_rows(&[], vec![grant("sso_providers:manage", false, Some("a"))]);
        assert!(!grants.holds("sso_providers:manage", DepartmentScope::Department("a")));
        assert!(!grants.holds("sso_providers:manage", DepartmentScope::OrgWide));
    }

    #[test]
    fn scoped_assignment_with_unknown_department_is_ignored() {
        let mut row = grant("users:manage", true, Some("a"));
        row.scope_path = None;
        let grants = PermissionGrants::from_rows(&[], vec![row]);
        assert!(!grants.holds("users:manage", DepartmentScope::OrgWide));
        assert!(!grants.holds("users:manage", DepartmentScope::Department("a")));
    }

    #[test]
    fn only_org_wide_super_admin_assignment_counts_as_super_admin() {
        let org_wide = PermissionGrants::from_rows(&[role(ROLE_SUPER_ADMIN, None)], vec![]);
        assert!(org_wide.is_super_admin());

        let scoped =
            PermissionGrants::from_rows(&[role(ROLE_SUPER_ADMIN, Some(Uuid::new_v4()))], vec![]);
        assert!(!scoped.is_super_admin());

        let other = PermissionGrants::from_rows(&[role("HR Admin", None)], vec![]);
        assert!(!other.is_super_admin());
    }

    #[test]
    fn missing_lists_each_permission_not_held_at_target_once() {
        let grants = PermissionGrants::from_rows(
            &[],
            vec![
                grant("users:view", true, Some("a")),
                grant("roles:view", false, None),
            ],
        );
        let requested = vec![
            "users:view".to_string(),
            "roles:view".to_string(),
            "budget:allocate".to_string(),
            "budget:allocate".to_string(),
        ];
        assert_eq!(
            grants.missing(&requested, DepartmentScope::Department("a.b")),
            vec!["budget:allocate".to_string()]
        );
        assert_eq!(
            grants.missing(&requested, DepartmentScope::OrgWide),
            vec!["budget:allocate".to_string(), "users:view".to_string()]
        );
    }

    #[test]
    fn department_scope_from_missing_path_is_org_wide() {
        assert_eq!(DepartmentScope::from_path(None), DepartmentScope::OrgWide);
        assert_eq!(
            DepartmentScope::from_path(Some("a")),
            DepartmentScope::Department("a")
        );
    }
}
