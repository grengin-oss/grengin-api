// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{collections::HashSet, fmt};

use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, JoinType, QueryFilter, QuerySelect, RelationTrait,
};
use uuid::Uuid;

use crate::{
    auth::{
        error::AuthError,
        permissions::{PERMISSION_ROLES_ASSIGN, ROLE_SUPER_ADMIN, permission_key},
    },
    dto::admin_roles::RoleChangeDeniedPayload,
    models::{permissions, role_permissions, roles},
    services::{
        auth_audit::{build_audit_payload, record_auth_event},
        permission_grants::{
            DepartmentScope, PermissionGrants, load_department_path, load_user_department_paths,
        },
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoleChangeDenial {
    SuperAdminOnly,
    SuperAdminMustBeOrgWide,
    ScopeOutsideAssignerScope,
    UserOutsideAssignerScope,
    MissingRolePermissions(Vec<String>),
}

impl fmt::Display for RoleChangeDenial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            Self::SuperAdminOnly => "super_admin_only",
            Self::SuperAdminMustBeOrgWide => "super_admin_must_be_org_wide",
            Self::ScopeOutsideAssignerScope => "scope_outside_assigner_scope",
            Self::UserOutsideAssignerScope => "user_outside_assigner_scope",
            Self::MissingRolePermissions(_) => "missing_role_permissions",
        };
        f.write_str(reason)
    }
}

pub struct RoleAssignmentRequest<'a> {
    pub role_name: &'a str,
    pub role_permissions: &'a [String],
    pub scope: DepartmentScope<'a>,
    pub user_department: DepartmentScope<'a>,
}

pub fn check_role_assignment(
    assigner: &PermissionGrants,
    request: &RoleAssignmentRequest<'_>,
) -> Result<(), RoleChangeDenial> {
    if request.role_name == ROLE_SUPER_ADMIN && !assigner.is_super_admin() {
        return Err(RoleChangeDenial::SuperAdminOnly);
    }
    if !assigner.holds(PERMISSION_ROLES_ASSIGN, request.scope) {
        return Err(RoleChangeDenial::ScopeOutsideAssignerScope);
    }
    if !assigner.holds(PERMISSION_ROLES_ASSIGN, request.user_department) {
        return Err(RoleChangeDenial::UserOutsideAssignerScope);
    }
    check_permissions_held(assigner, request.role_permissions, request.scope)
}

// Role-name checks treat Super Admin as global, so a department-scoped Super Admin
// grant would act org-wide in some places and scoped in others.
pub fn check_super_admin_grant_scope(
    role_name: &str,
    scope_department_id: Option<Uuid>,
) -> Result<(), RoleChangeDenial> {
    if role_name == ROLE_SUPER_ADMIN && scope_department_id.is_some() {
        Err(RoleChangeDenial::SuperAdminMustBeOrgWide)
    } else {
        Ok(())
    }
}

pub fn check_role_definition(
    editor: &PermissionGrants,
    added_permissions: &[String],
) -> Result<(), RoleChangeDenial> {
    check_permissions_held(editor, added_permissions, DepartmentScope::OrgWide)
}

pub fn added_permissions(existing: &[String], requested: &[String]) -> Vec<String> {
    let existing: HashSet<&String> = existing.iter().collect();
    let mut added: Vec<String> = requested
        .iter()
        .filter(|permission| !existing.contains(permission))
        .cloned()
        .collect();
    added.sort_unstable();
    added.dedup();
    added
}

fn check_permissions_held(
    grants: &PermissionGrants,
    required: &[String],
    scope: DepartmentScope<'_>,
) -> Result<(), RoleChangeDenial> {
    if grants.is_super_admin() {
        return Ok(());
    }
    let missing = grants.missing(required, scope);
    if missing.is_empty() {
        Ok(())
    } else {
        Err(RoleChangeDenial::MissingRolePermissions(missing))
    }
}

pub async fn load_role_permission_keys(
    db: &DatabaseConnection,
    role_id: Uuid,
) -> Result<Vec<String>, AuthError> {
    let rows = role_permissions::Entity::find()
        .select_only()
        .column_as(permissions::Column::Domain, "domain")
        .column_as(permissions::Column::Action, "action")
        .join(
            JoinType::InnerJoin,
            role_permissions::Relation::Permissions.def(),
        )
        .filter(role_permissions::Column::RoleId.eq(role_id))
        .into_tuple::<(String, String)>()
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("role permissions lookup error: {e}");
            AuthError::DbTimeout
        })?;
    Ok(rows
        .into_iter()
        .map(|(domain, action)| permission_key(&domain, &action))
        .collect())
}

pub async fn ensure_role_assignment_allowed(
    db: &DatabaseConnection,
    actor_id: Uuid,
    role: &roles::Model,
    scope_department_id: Option<Uuid>,
    user_ids: &[Uuid],
) -> Result<(), AuthError> {
    if user_ids.is_empty() {
        return Ok(());
    }
    let assigner = PermissionGrants::load(db, actor_id).await?;
    let role_permissions = load_role_permission_keys(db, role.id).await?;
    let scope_path = load_department_path(db, scope_department_id).await?;
    let user_paths = load_user_department_paths(db, user_ids).await?;

    for user_id in user_ids {
        let user_path = user_paths.get(user_id).ok_or(AuthError::ResourceNotFound)?;
        let request = RoleAssignmentRequest {
            role_name: &role.name,
            role_permissions: &role_permissions,
            scope: DepartmentScope::from_path(scope_path.as_deref()),
            user_department: DepartmentScope::from_path(user_path.as_deref()),
        };
        if let Err(denial) = check_role_assignment(&assigner, &request) {
            return Err(deny(
                db,
                actor_id,
                denial,
                Some(role.id),
                Some(*user_id),
                scope_department_id,
            )
            .await);
        }
    }
    Ok(())
}

pub async fn ensure_role_grant_allowed(
    db: &DatabaseConnection,
    actor_id: Uuid,
    role: &roles::Model,
    scope_department_id: Option<Uuid>,
    user_ids: &[Uuid],
) -> Result<(), AuthError> {
    if let Err(denial) = check_super_admin_grant_scope(&role.name, scope_department_id) {
        return Err(deny(
            db,
            actor_id,
            denial,
            Some(role.id),
            user_ids.first().copied(),
            scope_department_id,
        )
        .await);
    }
    ensure_role_assignment_allowed(db, actor_id, role, scope_department_id, user_ids).await
}

pub async fn ensure_role_definition_allowed(
    db: &DatabaseConnection,
    actor_id: Uuid,
    role_id: Option<Uuid>,
    added_permissions: &[String],
) -> Result<(), AuthError> {
    if added_permissions.is_empty() {
        return Ok(());
    }
    let editor = PermissionGrants::load(db, actor_id).await?;
    match check_role_definition(&editor, added_permissions) {
        Ok(()) => Ok(()),
        Err(denial) => Err(deny(db, actor_id, denial, role_id, None, None).await),
    }
}

async fn deny(
    db: &DatabaseConnection,
    actor_id: Uuid,
    denial: RoleChangeDenial,
    role_id: Option<Uuid>,
    user_id: Option<Uuid>,
    scope_department_id: Option<Uuid>,
) -> AuthError {
    let missing_permissions = match &denial {
        RoleChangeDenial::MissingRolePermissions(missing) => missing.clone(),
        _ => Vec::new(),
    };
    if let Some(payload) = build_audit_payload(RoleChangeDeniedPayload {
        reason: denial.to_string(),
        role_id,
        user_id,
        scope_department_id,
        missing_permissions,
    }) {
        let _ = record_auth_event(db, "auth.permission_denied", Some(actor_id), payload).await;
    }
    AuthError::PermissionDenied
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        auth::permissions::ROLE_DEPARTMENT_ADMIN,
        services::permission_grants::tests::{grant, role},
    };

    fn keys(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn super_admin() -> PermissionGrants {
        PermissionGrants::from_rows(
            &[role(ROLE_SUPER_ADMIN, None)],
            vec![
                grant("roles:assign", true, None),
                grant("users:manage", true, None),
            ],
        )
    }

    fn org_wide_hr_admin() -> PermissionGrants {
        PermissionGrants::from_rows(
            &[role("HR Admin", None)],
            vec![
                grant("roles:assign", true, None),
                grant("users:view", true, None),
                grant("users:manage", true, None),
            ],
        )
    }

    fn scoped_assigner(scope: &str) -> PermissionGrants {
        PermissionGrants::from_rows(
            &[role("Department Lead", Some(Uuid::new_v4()))],
            vec![
                grant("roles:assign", true, Some(scope)),
                grant("users:view", true, Some(scope)),
                grant("users:manage", true, Some(scope)),
            ],
        )
    }

    fn request<'a>(
        role_name: &'a str,
        role_permissions: &'a [String],
        scope: DepartmentScope<'a>,
        user_department: DepartmentScope<'a>,
    ) -> RoleAssignmentRequest<'a> {
        RoleAssignmentRequest {
            role_name,
            role_permissions,
            scope,
            user_department,
        }
    }

    #[test]
    fn super_admin_can_grant_super_admin_org_wide() {
        let permissions = keys(&["roles:manage", "system:maintain"]);
        let result = check_role_assignment(
            &super_admin(),
            &request(
                ROLE_SUPER_ADMIN,
                &permissions,
                DepartmentScope::OrgWide,
                DepartmentScope::OrgWide,
            ),
        );
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn org_wide_assigner_who_is_not_super_admin_cannot_grant_super_admin() {
        let permissions = keys(&["users:view"]);
        let result = check_role_assignment(
            &org_wide_hr_admin(),
            &request(
                ROLE_SUPER_ADMIN,
                &permissions,
                DepartmentScope::OrgWide,
                DepartmentScope::Department("a"),
            ),
        );
        assert_eq!(result, Err(RoleChangeDenial::SuperAdminOnly));
    }

    #[test]
    fn scoped_super_admin_assignment_cannot_grant_super_admin() {
        let grants = PermissionGrants::from_rows(
            &[role(ROLE_SUPER_ADMIN, Some(Uuid::new_v4()))],
            vec![grant("roles:assign", true, Some("a"))],
        );
        let permissions = keys(&[]);
        let result = check_role_assignment(
            &grants,
            &request(
                ROLE_SUPER_ADMIN,
                &permissions,
                DepartmentScope::Department("a"),
                DepartmentScope::Department("a"),
            ),
        );
        assert_eq!(result, Err(RoleChangeDenial::SuperAdminOnly));
    }

    #[test]
    fn scoped_assigner_cannot_create_org_wide_assignment() {
        let permissions = keys(&["users:view"]);
        let result = check_role_assignment(
            &scoped_assigner("a.b"),
            &request(
                "Observer",
                &permissions,
                DepartmentScope::OrgWide,
                DepartmentScope::Department("a.b"),
            ),
        );
        assert_eq!(result, Err(RoleChangeDenial::ScopeOutsideAssignerScope));
    }

    #[test]
    fn scoped_assigner_cannot_scope_assignment_outside_subtree() {
        let permissions = keys(&["users:view"]);
        let result = check_role_assignment(
            &scoped_assigner("a.b"),
            &request(
                "Observer",
                &permissions,
                DepartmentScope::Department("a.c"),
                DepartmentScope::Department("a.b"),
            ),
        );
        assert_eq!(result, Err(RoleChangeDenial::ScopeOutsideAssignerScope));

        let parent = check_role_assignment(
            &scoped_assigner("a.b"),
            &request(
                "Observer",
                &permissions,
                DepartmentScope::Department("a"),
                DepartmentScope::Department("a.b"),
            ),
        );
        assert_eq!(parent, Err(RoleChangeDenial::ScopeOutsideAssignerScope));
    }

    #[test]
    fn scoped_assigner_can_grant_held_permissions_inside_subtree() {
        let permissions = keys(&["users:view", "users:manage"]);
        let result = check_role_assignment(
            &scoped_assigner("a.b"),
            &request(
                "Team Lead",
                &permissions,
                DepartmentScope::Department("a.b.c"),
                DepartmentScope::Department("a.b.c.d"),
            ),
        );
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn scoped_assigner_cannot_assign_roles_to_user_outside_scope() {
        let permissions = keys(&["users:view"]);
        let outside = check_role_assignment(
            &scoped_assigner("a.b"),
            &request(
                "Observer",
                &permissions,
                DepartmentScope::Department("a.b"),
                DepartmentScope::Department("a.c"),
            ),
        );
        assert_eq!(outside, Err(RoleChangeDenial::UserOutsideAssignerScope));

        let unassigned = check_role_assignment(
            &scoped_assigner("a.b"),
            &request(
                "Observer",
                &permissions,
                DepartmentScope::Department("a.b"),
                DepartmentScope::OrgWide,
            ),
        );
        assert_eq!(unassigned, Err(RoleChangeDenial::UserOutsideAssignerScope));
    }

    #[test]
    fn assigner_cannot_grant_role_carrying_permissions_they_lack() {
        let permissions = keys(&["departments:manage", "budget:allocate", "users:manage"]);
        let result = check_role_assignment(
            &org_wide_hr_admin(),
            &request(
                ROLE_DEPARTMENT_ADMIN,
                &permissions,
                DepartmentScope::Department("a"),
                DepartmentScope::Department("a"),
            ),
        );
        assert_eq!(
            result,
            Err(RoleChangeDenial::MissingRolePermissions(keys(&[
                "budget:allocate",
                "departments:manage",
            ])))
        );
    }

    #[test]
    fn org_wide_non_scopeable_permission_is_needed_even_for_scoped_grants() {
        let grants = PermissionGrants::from_rows(
            &[],
            vec![
                grant("roles:assign", true, Some("a")),
                grant("sso_providers:manage", false, Some("a")),
            ],
        );
        let permissions = keys(&["sso_providers:manage"]);
        let result = check_role_assignment(
            &grants,
            &request(
                "IT Admin",
                &permissions,
                DepartmentScope::Department("a"),
                DepartmentScope::Department("a"),
            ),
        );
        assert_eq!(
            result,
            Err(RoleChangeDenial::MissingRolePermissions(keys(&[
                "sso_providers:manage"
            ])))
        );
    }

    #[test]
    fn super_admin_may_grant_permissions_missing_from_their_own_roles() {
        let permissions = keys(&["skills:manage"]);
        let result = check_role_assignment(
            &super_admin(),
            &request(
                "Skills Curator",
                &permissions,
                DepartmentScope::Department("a"),
                DepartmentScope::Department("b"),
            ),
        );
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn assigner_without_roles_assign_is_denied_at_any_scope() {
        let grants = PermissionGrants::from_rows(&[], vec![grant("users:manage", true, None)]);
        let permissions = keys(&[]);
        let result = check_role_assignment(
            &grants,
            &request(
                "User",
                &permissions,
                DepartmentScope::Department("a"),
                DepartmentScope::Department("a"),
            ),
        );
        assert_eq!(result, Err(RoleChangeDenial::ScopeOutsideAssignerScope));
    }

    #[test]
    fn role_editor_cannot_add_permissions_they_do_not_hold_org_wide() {
        let grants = PermissionGrants::from_rows(
            &[],
            vec![
                grant("roles:manage", false, None),
                grant("users:view", true, Some("a")),
            ],
        );
        assert_eq!(
            check_role_definition(&grants, &keys(&["roles:manage"])),
            Ok(())
        );
        assert_eq!(
            check_role_definition(&grants, &keys(&["users:view", "system:maintain"])),
            Err(RoleChangeDenial::MissingRolePermissions(keys(&[
                "system:maintain",
                "users:view",
            ])))
        );
        assert_eq!(
            check_role_definition(&super_admin(), &keys(&["system:maintain"])),
            Ok(())
        );
    }

    #[test]
    fn added_permissions_ignores_kept_and_removed_permissions() {
        let existing = keys(&["users:view", "users:manage"]);
        let requested = keys(&["users:view", "budget:view", "budget:view"]);
        assert_eq!(
            added_permissions(&existing, &requested),
            keys(&["budget:view"])
        );
        assert!(added_permissions(&existing, &keys(&[])).is_empty());
    }

    #[test]
    fn super_admin_can_only_be_granted_org_wide() {
        assert_eq!(
            check_super_admin_grant_scope(ROLE_SUPER_ADMIN, None),
            Ok(())
        );
        assert_eq!(
            check_super_admin_grant_scope(ROLE_SUPER_ADMIN, Some(Uuid::new_v4())),
            Err(RoleChangeDenial::SuperAdminMustBeOrgWide)
        );
    }

    #[test]
    fn other_roles_may_still_be_granted_with_a_department_scope() {
        assert_eq!(
            check_super_admin_grant_scope("Department Admin", Some(Uuid::new_v4())),
            Ok(())
        );
        assert_eq!(check_super_admin_grant_scope("User", None), Ok(()));
    }

    #[test]
    fn denial_reasons_are_stable_audit_strings() {
        assert_eq!(
            RoleChangeDenial::SuperAdminOnly.to_string(),
            "super_admin_only"
        );
        assert_eq!(
            RoleChangeDenial::SuperAdminMustBeOrgWide.to_string(),
            "super_admin_must_be_org_wide"
        );
        assert_eq!(
            RoleChangeDenial::MissingRolePermissions(vec![]).to_string(),
            "missing_role_permissions"
        );
    }
}
