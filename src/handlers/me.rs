use axum::{
    Json,
    extract::{Query, State},
};
use migration::extension::postgres::PgExpr;
use reqwest::StatusCode;
use sea_orm::sea_query::{Alias, BinOper, Expr, Func};
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, EntityTrait, JoinType, Order, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, RelationTrait,
};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
    },
    dto::{
        admin_department::{
            Department, DepartmentListQuery, DepartmentSortRule, DepartmentTree,
            DepartmentTreeNode, DepartmentTreeQuery, DepartmentsListResponse,
        },
        admin_user::User,
        analytics::{
            DepartmentAnalytics, DepartmentAnalyticsQuery, ScopedUserAnalyticsQuery, UserAnalytics,
        },
        common::SortRule,
        me::{
            AdministeredDepartmentUsersQuery, EffectivePermissionsResponse,
            MeDepartmentUsersResponse,
        },
    },
    handlers::admin_department::{
        ChildCountRow, DepartmentRow, DepartmentTreeRow, DeptCountRow, department_budget_snapshot,
        departments_base_select, departments_tree_select, load_department_admin_ids_map,
    },
    models::{
        departments, permissions, role_permissions, roles, user_role_assignments, users,
        users::UserStatus,
    },
    services::{
        analytics,
        authorization::{AuthorizationService, is_path_within_scope},
    },
    state::SharedState,
};

async fn load_administered_department_ids(
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

    let mut departments = Vec::new();
    if let Some(value) = user.effective_permissions {
        let permissions = value.get("permissions").unwrap_or(&Value::Null);
        let manage_scope = parse_permission_scope(permissions, "departments:manage");
        let view_scope = parse_permission_scope(permissions, "departments:view");

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
                        departments.push(id);
                    }
                }
            }
        }

        if departments.is_empty() {
            departments = match manage_scope {
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

    Ok(departments)
}

fn needs_effective_permissions_refresh(value: &Option<Value>) -> bool {
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

enum PermissionScope {
    Missing,
    OrgWide,
    Scoped(Vec<Uuid>),
}

fn parse_permission_scope(permissions: &Value, key: &str) -> PermissionScope {
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

async fn load_all_department_ids(db: &DatabaseConnection) -> Result<Vec<Uuid>, AuthError> {
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

async fn should_refresh_administered_departments(
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

async fn load_administered_department_paths(
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

fn scope_condition(scope_paths: &[String]) -> Condition {
    let mut cond = Condition::any();
    for path in scope_paths {
        cond = cond.add(Expr::col(departments::Column::Path).binary(
            BinOper::Custom("<@".into()),
            Expr::val(path.clone()).cast_as(Alias::new("ltree")),
        ));
    }
    cond
}

#[utoipa::path(
    get,
    path = "/me/permissions",
    tag = "me",
    responses(
        (status = 200, content_type = "application/json", body = EffectivePermissionsResponse),
        (status = 401, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error)
    )
)]
pub async fn get_my_permissions(
    claims: Claims,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<EffectivePermissionsResponse>), AuthError> {
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

    let mut default_effective = Map::new();
    default_effective.insert("permissions".to_string(), Value::Object(Map::new()));
    default_effective.insert("mcp_access".to_string(), Value::Object(Map::new()));
    default_effective.insert(
        "administered_departments".to_string(),
        Value::Array(Vec::new()),
    );

    let effective = user
        .effective_permissions
        .unwrap_or_else(|| Value::Object(default_effective));

    let permissions = effective
        .get("permissions")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let mcp_access = effective
        .get("mcp_access")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let administered_departments = effective
        .get("administered_departments")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    Ok((
        StatusCode::OK,
        Json(EffectivePermissionsResponse {
            permissions,
            mcp_access,
            administered_departments,
        }),
    ))
}

#[utoipa::path(
    get,
    path = "/me/analytics/administered-departments",
    tag = "me",
    params(
        ("start_date" = Option<String>, Query, description = "Start date (YYYY-MM-DD)"),
        ("end_date" = Option<String>, Query, description = "End date (YYYY-MM-DD)"),
        ("offset" = Option<u64>, Query, description = "Number of items to skip (default: 0)"),
        ("limit" = Option<u64>, Query, description = "Items per page (default: 20)"),
        ("search" = Option<String>, Query, description = "Search by department name"),
        ("department_id" = Option<Uuid>, Query, description = "Filter by department (must be within scope)"),
        ("sort" = Option<crate::dto::analytics::DepartmentAnalyticsSortRule>, Query, description = "Sort by name, created_at, updated_at, members, or sub_departments"),
        ("ascending" = Option<bool>, Query, description = "Sort ascending when true (default: false)"),
        ("live" = Option<bool>, Query, description = "Bypass cache and fetch live data"),
    ),
    responses(
        (status = 200, body = DepartmentAnalytics),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error)
    )
)]
pub async fn get_my_administered_department_analytics(
    claims: Claims,
    State(app_state): State<SharedState>,
    Query(query): Query<DepartmentAnalyticsQuery>,
) -> Result<(StatusCode, Json<DepartmentAnalytics>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            crate::auth::permissions::PERMISSION_ANALYTICS_VIEW,
            None,
            crate::services::authorization::PermissionScopeMode::AllowAnyScope,
            None,
        )
        .await?;

    let administered_ids = load_administered_department_ids(claims, &app_state).await?;
    let scope_paths = load_administered_department_paths(&app_state, &administered_ids).await?;

    let result =
        analytics::get_department_analytics_scoped(&app_state.database, query, &scope_paths)
            .await
            .map_err(|e| {
                eprintln!("Administered department analytics error: {e}");
                AuthError::DbTimeout
            })?;

    Ok((StatusCode::OK, Json(result)))
}

#[utoipa::path(
    get,
    path = "/me/analytics/administered-departments/users",
    tag = "me",
    params(
        ("start_date" = Option<String>, Query, description = "Start date (YYYY-MM-DD)"),
        ("end_date" = Option<String>, Query, description = "End date (YYYY-MM-DD)"),
        ("page" = Option<u64>, Query, description = "Page number (default: 0)"),
        ("limit" = Option<u64>, Query, description = "Items per page (default: 20)"),
        ("sort_by" = Option<String>, Query, description = "Sort field by name,email,totalRequests,totalTokens,totalCost,averageLatency,lastActivity"),
        ("order" = Option<String>, Query, description = "Sort order (asc/desc)"),
        ("search" = Option<String>, Query, description = "Search by name,email or department"),
        ("status" = Option<UserStatus>, Query, description = "Account status"),
        ("role_id" = Option<Uuid>, Query, description = "Filter by RBAC role id"),
        ("department_id" = Option<Uuid>, Query, description = "Filter by department (must be within scope)"),
    ),
    responses(
        (status = 200, body = UserAnalytics),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error)
    )
)]
pub async fn get_my_administered_department_user_analytics(
    claims: Claims,
    State(app_state): State<SharedState>,
    Query(query): Query<ScopedUserAnalyticsQuery>,
) -> Result<(StatusCode, Json<UserAnalytics>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            crate::auth::permissions::PERMISSION_ANALYTICS_VIEW,
            None,
            crate::services::authorization::PermissionScopeMode::AllowAnyScope,
            None,
        )
        .await?;

    let administered_ids = load_administered_department_ids(claims, &app_state).await?;
    let scope_paths = load_administered_department_paths(&app_state, &administered_ids).await?;

    let page = query.page.unwrap_or(0);
    let limit = query.limit.unwrap_or(20);

    let result = analytics::calculate_user_analytics_scoped(
        &app_state.database,
        query,
        page,
        limit,
        &scope_paths,
    )
    .await
    .map_err(|e| {
        eprintln!("Administered user analytics error: {e}");
        AuthError::DbTimeout
    })?;

    Ok((StatusCode::OK, Json(result)))
}

#[utoipa::path(
    get,
    path = "/me/administered-departments",
    tag = "me",
    params(
        ("parent_id" = Option<String>, Query, description = "Filter by parent department id (use \"root\" for scope roots)"),
        ("include_children" = Option<bool>, Query, description = "Include descendant departments when parent_id is set (default: false)"),
        ("search" = Option<String>, Query, description = "Search by department name"),
        ("sort" = Option<DepartmentSortRule>, Query, description = "Sort by name, created_at, updated_at, members, or sub_departments"),
        ("ascending" = Option<bool>, Query, description = "Sort ascending when true (default: false)"),
    ),
    responses(
        (status = 200, body = DepartmentsListResponse),
        (status = 401, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error)
    )
)]
pub async fn get_my_administered_departments_list(
    claims: Claims,
    State(app_state): State<SharedState>,
    Query(q): Query<DepartmentListQuery>,
) -> Result<(StatusCode, Json<DepartmentsListResponse>), AuthError> {
    let administered_ids = load_administered_department_ids(claims, &app_state).await?;
    if administered_ids.is_empty() {
        return Ok((
            StatusCode::OK,
            Json(DepartmentsListResponse {
                departments: vec![],
                total: 0,
            }),
        ));
    }

    let scope_paths = load_administered_department_paths(&app_state, &administered_ids).await?;
    if scope_paths.is_empty() {
        return Ok((
            StatusCode::OK,
            Json(DepartmentsListResponse {
                departments: vec![],
                total: 0,
            }),
        ));
    }

    let mut query = departments_base_select();
    query = query.filter(scope_condition(&scope_paths));

    if let Some(parent) = q.parent_id.as_deref() {
        if parent.eq_ignore_ascii_case("root") {
            let mut root_condition = Condition::any();
            root_condition = root_condition.add(departments::Column::ParentId.is_null());
            root_condition =
                root_condition.add(departments::Column::Id.is_in(administered_ids.clone()));
            query = query.filter(root_condition);
        } else {
            let parent_uuid =
                Uuid::parse_str(parent).map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;

            let parent_path = departments::Entity::find_by_id(parent_uuid)
                .select_only()
                .expr_as(Expr::cust("path::text"), "path")
                .into_tuple::<String>()
                .one(&app_state.database)
                .await
                .map_err(|e| {
                    eprintln!("db error: {e}");
                    AuthError::DbTimeout
                })?
                .ok_or(AuthError::DbNotFound)?;

            let in_scope = scope_paths
                .iter()
                .any(|scope| is_path_within_scope(scope, &parent_path));
            if !in_scope {
                return Ok((
                    StatusCode::OK,
                    Json(DepartmentsListResponse {
                        departments: vec![],
                        total: 0,
                    }),
                ));
            }

            if q.include_children {
                query = query.filter(Expr::col(departments::Column::Path).binary(
                    BinOper::Custom("<@".into()),
                    Expr::val(parent_path).cast_as(Alias::new("ltree")),
                ));
            } else {
                query = query.filter(departments::Column::ParentId.eq(parent_uuid));
            }
        }
    }

    if let Some(search) = q.search.as_deref() {
        query = query.filter(
            departments::Column::Name
                .into_expr()
                .ilike(format!("%{}%", search)),
        );
    }

    query = query.order_by_asc(departments::Column::Name);

    let rows = query
        .into_model::<DepartmentRow>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error: {e}");
            AuthError::DbTimeout
        })?;

    let total = rows.len() as i64;
    if rows.is_empty() {
        return Ok((
            StatusCode::OK,
            Json(DepartmentsListResponse {
                departments: vec![],
                total,
            }),
        ));
    }

    let dept_ids: Vec<Uuid> = rows.iter().map(|d| d.id).collect();

    let direct_counts: Vec<DeptCountRow> = users::Entity::find()
        .select_only()
        .column(users::Column::DepartmentId)
        .expr_as(Func::count(Expr::col(users::Column::Id)), "cnt")
        .filter(users::Column::DepartmentId.is_in(dept_ids.clone()))
        .filter(users::Column::Status.ne(users::UserStatus::Deleted))
        .group_by(users::Column::DepartmentId)
        .into_model::<DeptCountRow>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("direct member count error: {e}");
            AuthError::DbTimeout
        })?;

    let direct_map: HashMap<Uuid, i64> = direct_counts
        .into_iter()
        .map(|r| (r.department_id, r.cnt))
        .collect();

    let child_counts: Vec<ChildCountRow> = departments::Entity::find()
        .select_only()
        .column(departments::Column::ParentId)
        .expr_as(Func::count(Expr::col(departments::Column::Id)), "cnt")
        .filter(departments::Column::ParentId.is_in(dept_ids.clone()))
        .group_by(departments::Column::ParentId)
        .into_model::<ChildCountRow>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("child count error: {e}");
            AuthError::DbTimeout
        })?;

    let child_map: HashMap<Uuid, i64> = child_counts
        .into_iter()
        .map(|r| (r.parent_id, r.cnt))
        .collect();

    let admin_map = load_department_admin_ids_map(&app_state.database, &dept_ids).await?;

    let mut departments = Vec::with_capacity(rows.len());
    for d in rows {
        let member_count = *direct_map.get(&d.id).unwrap_or(&0) as i32;
        let child_count = *child_map.get(&d.id).unwrap_or(&0) as i32;
        let (budget_distributed, budget_available, budget_used) = department_budget_snapshot(
            &app_state.database,
            d.id,
            d.budget_allocated,
            d.budget_period,
        )
        .await?;
        let admin_ids = admin_map.get(&d.id).cloned().unwrap_or_default();

        departments.push(Department {
            id: d.id,
            name: d.name,
            description: d.description,
            parent_id: d.parent_id,
            path: d.path,
            depth: d.depth,
            admin_ids,
            member_count,
            total_member_count: 0,
            child_count,
            budget_allocated: d.budget_allocated.to_string().parse().unwrap_or(0.0),
            budget_distributed,
            budget_available,
            budget_used,
            budget_period: d.budget_period,
            retention_days: d.retention_days,
            allowed_models: Vec::new(),
            created_at: d.created_at,
            updated_at: d.updated_at,
        });
    }

    let sort_rule = q.sort.unwrap_or(DepartmentSortRule::Name);
    let ascending = q.ascending.unwrap_or(false);
    departments.sort_by(|a, b| {
        let ordering = match sort_rule {
            DepartmentSortRule::Name => a.name.cmp(&b.name),
            DepartmentSortRule::CreatedAt => a.created_at.cmp(&b.created_at),
            DepartmentSortRule::UpdatedAt => a.updated_at.cmp(&b.updated_at),
            DepartmentSortRule::Members => a.member_count.cmp(&b.member_count),
            DepartmentSortRule::SubDepartments => a.child_count.cmp(&b.child_count),
        };
        if ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });

    Ok((
        StatusCode::OK,
        Json(DepartmentsListResponse { departments, total }),
    ))
}

#[utoipa::path(
    get,
    path = "/me/administered-departments/tree",
    tag = "me",
    params(
        ("root_id" = Option<Uuid>, Query, description = "Start from this department (default: scope roots)"),
        ("max_depth" = Option<i32>, Query, description = "Default value : 10")
    ),
    responses(
        (status = 200, body = DepartmentTree),
        (status = 401, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error)
    )
)]
pub async fn get_my_administered_departments_tree(
    claims: Claims,
    State(app_state): State<SharedState>,
    Query(q): Query<DepartmentTreeQuery>,
) -> Result<(StatusCode, Json<DepartmentTree>), AuthError> {
    let administered_ids = load_administered_department_ids(claims, &app_state).await?;
    if administered_ids.is_empty() {
        return Ok((StatusCode::OK, Json(DepartmentTree { tree: vec![] })));
    }

    let scope_paths = load_administered_department_paths(&app_state, &administered_ids).await?;
    if scope_paths.is_empty() {
        return Ok((StatusCode::OK, Json(DepartmentTree { tree: vec![] })));
    }

    let max_depth = q.max_depth.unwrap_or(10).max(0);

    let root = if let Some(root_id) = q.root_id {
        let root_dept = departments_base_select()
            .filter(departments::Column::Id.eq(root_id))
            .into_model::<DepartmentRow>()
            .one(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("db error: {e}");
                AuthError::DbTimeout
            })?
            .ok_or(AuthError::DbNotFound)?;

        let in_scope = scope_paths
            .iter()
            .any(|scope| is_path_within_scope(scope, &root_dept.path));
        if !in_scope {
            return Ok((StatusCode::OK, Json(DepartmentTree { tree: vec![] })));
        }

        Some(root_dept)
    } else {
        None
    };

    let mut query = departments_tree_select();
    query = query.filter(scope_condition(&scope_paths));

    if let Some(root_dept) = &root {
        query = query.filter(Expr::col(departments::Column::Path).binary(
            BinOper::Custom("<@".into()),
            Expr::val(root_dept.path.clone()).cast_as(Alias::new("ltree")),
        ));
        query = query.filter(departments::Column::Depth.lte(root_dept.depth + max_depth));
    } else {
        query = query.filter(departments::Column::Depth.lte(max_depth));
    }

    let rows = query
        .into_model::<DepartmentTreeRow>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error: {e}");
            AuthError::DbTimeout
        })?;

    if rows.is_empty() {
        return Ok((StatusCode::OK, Json(DepartmentTree { tree: vec![] })));
    }

    let dept_ids: Vec<Uuid> = rows.iter().map(|d| d.id).collect();

    let direct_counts: Vec<DeptCountRow> = users::Entity::find()
        .select_only()
        .column(users::Column::DepartmentId)
        .expr_as(Func::count(Expr::col(users::Column::Id)), "cnt")
        .filter(users::Column::DepartmentId.is_in(dept_ids.clone()))
        .filter(users::Column::Status.ne(users::UserStatus::Deleted))
        .group_by(users::Column::DepartmentId)
        .into_model::<DeptCountRow>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("direct member count error: {e}");
            AuthError::DbTimeout
        })?;

    let direct_map: HashMap<Uuid, i64> = direct_counts
        .into_iter()
        .map(|r| (r.department_id, r.cnt))
        .collect();

    let admin_map = load_department_admin_ids_map(&app_state.database, &dept_ids).await?;

    let mut nodes: HashMap<Uuid, DepartmentTreeNode> = HashMap::with_capacity(rows.len());
    let mut children_map: HashMap<Uuid, Vec<Uuid>> = HashMap::with_capacity(rows.len());

    for d in rows {
        let member_count = *direct_map.get(&d.id).unwrap_or(&0) as i32;
        let admin_ids = admin_map.get(&d.id).cloned().unwrap_or_default();

        if let Some(pid) = d.parent_id {
            children_map.entry(pid).or_default().push(d.id);
        }

        nodes.insert(
            d.id,
            DepartmentTreeNode {
                id: d.id,
                name: d.name,
                description: d.description,
                parent_id: d.parent_id,
                path: d.path,
                depth: d.depth,
                admin_ids,
                member_count,
                total_member_count: member_count,
                child_count: 0,
                budget_allocated: d.budget_allocated.to_string().parse().unwrap_or(0.0),
                budget_distributed: 0.0,
                budget_available: 0.0,
                budget_used: 0.0,
                budget_period: d.budget_period,
                retention_days: d.retention_days,
                allowed_models: Vec::new(),
                created_at: d.created_at,
                updated_at: d.updated_at,
                children: vec![],
            },
        );
    }

    let root_ids: Vec<Uuid> = if let Some(root_dept) = &root {
        vec![root_dept.id]
    } else {
        nodes
            .values()
            .filter_map(|n| match n.parent_id {
                None => Some(n.id),
                Some(pid) => (!nodes.contains_key(&pid)).then_some(n.id),
            })
            .collect()
    };

    fn attach(
        id: Uuid,
        nodes: &mut HashMap<Uuid, DepartmentTreeNode>,
        children_map: &mut HashMap<Uuid, Vec<Uuid>>,
        visiting: &mut HashSet<Uuid>,
    ) -> Option<DepartmentTreeNode> {
        if !visiting.insert(id) {
            eprintln!("cycle detected while building department tree at: {id}");
            return None;
        }
        let mut node = nodes.remove(&id)?;
        let child_ids = children_map.remove(&id).unwrap_or_default();

        let mut total = node.member_count;
        let mut children = Vec::with_capacity(child_ids.len());

        for cid in child_ids {
            if let Some(child) = attach(cid, nodes, children_map, visiting) {
                total += child.total_member_count;
                children.push(child);
            }
        }

        node.child_count = children.len() as i32;
        node.total_member_count = total;
        node.children = children;
        visiting.remove(&id);
        Some(node)
    }

    let mut tree = Vec::with_capacity(root_ids.len());
    let mut visiting = HashSet::with_capacity(root_ids.len());
    for id in root_ids {
        if let Some(node) = attach(id, &mut nodes, &mut children_map, &mut visiting) {
            tree.push(node);
        }
    }

    Ok((StatusCode::OK, Json(DepartmentTree { tree })))
}

#[utoipa::path(
    get,
    path = "/me/administered-departments/users",
    tag = "me",
    params(
        ("department_id" = Option<Uuid>, Query, description = "Department id (must be within scope). If omitted, lists users across all administered departments."),
        ("include_sub_department" = Option<bool>, Query, description = "Include users from sub-departments"),
        ("limit" = Option<u64>, Query, description = "Default value : 20"),
        ("offset" = Option<u64>, Query, description = "Default value : 0"),
        ("search" = Option<String>, Query, description = "Search by name,email,department"),
        ("status" = Option<UserStatus>, Query, description = "Account status"),
        ("role_id" = Option<Uuid>, Query, description = "Filter by RBAC role id"),
        ("order" = Option<String>, Query, description = "Sort order (asc/desc)"),
        ("sort" = Option<SortRule>, Query, description = "Sort by name,email,created_at,updated_at,last_login_at"),
    ),
    responses(
       (status = 200, body = MeDepartmentUsersResponse),
       (status = 401, content_type = "application/json", body = Error),
       (status = 404, content_type = "application/json", body = Error),
       (status = 503, content_type = "application/json", body = Error)
    )
)]
pub async fn get_my_administered_department_members(
    claims: Claims,
    State(app_state): State<SharedState>,
    Query(query): Query<AdministeredDepartmentUsersQuery>,
) -> Result<(StatusCode, Json<MeDepartmentUsersResponse>), AuthError> {
    let include_sub_department = query.include_sub_department.unwrap_or(false);
    let department_id = query.department_id;
    let limit = query.limit.unwrap_or(30).max(1);
    let offset = query.offset.unwrap_or(0);
    let page = offset / limit;
    let mut response = MeDepartmentUsersResponse {
        total: 0,
        users: Vec::new(),
    };

    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            crate::auth::permissions::PERMISSION_USERS_VIEW,
            None,
            crate::services::authorization::PermissionScopeMode::AllowAnyScope,
            None,
        )
        .await?;

    let administered_ids = load_administered_department_ids(claims, &app_state).await?;
    if administered_ids.is_empty() {
        return Ok((StatusCode::OK, Json(response)));
    }

    let scope_paths = load_administered_department_paths(&app_state, &administered_ids).await?;
    if scope_paths.is_empty() {
        return Ok((StatusCode::OK, Json(response)));
    }

    let mut select = users::Entity::find()
        .filter(users::Column::Status.ne(UserStatus::Deleted))
        .join(JoinType::LeftJoin, users::Relation::Departments.def());

    if let Some(department_id) = department_id {
        let root = departments_base_select()
            .filter(departments::Column::Id.eq(department_id))
            .into_model::<DepartmentRow>()
            .one(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("db error: {e}");
                AuthError::DbTimeout
            })?
            .ok_or(AuthError::DbNotFound)?;

        let in_scope = scope_paths
            .iter()
            .any(|scope| is_path_within_scope(scope, &root.path));
        if !in_scope {
            return Ok((StatusCode::OK, Json(response)));
        }

        if include_sub_department {
            let subtree_dept_ids: Vec<Uuid> = departments::Entity::find()
                .select_only()
                .column(departments::Column::Id)
                .filter(Expr::col(departments::Column::Path).binary(
                    BinOper::Custom("<@".into()),
                    Expr::val(root.path.clone()).cast_as(Alias::new("ltree")),
                ))
                .into_tuple()
                .all(&app_state.database)
                .await
                .map_err(|e| {
                    eprintln!("db error: {e}");
                    AuthError::DbTimeout
                })?
                .into_iter()
                .map(|(id,)| id)
                .collect();

            select = select.filter(users::Column::DepartmentId.is_in(subtree_dept_ids));
        } else {
            select = select.filter(users::Column::DepartmentId.eq(department_id));
        }
    } else {
        select = select.filter(scope_condition(&scope_paths));
    }

    if let Some(role_id) = query.role_id {
        let role_user_ids: Vec<Uuid> = user_role_assignments::Entity::find()
            .select_only()
            .column(user_role_assignments::Column::UserId)
            .filter(user_role_assignments::Column::RoleId.eq(role_id))
            .into_tuple::<Uuid>()
            .all(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("role filter lookup error: {e}");
                AuthError::DbTimeout
            })?;
        if role_user_ids.is_empty() {
            return Ok((StatusCode::OK, Json(response)));
        }
        select = select.filter(users::Column::Id.is_in(role_user_ids));
    }

    if let Some(status) = query.status {
        select = select.filter(users::Column::Status.eq(status));
    }

    let order = query.order.as_deref().unwrap_or("desc");
    let ord = if order.eq_ignore_ascii_case("asc") {
        Order::Asc
    } else {
        Order::Desc
    };
    if let Some(sort) = query.sort {
        select = match sort {
            SortRule::Name => select.order_by(users::Column::Name, ord),
            SortRule::Email => select.order_by(users::Column::Email, ord),
            SortRule::CreatedAt => select.order_by(users::Column::CreatedAt, ord),
            SortRule::UpdatedAt => select.order_by(users::Column::UpdatedAt, ord),
            SortRule::LastLoginAt => select.order_by(users::Column::LastLoginAt, ord),
            _ => select.order_by(users::Column::CreatedAt, ord),
        };
    }

    if let Some(search) = &query.search {
        select = select.filter(
            Condition::any()
                .add(
                    users::Column::Name
                        .into_expr()
                        .ilike(format!("%{}%", search)),
                )
                .add(
                    users::Column::Email
                        .into_expr()
                        .ilike(format!("%{}%", search)),
                )
                .add(
                    departments::Column::Name
                        .into_expr()
                        .ilike(format!("%{}%", search)),
                ),
        );
    }

    let paginator = select.paginate(&app_state.database, limit);
    response.total = paginator.num_items().await.map_err(|e| {
        eprintln!("db get many error: {}", e);
        AuthError::DbTimeout
    })? as i32;

    let users_row = paginator.fetch_page(page).await.map_err(|e| {
        eprintln!("db get many error: {}", e);
        AuthError::DbTimeout
    })?;

    let user_ids: Vec<Uuid> = users_row.iter().map(|u| u.id).collect();
    let roles_map = authz.user_roles_map(&user_ids).await?;
    response.users = users_row
        .into_iter()
        .map(|user| {
            let roles = roles_map.get(&user.id).cloned().unwrap_or_default();
            let is_super_admin = roles.iter().any(|r| r == "Super Admin");
            User {
                id: user.id,
                sub: user
                    .azure_id
                    .unwrap_or(user.google_id.unwrap_or(user.email.clone())),
                email: user.email,
                name: user.name,
                picture: user.picture,
                hd: user.hd,
                roles,
                status: user.status,
                department: None,
                department_id: user.department_id,
                is_super_admin,
                has_password: user.password.is_some(),
                mfa_enabled: user.mfa_enabled,
                last_login_at: Some(user.last_login_at),
                password_changed_at: user.password_changed_at,
                created_at: user.created_at,
                updated_at: user.updated_at,
                effective_permissions: user.effective_permissions,
            }
        })
        .collect();
    Ok((StatusCode::OK, Json(response)))
}
