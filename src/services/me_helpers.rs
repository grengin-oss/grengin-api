// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::{claims::Claims, error::AuthError, permissions::ROLE_SUPER_ADMIN},
    dto::me::PermissionScope,
    models::{departments, permissions, role_permissions, roles, user_role_assignments},
    services::authorization::AuthorizationService,
    state::SharedState,
};
use sea_orm::sea_query::{Alias, BinOper, Expr};
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, EntityTrait, JoinType, PaginatorTrait, QueryFilter,
    QuerySelect, RelationTrait,
};
use serde_json::Value;
use uuid::Uuid;

pub async fn load_administered_department_ids(
    claims: Claims,
    app_state: &SharedState,
) -> Result<Vec<Uuid>, AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    if authz
        .user_has_role_name(claims.user_id, ROLE_SUPER_ADMIN)
        .await?
    {
        return load_all_department_ids(&app_state.database).await;
    }

    let rows = user_role_assignments::Entity::find()
        .select_only()
        .column(user_role_assignments::Column::ScopeDepartmentId)
        .column_as(permissions::Column::Action, "action")
        .join(
            JoinType::InnerJoin,
            user_role_assignments::Relation::Roles.def(),
        )
        .join(JoinType::InnerJoin, roles::Relation::RolePermissions.def())
        .join(
            JoinType::InnerJoin,
            role_permissions::Relation::Permissions.def(),
        )
        .filter(user_role_assignments::Column::UserId.eq(claims.user_id))
        .filter(permissions::Column::Domain.eq("departments"))
        .filter(
            Condition::any()
                .add(permissions::Column::Action.eq("manage"))
                .add(permissions::Column::Action.eq("view")),
        )
        .into_tuple::<(Option<Uuid>, String)>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("department permission scope lookup error: {e}");
            AuthError::DbTimeout
        })?;

    match preferred_department_scope(&rows) {
        PermissionScope::OrgWide => load_all_department_ids(&app_state.database).await,
        PermissionScope::Scoped(ids) => Ok(ids),
        PermissionScope::Missing => Ok(Vec::new()),
    }
}

fn preferred_department_scope(rows: &[(Option<Uuid>, String)]) -> PermissionScope {
    let manage_scope = permission_scope_from_rows(rows, "manage");
    if !matches!(manage_scope, PermissionScope::Missing) {
        return manage_scope;
    }
    permission_scope_from_rows(rows, "view")
}

fn permission_scope_from_rows(rows: &[(Option<Uuid>, String)], action: &str) -> PermissionScope {
    let mut matched = false;
    let mut ids = Vec::new();
    for (scope_id, row_action) in rows {
        if row_action != action {
            continue;
        }
        matched = true;
        match scope_id {
            None => return PermissionScope::OrgWide,
            Some(id) => ids.push(*id),
        }
    }

    if !matched {
        PermissionScope::Missing
    } else {
        ids.sort_unstable();
        ids.dedup();
        PermissionScope::Scoped(ids)
    }
}

pub fn needs_effective_permissions_refresh(value: &Option<Value>) -> bool {
    match value {
        Some(Value::Object(map)) => {
            !map.contains_key("permissions")
                || !map.contains_key("mcp_access")
                || !map.contains_key("administered_departments")
        }
        Some(_) => true,
        None => true,
    }
}

pub fn parse_permission_scope(permissions: &Value, key: &str) -> PermissionScope {
    let value = match permissions.get(key) {
        Some(value) => value,
        None => return PermissionScope::Missing,
    };

    match value {
        Value::String(text) => {
            if text.is_empty() {
                PermissionScope::Missing
            } else {
                PermissionScope::OrgWide
            }
        }
        Value::Array(items) => {
            let ids = items
                .iter()
                .filter_map(|item| item.as_str())
                .filter_map(|item| Uuid::parse_str(item).ok())
                .collect::<Vec<_>>();
            if ids.is_empty() {
                PermissionScope::Missing
            } else {
                PermissionScope::Scoped(ids)
            }
        }
        _ => PermissionScope::Missing,
    }
}

pub async fn load_all_department_ids(db: &DatabaseConnection) -> Result<Vec<Uuid>, AuthError> {
    departments::Entity::find()
        .select_only()
        .column(departments::Column::Id)
        .into_tuple::<Uuid>()
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("department list error: {e}");
            AuthError::DbTimeout
        })
}

pub async fn should_refresh_administered_departments(
    db: &DatabaseConnection,
    user_id: Uuid,
    effective_permissions: &Option<Value>,
) -> Result<bool, AuthError> {
    let empty = match effective_permissions {
        Some(Value::Object(map)) => map
            .get("administered_departments")
            .and_then(Value::as_array)
            .map(|items| items.is_empty())
            .unwrap_or(true),
        _ => true,
    };

    if !empty {
        return Ok(false);
    }

    let count = user_role_assignments::Entity::find()
        .select_only()
        .column(user_role_assignments::Column::Id)
        .join(
            JoinType::InnerJoin,
            user_role_assignments::Relation::Roles.def(),
        )
        .join(JoinType::InnerJoin, roles::Relation::RolePermissions.def())
        .join(
            JoinType::InnerJoin,
            role_permissions::Relation::Permissions.def(),
        )
        .filter(user_role_assignments::Column::UserId.eq(user_id))
        .filter(permissions::Column::Domain.eq("departments"))
        .filter(permissions::Column::Action.eq("manage"))
        .count(db)
        .await
        .map_err(|e| {
            eprintln!("administered departments check error: {e}");
            AuthError::DbTimeout
        })?;

    Ok(count > 0)
}

pub async fn load_administered_department_paths(
    app_state: &SharedState,
    department_ids: &[Uuid],
) -> Result<Vec<String>, AuthError> {
    if department_ids.is_empty() {
        return Ok(Vec::new());
    }

    let rows = departments::Entity::find()
        .select_only()
        .column(departments::Column::Id)
        .expr_as(Expr::cust("path::text"), "path")
        .filter(departments::Column::Id.is_in(department_ids.iter().copied()))
        .into_tuple::<(Uuid, String)>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("department paths lookup error: {e}");
            AuthError::DbTimeout
        })?;

    Ok(rows.into_iter().map(|(_, path)| path).collect())
}

pub fn scope_condition(scope_paths: &[String]) -> Condition {
    let mut cond = Condition::any();
    for path in scope_paths {
        cond = cond.add(Expr::col(departments::Column::Path).binary(
            BinOper::Custom("<@".into()),
            Expr::val(path.clone()).cast_as(Alias::new("ltree")),
        ));
    }
    cond
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manage_scope_takes_precedence_over_view_scope() {
        let managed = Uuid::new_v4();
        let viewed = Uuid::new_v4();
        let rows = vec![
            (Some(viewed), "view".to_string()),
            (Some(managed), "manage".to_string()),
        ];

        assert_eq!(
            preferred_department_scope(&rows),
            PermissionScope::Scoped(vec![managed])
        );
    }

    #[test]
    fn org_wide_scope_wins_within_the_selected_permission() {
        let rows = vec![
            (Some(Uuid::new_v4()), "manage".to_string()),
            (None, "manage".to_string()),
        ];

        assert_eq!(preferred_department_scope(&rows), PermissionScope::OrgWide);
    }

    #[test]
    fn view_scope_is_used_when_manage_permission_is_missing() {
        let viewed = Uuid::new_v4();
        let rows = vec![(Some(viewed), "view".to_string())];

        assert_eq!(
            preferred_department_scope(&rows),
            PermissionScope::Scoped(vec![viewed])
        );
    }
}
