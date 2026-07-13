use crate::{
    auth::{claims::Claims, error::AuthError},
    dto::me::PermissionScope,
    models::{departments, permissions, role_permissions, roles, user_role_assignments, users},
    services::authorization::AuthorizationService,
    state::SharedState,
};
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, EntityTrait, JoinType, PaginatorTrait, QueryFilter,
    QuerySelect, RelationTrait,
};
use sea_orm::sea_query::{Alias, BinOper, Expr};
use serde_json::Value;
use uuid::Uuid;

pub async fn load_administered_department_ids(
    claims: Claims,
    app_state: &SharedState,
) -> Result<Vec<Uuid>, AuthError> {
    let authz = AuthorizationService::new(&app_state.database);

    let mut user = users::Entity::find_by_id(claims.user_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("user lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    let mut needs_refresh = needs_effective_permissions_refresh(&user.effective_permissions);
    if !needs_refresh
        && should_refresh_administered_departments(
            &app_state.database,
            user.id,
            &user.effective_permissions,
        )
        .await?
    {
        needs_refresh = true;
    }

    if needs_refresh {
        authz.recompute_effective_permissions(user.id).await?;
        user = users::Entity::find_by_id(claims.user_id)
            .one(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("user lookup error: {e}");
                AuthError::DbTimeout
            })?
            .ok_or(AuthError::ResourceNotFound)?;
    }

    let mut dept_ids = Vec::new();
    if let Some(value) = user.effective_permissions {
        let permissions_val = value.get("permissions").unwrap_or(&Value::Null);
        let manage_scope = parse_permission_scope(permissions_val, "departments:manage");
        let view_scope = parse_permission_scope(permissions_val, "departments:view");

        if matches!(manage_scope, PermissionScope::Missing)
            && matches!(view_scope, PermissionScope::Missing)
        {
            return Ok(Vec::new());
        }

        if let Some(list) = value
            .get("administered_departments")
            .and_then(|v| v.as_array())
        {
            for item in list {
                if let Some(id_str) = item.as_str() {
                    if let Ok(id) = Uuid::parse_str(id_str) {
                        dept_ids.push(id);
                    }
                }
            }
        }

        if dept_ids.is_empty() {
            dept_ids = match manage_scope {
                PermissionScope::OrgWide => load_all_department_ids(&app_state.database).await?,
                PermissionScope::Scoped(ids) if !ids.is_empty() => ids,
                _ => match view_scope {
                    PermissionScope::OrgWide => {
                        load_all_department_ids(&app_state.database).await?
                    }
                    PermissionScope::Scoped(ids) => ids,
                    PermissionScope::Missing => Vec::new(),
                },
            };
        }
    }

    Ok(dept_ids)
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
