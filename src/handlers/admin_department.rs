use std::collections::{HashMap, HashSet};
use axum::{Json, extract::{Path, Query, State}};
use chrono::{DateTime, Utc};
use migration::{Alias, BinOper, Func, SimpleExpr, extension::postgres::PgExpr};
use rust_decimal::Decimal;
use sea_orm::{ActiveModelTrait, Condition, DatabaseConnection, EntityName as _, FromQueryResult, JoinType, Order, PaginatorTrait, QueryOrder, QuerySelect, RelationTrait, sea_query::{Expr, PostgresQueryBuilder,Query as SqlQuery}};
use reqwest::StatusCode;
use sea_orm::{ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait as _, QueryFilter, Statement, sqlx::postgres::types::{PgLTree, PgLTreeLabel}};
use uuid::Uuid;
use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, AuthErrorResponse},
        permissions::{
            PERMISSION_DEPARTMENTS_MANAGE, PERMISSION_DEPARTMENTS_VIEW, PERMISSION_ROLES_ASSIGN,
            PERMISSION_USERS_VIEW, ROLE_DEPARTMENT_ADMIN,
        },
    },
    dto::{
        admin_department::{
            DepartmentListQuery, DepartmentMembersResponse, DepartmentMemeberListQuery,
            DepartmentRequest, DepartmentResponse, DepartmentTreeNode, DepartmentTreeQuery,
            DepartmentTreeResponse, DepartmentUpdateRequest, DepartmentsListResponse,
            MoveDepartmentRequest, RoleAssignmentPayload,
        },
        admin_user::UserDetails,
        common::SortRule,
    },
    models::{
        departments::{self, ActionOnExceed, BudgetPeriod},
        roles,
        user_role_assignments,
        users::{self, UserStatus},
    },
    services::{
        authorization::{AuthorizationService, PermissionScopeMode},
        auth_audit::{build_audit_payload, record_auth_event},
        budget_allocation::{period_bounds, sum_child_allocations, sum_department_cost_in_range},
    },
    state::SharedState,
};

#[derive(Debug, Clone, FromQueryResult)]
pub struct DepartmentRow {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    #[sea_orm(from_alias = "parentId")]
    pub parent_id: Option<Uuid>,
    pub depth: i32,
    pub path: String, // comes from path::text alias
    #[sea_orm(from_alias = "budgetAllocated")]
    pub budget_allocated: Decimal,
    #[sea_orm(from_alias = "budgetPeriod")]
    pub budget_period: BudgetPeriod,
    #[sea_orm(from_alias = "actionOnExceed")]
    pub action_on_exceed: ActionOnExceed,
    #[sea_orm(from_alias = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[sea_orm(from_alias = "updatedAt")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromQueryResult)]
pub struct DepartmentTreeRow {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    #[sea_orm(from_alias = "parentId")]
    pub parent_id: Option<Uuid>,
    pub depth: i32,
    pub path: String,
    #[sea_orm(from_alias = "budgetAllocated")]
    pub budget_allocated: Decimal,
    #[sea_orm(from_alias = "budgetPeriod")]
    pub budget_period: BudgetPeriod,
    #[sea_orm(from_alias = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[sea_orm(from_alias = "updatedAt")]
    pub updated_at: DateTime<Utc>,
}

fn ltree_label_from_uuid(id: uuid::Uuid) -> String {
    id.simple().to_string()
}

fn build_ltree_path(parent_path: Option<&str>, id: uuid::Uuid) -> Result<String, AuthError> {
    let mut tree = if let Some(p) = parent_path {
        // parse existing ltree string into PgLTree (validates overall format)
        p.parse::<PgLTree>().map_err(|_| AuthError::ServiceTemporarilyUnavailable)?
    } else {
        PgLTree::new()
    };

    let label_str = ltree_label_from_uuid(id);
    let label = PgLTreeLabel::new(label_str).map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    tree.push(label);

    Ok(tree.to_string())
}

async fn sync_department_admin_assignments(
    authz: &AuthorizationService<'_>,
    db: &DatabaseConnection,
    actor_id: Uuid,
    department_id: Uuid,
    department_admin_ids: &[Uuid],
    permission_scope: Option<Uuid>,
) -> Result<(), AuthError> {
    authz
        .ensure_permission(
            actor_id,
            PERMISSION_ROLES_ASSIGN,
            permission_scope,
            PermissionScopeMode::RequireOrgWide,
            Some(department_id),
        )
        .await?;

    let role = roles::Entity::find()
        .filter(roles::Column::Name.eq(ROLE_DEPARTMENT_ADMIN))
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("role lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    let desired_ids: HashSet<Uuid> = department_admin_ids.iter().copied().collect();

    if !desired_ids.is_empty() {
        let existing_users = users::Entity::find()
            .select_only()
            .column(users::Column::Id)
            .filter(users::Column::Id.is_in(desired_ids.iter().copied()))
            .into_tuple::<Uuid>()
            .all(db)
            .await
            .map_err(|e| {
                eprintln!("user lookup error: {e}");
                AuthError::DbTimeout
            })?;

        if existing_users.len() != desired_ids.len() {
            return Err(AuthError::ResourceNotFound);
        }
    }

    let existing_assignments = user_role_assignments::Entity::find()
        .filter(user_role_assignments::Column::RoleId.eq(role.id))
        .filter(user_role_assignments::Column::ScopeDepartmentId.eq(department_id))
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("role assignment lookup error: {e}");
            AuthError::DbTimeout
        })?;

    let existing_ids: HashSet<Uuid> = existing_assignments
        .iter()
        .map(|assignment| assignment.user_id)
        .collect();

    let to_add: Vec<Uuid> = desired_ids.difference(&existing_ids).copied().collect();
    let to_remove: Vec<user_role_assignments::Model> = existing_assignments
        .into_iter()
        .filter(|assignment| !desired_ids.contains(&assignment.user_id))
        .collect();

    let now = Utc::now();
    let mut affected_users: HashSet<Uuid> = HashSet::new();

    for user_id in to_add {
        let assignment_id = Uuid::new_v4();
        let assignment = user_role_assignments::ActiveModel {
            id: Set(assignment_id),
            user_id: Set(user_id),
            role_id: Set(role.id),
            scope_department_id: Set(Some(department_id)),
            assigned_by: Set(actor_id),
            created_at: Set(now),
            updated_at: Set(now),
        };

        assignment
            .insert(db)
            .await
            .map_err(|e| {
                let s = e.to_string();
                if s.contains("duplicate key value violates unique constraint") {
                    AuthError::DbConflict
                } else {
                    eprintln!("role assignment insert error: {e}");
                    AuthError::DbTimeout
                }
            })?;

        if let Some(payload) = build_audit_payload(RoleAssignmentPayload {
            assignment_id,
            user_id,
            role_id: role.id,
            scope_department_id: Some(department_id),
        }) {
            let _ = record_auth_event(
                db,
                "auth.role_assigned",
                Some(actor_id),
                payload,
            )
            .await;
        }

        affected_users.insert(user_id);
    }

    for assignment in to_remove {
        let assignment_id = assignment.id;
        let user_id = assignment.user_id;

        user_role_assignments::Entity::delete_by_id(assignment_id)
            .exec(db)
            .await
            .map_err(|e| {
                eprintln!("role assignment delete error: {e}");
                AuthError::DbTimeout
            })?;

        if let Some(payload) = build_audit_payload(RoleAssignmentPayload {
            assignment_id,
            user_id,
            role_id: role.id,
            scope_department_id: Some(department_id),
        }) {
            let _ = record_auth_event(
                db,
                "auth.role_unassigned",
                Some(actor_id),
                payload,
            )
            .await;
        }

        affected_users.insert(user_id);
    }

    if !affected_users.is_empty() {
        let affected: Vec<Uuid> = affected_users.into_iter().collect();
        let _ = authz.recompute_effective_permissions_for_users(&affected).await;
    }

    Ok(())
}

async fn load_department_admin_ids_map(
    db: &DatabaseConnection,
    department_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<Uuid>>, AuthError> {
    if department_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let role = roles::Entity::find()
        .filter(roles::Column::Name.eq(ROLE_DEPARTMENT_ADMIN))
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("role lookup error: {e}");
            AuthError::DbTimeout
        })?;

    let role = match role {
        Some(role) => role,
        None => return Ok(HashMap::new()),
    };

    let rows = user_role_assignments::Entity::find()
        .select_only()
        .column(user_role_assignments::Column::ScopeDepartmentId)
        .column(user_role_assignments::Column::UserId)
        .filter(user_role_assignments::Column::RoleId.eq(role.id))
        .filter(user_role_assignments::Column::ScopeDepartmentId.is_in(department_ids.iter().copied()))
        .into_tuple::<(Option<Uuid>, Uuid)>()
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("role assignment lookup error: {e}");
            AuthError::DbTimeout
        })?;

    let mut map: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (scope_id, user_id) in rows {
        if let Some(dept_id) = scope_id {
            map.entry(dept_id).or_default().push(user_id);
        }
    }

    Ok(map)
}


// Selects all columns but casts path -> text so decoding works
pub fn departments_base_select() -> sea_orm::Select<departments::Entity> {
    departments::Entity::find()
        .select_only()
        .column(departments::Column::Id)
        .column(departments::Column::Name)
        .column(departments::Column::Description)
        .column(departments::Column::ParentId)
        .column(departments::Column::Depth)
        .expr_as(Expr::cust("path::text"), "path")
        .column(departments::Column::BudgetAllocated)
        .column(departments::Column::BudgetPeriod)
        .column(departments::Column::ActionOnExceed)
        .column(departments::Column::CreatedAt)
        .column(departments::Column::UpdatedAt)
}

fn departments_tree_select() -> sea_orm::Select<departments::Entity> {
    departments::Entity::find()
        .select_only()
        .column(departments::Column::Id)
        .column(departments::Column::Name)
        .column(departments::Column::Description)
        .column(departments::Column::ParentId)
        .column(departments::Column::Depth)
        .expr_as(Expr::cust("path::text"), "path")
        .column(departments::Column::BudgetAllocated)
        .column(departments::Column::BudgetPeriod)
        .column(departments::Column::CreatedAt)
        .column(departments::Column::UpdatedAt)
}

async fn department_budget_snapshot(
    db: &sea_orm::DatabaseConnection,
    department_id: Uuid,
    budget_allocated: Decimal,
    budget_period: BudgetPeriod,
) -> Result<(f64, f64, f64), AuthError> {
    let budget_distributed = sum_child_allocations(db, department_id, None)
        .await
        .map_err(|e| {
            eprintln!("sum_child_allocations error: {e}");
            AuthError::DbTimeout
        })?;

    let now = Utc::now();
    let (period_start, period_end) = period_bounds(&budget_period, now);
    let budget_used = sum_department_cost_in_range(db, department_id, period_start, period_end)
        .await
        .map_err(|e| {
            eprintln!("sum_department_cost_in_range error: {e}");
            AuthError::DbTimeout
        })?;

    let budget_available = (budget_allocated - budget_distributed - budget_used).max(Decimal::ZERO);

    Ok((
        budget_distributed.to_string().parse().unwrap_or(0.0),
        budget_available.to_string().parse().unwrap_or(0.0),
        budget_used.to_string().parse().unwrap_or(0.0),
    ))
}

#[utoipa::path(
    post,
    path = "/admin/departments",
    tag = "admin",
    request_body = DepartmentRequest,
    responses(
       (status = 201, body = DepartmentResponse),
       (status = 401, content_type = "application/json", body = AuthErrorResponse),
       (status = 404, content_type = "application/json", body = AuthErrorResponse, description = "Parent department not found"),
       (status = 503, content_type = "application/json", body = AuthErrorResponse),
    )
)]
pub async fn create_department(
    claims: Claims,
    State(app_state): State<SharedState>,
    Json(req): Json<DepartmentRequest>,
) -> Result<(StatusCode,Json<DepartmentResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_DEPARTMENTS_MANAGE,
            req.parent_id,
            PermissionScopeMode::RequireOrgWide,
            req.parent_id,
        )
        .await?;

    let id = Uuid::new_v4();
    let created_at = Utc::now();
    let updated_at = created_at;

    // Fetch parent (if any) to compute path/depth
    let (parent_path, depth) = if let Some(parent_id) = req.parent_id {
        let parent = departments_base_select()
            .filter(departments::Column::Id.eq(parent_id))
            .into_model::<DepartmentRow>()
            .one(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("db error: {e}");
                AuthError::DbTimeout
            })?
            .ok_or(AuthError::DbNotFound)?;

        (Some(parent.path), parent.depth + 1)
    } else {
        (None, 0)
    };

    let path_str = build_ltree_path(parent_path.as_deref(), id)?;

   let insert = SqlQuery::insert()
    .into_table(departments::Entity)
    .columns([
        departments::Column::Id,
        departments::Column::Name,
        departments::Column::Description,
        departments::Column::ParentId,
        departments::Column::Depth,
        departments::Column::Path,
        departments::Column::CreatedAt,
        departments::Column::UpdatedAt,
    ])
    .values_panic([
        id.into(),
        req.name.clone().into(),
        req.description.clone().into(),
        req.parent_id.into(),
        depth.into(),
        Expr::val(path_str.clone())
            .cast_as(Alias::new("ltree"))
            .into(),
        created_at.into(),
        updated_at.into(),
    ])
    .to_owned();
let (sql, values) = insert.build(PostgresQueryBuilder);
    app_state
    .database
    .execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .map_err(|e| {
        eprintln!("insert error: {e}");
        AuthError::DbTimeout
    })?;

    if let Some(admin_ids) = req.admin_ids.as_deref() {
        sync_department_admin_assignments(
            &authz,
            &app_state.database,
            claims.user_id,
            id,
            admin_ids,
            req.parent_id,
        )
        .await?;
    }
    let (_, Json(response)) = get_department_by_id(claims, State(app_state), Path(id)).await?;
    Ok((StatusCode::CREATED, Json(response)))
}

#[derive(Debug, FromQueryResult)]
struct DeptCountRow {
    #[sea_orm(from_alias = "departmentId")]
    department_id: Uuid,
    cnt: i64,
}

#[derive(Debug, FromQueryResult)]
struct ChildCountRow {
    #[sea_orm(from_alias = "parentId")]
    parent_id: Uuid,
    cnt: i64,
}

#[utoipa::path(
    get,
    path = "/admin/departments",
    tag = "admin",
    responses(
       (status = 200, body = DepartmentsListResponse),
       (status = 401, content_type = "application/json", body = AuthErrorResponse),
       (status = 404, content_type = "application/json", body = AuthErrorResponse),
       (status = 503, content_type = "application/json", body = AuthErrorResponse),
    )
)]
pub async fn list_departments(
    claims: Claims,
    State(app_state): State<SharedState>,
    Query(q): Query<DepartmentListQuery>,
) -> Result<(StatusCode, Json<DepartmentsListResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    let target_scope = q
        .parent_id
        .as_deref()
        .and_then(|parent| {
            if parent.eq_ignore_ascii_case("root") {
                None
            } else {
                Uuid::parse_str(parent).ok()
            }
        });
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_DEPARTMENTS_VIEW,
            target_scope,
            PermissionScopeMode::RequireOrgWide,
            target_scope,
        )
        .await?;

    let mut query = departments_base_select();

    if let Some(parent) = q.parent_id.as_deref() {
        if parent.eq_ignore_ascii_case("root") {
            query = query.filter(departments::Column::ParentId.is_null());
        } else {
            let parent_uuid =
                Uuid::parse_str(parent).map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;

            if q.include_children {
                let parent_dept = departments_base_select()
                    .filter(departments::Column::Id.eq(parent_uuid))
                    .into_model::<DepartmentRow>()
                    .one(&app_state.database)
                    .await
                    .map_err(|e| {
                        eprintln!("db error: {e}");
                        AuthError::DbTimeout
                    })?
                    .ok_or(AuthError::DbNotFound)?;

                // path <@ CAST(parent_path AS ltree)  (no raw string)
                query = query.filter(
                    Expr::col(departments::Column::Path).binary(
                        BinOper::Custom("<@".into()),
                        Expr::val(parent_dept.path.clone()).cast_as(Alias::new("ltree")),
                    ),
                );
            } else {
                query = query.filter(departments::Column::ParentId.eq(parent_uuid));
            }
        }
    }

    let rows = query
        .into_model::<DepartmentRow>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error: {e}");
            AuthError::DbTimeout
        })?;

    let total = rows.len() as i64;

    // If no departments, return early
    if rows.is_empty() {
        return Ok((
            StatusCode::OK,
            Json(DepartmentsListResponse {
                departments: vec![],
                total,
            }),
        ));
    }

    // Collect department IDs
    let dept_ids: Vec<Uuid> = rows.iter().map(|d| d.id).collect();

    // -------- member_count (direct users) in ONE query --------
    let direct_counts: Vec<DeptCountRow> = users::Entity::find()
        .select_only()
        .column(users::Column::DepartmentId)
        .expr_as(Func::count(Expr::col(users::Column::Id)), "cnt")
        .filter(users::Column::DepartmentId.is_in(dept_ids.clone()))
        .filter(users::Column::Status.ne(UserStatus::Deleted))
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

    // -------- child_count (direct children) in ONE query --------
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

    // Build response
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

        departments.push(DepartmentResponse {
            id: d.id,
            name: d.name,
            description: d.description,
            parent_id: d.parent_id,
            path: d.path,
            depth: d.depth,
            admin_ids,
            member_count,
            total_member_count: 0, // keep for detail endpoint to avoid heavy list queries
            child_count,
            budget_allocated: d.budget_allocated.to_string().parse().unwrap_or(0.0),
            budget_distributed,
            budget_available,
            budget_used,
            budget_period: d.budget_period,
            created_at: d.created_at,
            updated_at: d.updated_at,
        });
    }

    Ok((StatusCode::OK, Json(DepartmentsListResponse { departments, total })))
}

#[utoipa::path(
    get,
    path = "/admin/departments/tree",
    tag = "admin",
    params(
        ("root_id" = Option<Uuid>, Query, description = "Start from this department (default: entire org)"),
        ("max_depth" = Option<i32>, Query, description = "Default value : 10")
    ),
    responses(
       (status = 200, body = DepartmentTreeResponse, description = "Department tree"),
       (status = 401, content_type = "application/json", body = AuthErrorResponse, description = "Unauthorized"),
       (status = 403, content_type = "application/json", body = AuthErrorResponse, description = "Forbidden - Admin role required"),
       (status = 404, content_type = "application/json", body = AuthErrorResponse, description = "Resource not found"),
       (status = 503, content_type = "application/json", body = AuthErrorResponse, description = "DB timeout/unavailable")
    )
)]
pub async fn get_departments_tree(
    claims: Claims,
    State(app_state): State<SharedState>,
    Query(q): Query<DepartmentTreeQuery>,
) -> Result<(StatusCode, Json<DepartmentTreeResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_DEPARTMENTS_VIEW,
            q.root_id,
            PermissionScopeMode::RequireOrgWide,
            q.root_id,
        )
        .await?;

    let max_depth = q.max_depth.unwrap_or(10).max(0);

    let root = if let Some(root_id) = q.root_id {
        Some(
            departments_base_select()
                .filter(departments::Column::Id.eq(root_id))
                .into_model::<DepartmentRow>()
                .one(&app_state.database)
                .await
                .map_err(|e| {
                    eprintln!("db error: {e}");
                    AuthError::DbTimeout
                })?
                .ok_or(AuthError::DbNotFound)?,
        )
    } else {
        None
    };

    let mut query = departments_tree_select();

    if let Some(root_dept) = &root {
        query = query.filter(
            Expr::col(departments::Column::Path).binary(
                BinOper::Custom("<@".into()),
                Expr::val(root_dept.path.clone()).cast_as(Alias::new("ltree")),
            ),
        );
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
        return Ok((StatusCode::OK, Json(DepartmentTreeResponse { tree: vec![] })));
    }

    let dept_ids: Vec<Uuid> = rows.iter().map(|d| d.id).collect();

    let direct_counts: Vec<DeptCountRow> = users::Entity::find()
        .select_only()
        .column(users::Column::DepartmentId)
        .expr_as(Func::count(Expr::col(users::Column::Id)), "cnt")
        .filter(users::Column::DepartmentId.is_in(dept_ids.clone()))
        .filter(users::Column::Status.ne(UserStatus::Deleted))
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
        // Guard against accidental cycles in the hierarchy to avoid infinite recursion.
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

    Ok((StatusCode::OK, Json(DepartmentTreeResponse { tree })))
}

#[utoipa::path(
    get,
    path = "/admin/department/{department_id}",
    tag = "admin",
    params(
        ("department_id" = Uuid, Path, description = "Department id")
    ),
    responses(
       (status = 200, body = DepartmentResponse),
       (status = 401, content_type = "application/json", body = AuthErrorResponse),
       (status = 404, content_type = "application/json", body = AuthErrorResponse),
       (status = 503, content_type = "application/json", body = AuthErrorResponse),
    )
)]
pub async fn get_department_by_id(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(department_id): Path<Uuid>,
) -> Result<(StatusCode, Json<DepartmentResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_DEPARTMENTS_VIEW,
            Some(department_id),
            PermissionScopeMode::RequireOrgWide,
            Some(department_id),
        )
        .await?;

    let dept = departments_base_select()
        .filter(departments::Column::Id.eq(department_id))
        .into_model::<DepartmentRow>()
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::DbNotFound)?;

    let mut admin_map = load_department_admin_ids_map(&app_state.database, &[dept.id]).await?;
    let admin_ids = admin_map.remove(&dept.id).unwrap_or_default();

    // 1) Direct members count
    let member_count = users::Entity::find()
        .filter(users::Column::DepartmentId.eq(dept.id))
        .filter(users::Column::Status.ne(UserStatus::Deleted))
        .count(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("count error: {e}");
            AuthError::DbTimeout
        })? as i32;

    // 2) Subtree department IDs (self + descendants), using ltree: d.path <@ (dept.path::ltree)
    let subtree_ids: Vec<Uuid> = departments::Entity::find()
        .select_only()
        .column(departments::Column::Id)
        .filter(
            Expr::col(departments::Column::Path).binary(
                BinOper::Custom("<@".into()),
                Expr::val(dept.path.clone()).cast_as(Alias::new("ltree")),
            ),
        )
        .into_tuple::<(Uuid,)>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("subtree query error: {e}");
            AuthError::DbTimeout
        })?
        .into_iter()
        .map(|(id,)| id)
        .collect();

    // 3) Total members count (self + descendants)
    let total_member_count = users::Entity::find()
        .filter(users::Column::DepartmentId.is_in(subtree_ids))
        .filter(users::Column::Status.ne(UserStatus::Deleted))
        .count(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("total count error: {e}");
            AuthError::DbTimeout
        })? as i32;

    // 4) Optional: direct child count
    let child_count = departments::Entity::find()
        .filter(departments::Column::ParentId.eq(dept.id))
        .count(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("child count error: {e}");
            AuthError::DbTimeout
        })? as i32;

    let (budget_distributed, budget_available, budget_used) = department_budget_snapshot(
        &app_state.database,
        dept.id,
        dept.budget_allocated,
        dept.budget_period,
    )
    .await?;

    let resp = DepartmentResponse {
        id: dept.id,
        name: dept.name,
        description: dept.description,
        parent_id: dept.parent_id,
        path: dept.path,
        depth: dept.depth,
        admin_ids,
        member_count,
        total_member_count,
        child_count,
        budget_allocated: dept.budget_allocated.to_string().parse().unwrap_or(0.0),
        budget_distributed,
        budget_available,
        budget_used,
        budget_period: dept.budget_period,
        created_at: dept.created_at,
        updated_at: dept.updated_at,
    };

    Ok((StatusCode::OK, Json(resp)))
}

#[utoipa::path( 
    put, path = "/admin/departments/{department_id}",
    tag = "admin", params( ("department_id" = Uuid, Path, description = "Department id") ),
    request_body = DepartmentUpdateRequest,
    responses( 
        (status = 200, body = DepartmentResponse),
        (status = 401, content_type = "application/json", body = AuthErrorResponse),
        (status = 404, content_type = "application/json", body = AuthErrorResponse),
        (status = 503, content_type = "application/json", body = AuthErrorResponse), 
    ) 
)]
pub async fn update_department(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(department_id): Path<Uuid>,
    Json(req): Json<DepartmentUpdateRequest>,
) -> Result<(StatusCode, Json<DepartmentResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_DEPARTMENTS_MANAGE,
            Some(department_id),
            PermissionScopeMode::RequireOrgWide,
            Some(department_id),
        )
        .await?;

    let dept = departments_base_select()
        .filter(departments::Column::Id.eq(department_id))
        .into_model::<DepartmentRow>()
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::DbNotFound)?;

    let name = req.name.clone().unwrap_or(dept.name.clone());
    let description = req.description.clone().unwrap_or(dept.description.clone());
    let parent_id = req.parent_id.or(dept.parent_id);
    let parent_changed = parent_id != dept.parent_id;
    let budget_period = req.budget_period.unwrap_or(dept.budget_period);
    let action_on_exceed = req.action_on_exceed.unwrap_or(dept.action_on_exceed);
    let budget_allocated = if let Some(value) = req.budget_allocated {
        Decimal::from_f32_retain(value).ok_or(AuthError::ServiceTemporarilyUnavailable)?
    } else {
        dept.budget_allocated
    };

    if req.parent_id.is_some() || req.budget_allocated.is_some() {
        if let Some(parent_id) = parent_id {
            let parent = departments_base_select()
                .filter(departments::Column::Id.eq(parent_id))
                .into_model::<DepartmentRow>()
                .one(&app_state.database)
                .await
                .map_err(|e| {
                    eprintln!("find parent error: {e}");
                    AuthError::DbTimeout
                })?
                .ok_or(AuthError::DbNotFound)?;

            let other_children_alloc =
                sum_child_allocations(&app_state.database, parent.id, Some(dept.id))
                    .await
                    .map_err(|e| {
                        eprintln!("sum other children error: {e}");
                        AuthError::DbTimeout
                    })?;

            let now = Utc::now();
            let (p_start, p_end) = period_bounds(&parent.budget_period, now);
            let parent_used =
                sum_department_cost_in_range(&app_state.database, parent.id, p_start, p_end)
                    .await
                    .map_err(|e| {
                        eprintln!("sum parent used error: {e}");
                        AuthError::DbTimeout
                    })?;

            let parent_available_for_children =
                (parent.budget_allocated - other_children_alloc - parent_used)
                    .max(Decimal::ZERO);

            if budget_allocated > parent_available_for_children {
                return Err(AuthError::BudgetExceedsParentAvailable);
            }
        }
    }

    // Recompute path/depth if parent changes
    let (path, depth) = if parent_changed {
        if let Some(parent_id) = parent_id {
            let parent = departments_base_select()
                .filter(departments::Column::Id.eq(parent_id))
                .into_model::<DepartmentRow>()
                .one(&app_state.database)
                .await
                .map_err(|e| {
                    eprintln!("db error: {e}");
                    AuthError::DbTimeout
                })?
                .ok_or(AuthError::DbNotFound)?;

            let label = ltree_label_from_uuid(department_id);
            (format!("{}.{}", parent.path, label), parent.depth + 1)
        } else {
            (ltree_label_from_uuid(department_id), 0)
        }
    } else {
        (dept.path.clone(), dept.depth)
    };

    let updated_at = Utc::now();

    // ✅ SeaQuery UPDATE with CAST(... AS ltree)
    let stmt = SqlQuery::update()
        .table(departments::Entity)
        .values([
            (departments::Column::Name, name.into()),
            (departments::Column::Description, description.into()),
            (departments::Column::ParentId, parent_id.into()),
            (departments::Column::Depth, depth.into()),
            (
                departments::Column::Path,
                Expr::val(path.clone())
                    .cast_as(Alias::new("ltree"))
                    .into(),
            ),
            (departments::Column::BudgetAllocated, budget_allocated.into()),
            (departments::Column::BudgetPeriod, budget_period.into()),
            (departments::Column::ActionOnExceed, action_on_exceed.into()),
            (departments::Column::UpdatedAt, updated_at.into()),
        ])
        .and_where(Expr::col(departments::Column::Id).eq(department_id))
        .to_owned();

    let (sql, values) = stmt.build(PostgresQueryBuilder);

    app_state
        .database
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            values,
        ))
        .await
        .map_err(|e| {
            eprintln!("update error: {e}");
            AuthError::DbTimeout
        })?;

    if let Some(admin_ids) = req.admin_ids.as_deref() {
        sync_department_admin_assignments(
            &authz,
            &app_state.database,
            claims.user_id,
            department_id,
            admin_ids,
            Some(department_id),
        )
        .await?;
    }

    if parent_changed {
        let _ = authz
            .recompute_effective_permissions_for_department_scope(department_id)
            .await;
    }

    get_department_by_id(claims, State(app_state), Path(department_id)).await
}

#[utoipa::path(
    post,
    path = "/admin/departments/{department_id}/move",
    tag = "admin",
    params(
        ("department_id" = Uuid, Path, description = "Department id")
    ),
    request_body = MoveDepartmentRequest,
    responses(
        (status = 200, body = DepartmentResponse, description = "Department moved successfully"),
        (status = 401, content_type = "application/json", body = AuthErrorResponse),
        (status = 403, content_type = "application/json", body = AuthErrorResponse, description = "Forbidden - Admin role required"),
        (status = 404, content_type = "application/json", body = AuthErrorResponse),
        (status = 409, content_type = "application/json", body = AuthErrorResponse, description = "Invalid move target"),
        (status = 503, content_type = "application/json", body = AuthErrorResponse),
    )
)]
pub async fn move_department(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(department_id): Path<Uuid>,
    Json(req): Json<MoveDepartmentRequest>,
) -> Result<(StatusCode, Json<DepartmentResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_DEPARTMENTS_MANAGE,
            Some(department_id),
            PermissionScopeMode::RequireOrgWide,
            Some(department_id),
        )
        .await?;
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_DEPARTMENTS_MANAGE,
            Some(req.new_parent_id),
            PermissionScopeMode::RequireOrgWide,
            Some(req.new_parent_id),
        )
        .await?;

    let dept = departments_base_select()
        .filter(departments::Column::Id.eq(department_id))
        .into_model::<DepartmentRow>()
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::DbNotFound)?;

    let new_parent = departments_base_select()
        .filter(departments::Column::Id.eq(req.new_parent_id))
        .into_model::<DepartmentRow>()
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::DbNotFound)?;

    // Prevent moving under itself or one of its descendants.
    if req.new_parent_id == department_id || new_parent.path.starts_with(&dept.path) {
        return Err(AuthError::DbConflict);
    }

    let label = ltree_label_from_uuid(department_id);
    let new_path = format!("{}.{}", new_parent.path, label);
    let new_depth = new_parent.depth + 1;
    let depth_delta = new_depth - dept.depth;
    let updated_at = Utc::now();

    // Update the root department's parent reference.
    let parent_stmt = SqlQuery::update()
        .table(departments::Entity)
        .values([
            (departments::Column::ParentId, req.new_parent_id.into()),
            (departments::Column::UpdatedAt, updated_at.into()),
        ])
        .and_where(Expr::col(departments::Column::Id).eq(department_id))
        .to_owned();

    let (parent_sql, parent_values) = parent_stmt.build(PostgresQueryBuilder);
    app_state
        .database
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            parent_sql,
            parent_values,
        ))
        .await
        .map_err(|e| {
            eprintln!("db update error: {e}");
            AuthError::DbTimeout
        })?;

    // Update the entire subtree's path + depth using a prefix replace.
    let subtree_stmt = SqlQuery::update()
        .table(departments::Entity)
        .values([
            (
                departments::Column::Depth,
                Expr::col(departments::Column::Depth)
                    .add(depth_delta)
                    .into(),
            ),
            (
                departments::Column::Path,
                Expr::cust(format!(
                    "replace(path::text, '{}', '{}')::ltree",
                    dept.path, new_path
                ))
                .into(),
            ),
            (departments::Column::UpdatedAt, updated_at.into()),
        ])
        .and_where(
            Expr::col(departments::Column::Path).binary(
                BinOper::Custom("<@".into()),
                Expr::val(dept.path.clone()).cast_as(Alias::new("ltree")),
            ),
        )
        .to_owned();

    let (subtree_sql, subtree_values) = subtree_stmt.build(PostgresQueryBuilder);
    app_state
        .database
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            subtree_sql,
            subtree_values,
        ))
        .await
        .map_err(|e| {
            eprintln!("db subtree update error: {e}");
            AuthError::DbTimeout
        })?;

    let _ = authz
        .recompute_effective_permissions_for_department_scope(department_id)
        .await;

    get_department_by_id(claims, State(app_state), Path(department_id)).await
}

#[utoipa::path(
    delete,
    path = "/admin/departments/{department_id}",
    tag = "admin",
    params(
        ("department_id" = Uuid, Path, description = "Department id")
    ),
    responses(
       (status = 204, description = "Deleted"),
       (status = 401, content_type = "application/json", body = AuthErrorResponse),
       (status = 404, content_type = "application/json", body = AuthErrorResponse),
       (status = 503, content_type = "application/json", body = AuthErrorResponse),
    )
)]
pub async fn delete_department(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(department_id): Path<Uuid>,
) -> Result<StatusCode, AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_DEPARTMENTS_MANAGE,
            Some(department_id),
            PermissionScopeMode::RequireOrgWide,
            Some(department_id),
        )
        .await?;
    // Remove scoped role assignments for this department to avoid FK set-null conflicts
    let _ = user_role_assignments::Entity::delete_many()
        .filter(user_role_assignments::Column::ScopeDepartmentId.eq(department_id))
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role assignment delete error: {e}");
            AuthError::DbTimeout
        })?;
    let res = departments::Entity::delete_by_id(department_id)
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("delete error: {e}");
            AuthError::DbTimeout
        })?;
    if res.rows_affected == 0 {
        return Err(AuthError::DbNotFound);
    }
    let _ = authz.recompute_effective_permissions_for_all_users().await;
  Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/admin/departments/{department_id}/members",
    tag = "admin",
    request_body = Vec<Uuid>,
    params(
        ("department_id" = Uuid, Path, description = "Department id")
    ),
    responses(
       (status = 200, body = DepartmentResponse),
       (status = 401, content_type = "application/json", body = AuthErrorResponse),
       (status = 404, content_type = "application/json", body = AuthErrorResponse),
       (status = 503, content_type = "application/json", body = AuthErrorResponse),
    )
)]
pub async fn add_users_in_department(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(department_id): Path<Uuid>,
    Json(user_ids):Json<Vec<Uuid>>,
) -> Result<(StatusCode,Json<DepartmentResponse>), AuthError> {
     let authz = AuthorizationService::new(&app_state.database);
     authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_DEPARTMENTS_MANAGE,
            Some(department_id),
            PermissionScopeMode::RequireOrgWide,
            Some(department_id),
        )
        .await?;
    let response = get_department_by_id(claims,State(app_state.clone()),Path(department_id.clone()))
        .await
        .map_err(|_|{
          AuthError::DbTimeout  
        })?;
     users::Entity::update_many()
        .filter(users::Column::Id.is_in(user_ids.clone()))
        .col_expr(users::Column::DepartmentId, Expr::value(department_id))
        .col_expr(users::Column::UpdatedAt, Expr::value(Utc::now()))
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db update many error: {e}");
            AuthError::DbTimeout
        })?;
    let _ = authz.recompute_effective_permissions_for_users(&user_ids).await;
    Ok(response)
}

#[utoipa::path(
    delete,
    path = "/admin/departments/{department_id}/members",
    tag = "admin",
    params(
        ("department_id" = Uuid, Path, description = "Department id"),
        ("department_id" = Uuid, Path, description = "Department id"),
    ),
    responses(
       (status = 200, body = DepartmentResponse),
       (status = 401, content_type = "application/json", body = AuthErrorResponse),
       (status = 404, content_type = "application/json", body = AuthErrorResponse),
       (status = 503, content_type = "application/json", body = AuthErrorResponse),
    )
)]
pub async fn remove_users_from_department(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(department_id): Path<Uuid>,
    Query(query):Query<DepartmentMemeberListQuery>,
    Json(user_ids):Json<Vec<Uuid>>,
) -> Result<(StatusCode,Json<DepartmentResponse>), AuthError> {
     let authz = AuthorizationService::new(&app_state.database);
     authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_DEPARTMENTS_MANAGE,
            Some(department_id),
            PermissionScopeMode::RequireOrgWide,
            Some(department_id),
        )
        .await?;
 let force = query.force.unwrap_or(false);

 let users_t = Alias::new(users::Entity.table_name());
 let depts_t = Alias::new(departments::Entity.table_name());

// SELECT parent_id FROM departments WHERE id = ?
let parent_select = SqlQuery::select()
    .column((depts_t.clone(), departments::Column::ParentId))
    .from(depts_t.clone())
    .and_where(Expr::col((depts_t.clone(), departments::Column::Id)).eq(department_id))
    .to_owned();

let parent_subexpr: SimpleExpr = SimpleExpr::SubQuery(
    None,
    Box::new(parent_select.into_sub_query_statement()),
);

let new_dept_expr: SimpleExpr = if force {
    parent_subexpr
} else {
    // department_id = NULL
    Expr::value(Option::<Uuid>::None).into()
};

// UPDATE users SET department_id = (subquery or NULL) WHERE ...
let update_stmt = SqlQuery::update()
    .table(users_t.clone())
    .value(users::Column::DepartmentId, new_dept_expr)
    .value(users::Column::UpdatedAt, Expr::value(Utc::now()))
    .and_where(Expr::col((users_t.clone(), users::Column::Id)).is_in(user_ids.clone()))
    .and_where(Expr::col((users_t.clone(), users::Column::DepartmentId)).eq(department_id))
    .to_owned();

// execute (no manual SQL text; SQL is generated + values bound)
let (sql, values) = update_stmt.build(PostgresQueryBuilder);

app_state
    .database
    .execute(Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values))
    .await
    .map_err(|e| {
        eprintln!("db update error: {e}");
        AuthError::DbTimeout
    })?;
  let _ = authz.recompute_effective_permissions_for_users(&user_ids).await;
  get_department_by_id(claims,State(app_state),Path(department_id))
   .await
}

#[utoipa::path(
    get,
    path = "/admin/departments/{department_id}/members",
    tag = "admin",
    params(
        ("department_id" = Uuid, Path, description = "department_id Uuid"),
        ("include_sub_department" = bool, Query, description = "Default value : false"),
        ("search" = Option<String>, Query, description = "Search by name,email,department"),
        ("status" = Option<UserStatus>, Query, description = "Account status"),
        ("role_id" = Option<Uuid>, Query, description = "Filter by RBAC role id"),
        ("sort" = Option<SortRule>, Query, description = "Sort by column example 'name','updated_at','created_at','email','last_login_at'"),
        ("order" = Option<String>, Query, description = "Sort order (asc/desc)"),
    ),
    responses(
       (status = 200, body = DepartmentMembersResponse),
       (status = 401, content_type = "application/json", body = AuthErrorResponse),
       (status = 404, content_type = "application/json", body = AuthErrorResponse),
       (status = 503, content_type = "application/json", body = AuthErrorResponse),
    )
)]
pub async fn get_users_from_department(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(department_id): Path<Uuid>,
    Query(query):Query<DepartmentMemeberListQuery>
) -> Result<(StatusCode,Json<DepartmentMembersResponse>), AuthError> {
     let include_sub_department = query.include_sub_department.unwrap_or(false);
     let mut response = DepartmentMembersResponse{total:0,members:Vec::new()};
     let authz = AuthorizationService::new(&app_state.database);
     authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_USERS_VIEW,
            Some(department_id),
            PermissionScopeMode::RequireOrgWide,
            Some(department_id),
        )
        .await?;
    let root = departments_base_select()
      .filter(departments::Column::Id.eq(department_id))
      .into_model::<DepartmentRow>()
      .one(&app_state.database)
      .await
      .map_err(|e| { eprintln!("db error: {e}"); AuthError::DbTimeout })?
      .ok_or(AuthError::DbNotFound)?;

    let subtree_dept_ids: Vec<Uuid> = departments::Entity::find()
       .select_only()
       .column(departments::Column::Id)
       .filter(
         Expr::col(departments::Column::Path).binary(
            BinOper::Custom("<@".into()),
            // RHS must be ltree-typed
            Expr::val(root.path.clone()).cast_as(Alias::new("ltree")),
        ),
        )
        .into_tuple() // returns Vec<(Uuid,)>
        .all(&app_state.database)
        .await
        .map_err(|e| { eprintln!("db error: {e}"); AuthError::DbTimeout })?
        .into_iter()
        .map(|(id,)| id)
        .collect();
    let mut select = users::Entity::find()
        .filter(users::Column::Status.ne(UserStatus::Deleted))
        .join(JoinType::LeftJoin, users::Relation::Departments.def());
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
            return Ok((StatusCode::OK, Json(DepartmentMembersResponse { total: 0, members: Vec::new() })));
        }
        select = select.filter(users::Column::Id.is_in(role_user_ids));
    }
    if let Some(status) = query.status{
       select = select.filter(users::Column::Status.eq(status))
    }
    let order = query.order.as_deref().unwrap_or("desc");
    let ord = if order.eq_ignore_ascii_case("asc") {
        Order::Asc
    } else {
        Order::Desc
    };
    if let Some(sort) = query.sort{
       select = match sort {
          SortRule::Name => select.order_by(users::Column::Name,ord),
          SortRule::Email => select.order_by(users::Column::Email,ord),
          SortRule::CreatedAt => select.order_by(users::Column::CreatedAt,ord),
          SortRule::UpdatedAt => select.order_by(users::Column::UpdatedAt,ord),
          SortRule::LastLoginAt => select.order_by(users::Column::LastLoginAt,ord),
          _ => select.order_by(users::Column::CreatedAt,ord),
      };
    }
    if let Some(search) = &query.search{
       select = select.filter(    
       Condition::any()
         .add(users::Column::Name.into_expr().ilike(format!("%{}%", search)))
         .add(users::Column::Email.into_expr().ilike(format!("%{}%", search)))
         .add(departments::Column::Name.into_expr().ilike(format!("%{}%", search)))
     );
    }
    let users_row = if include_sub_department {
    // subtree_dept_ids logic above...
    select
        .filter(users::Column::DepartmentId.is_in(subtree_dept_ids))
        .all(&app_state.database)
        .await
        .map_err(|e| { eprintln!("db error: {e}"); AuthError::DbTimeout })? 
    } else {
       select
        .filter(users::Column::DepartmentId.eq(department_id))
        .all(&app_state.database)
        .await
        .map_err(|e| { eprintln!("db error: {e}"); AuthError::DbTimeout })?
     };
     let user_ids: Vec<Uuid> = users_row.iter().map(|u| u.id).collect();
     let roles_map = authz.user_roles_map(&user_ids).await?;
     response.members = users_row
       .into_iter()
       .map(|user| {
        let roles = roles_map.get(&user.id).cloned().unwrap_or_default();
        let is_super_admin = roles.iter().any(|r| r == "Super Admin");
        UserDetails{ 
            id:user.id,
            sub: user.azure_id.unwrap_or(user.google_id.unwrap_or(user.email.clone())),
            email:user.email,
            name: user.name,
            picture:user.picture,
            hd:user.hd,
            roles,
            status:user.status,
            department:None,
            department_id:user.department_id,
            is_super_admin,
            has_password:user.password.is_some(),
            mfa_enabled:user.mfa_enabled,
            last_login_at:Some(user.last_login_at),
            password_changed_at:user.password_changed_at,
            created_at:user.created_at,
            updated_at:user.updated_at,
            effective_permissions:user.effective_permissions,
        }
    }).collect();
    response.total = response.members.len() as i32;
  Ok((StatusCode::OK,Json(response)))
}
