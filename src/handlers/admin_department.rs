use std::collections::{HashMap, HashSet};
use axum::{Json, extract::{Path, Query, State}};
use chrono::{DateTime, Utc};
use migration::{Alias, BinOper, Func, SimpleExpr, extension::postgres::PgExpr};
use rust_decimal::Decimal;
use sea_orm::{Condition, EntityName as _, FromQueryResult, JoinType, Order, PaginatorTrait, QueryOrder, QuerySelect, RelationTrait, sea_query::{Expr, PostgresQueryBuilder,Query as SqlQuery}};
use reqwest::StatusCode;
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait as _, QueryFilter, Statement, sqlx::postgres::types::{PgLTree, PgLTreeLabel}};
use uuid::Uuid;
use crate::{auth::{claims::Claims, error::{AuthError, AuthErrorResponse}}, dto::{admin_department::{DepartmentListQuery, DepartmentMembersResponse, DepartmentMemeberListQuery, DepartmentRequest, DepartmentResponse, DepartmentTreeNode, DepartmentTreeQuery, DepartmentTreeResponse, DepartmentsListResponse, MoveDepartmentRequest}, admin_user::UserDetails, common::SortRule}, models::{departments::{self, BudgetPeriod}, users::{self, UserRole, UserStatus}}, state::SharedState};

#[derive(Debug, Clone, FromQueryResult)]
pub struct DepartmentRow {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    #[sea_orm(from_alias = "parentId")]
    pub parent_id: Option<Uuid>,
    pub depth: i32,
    pub path: String, // comes from path::text alias
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
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
    }

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
  get_department_by_id(claims,State(app_state),Path(id))
    .await
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
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
    }

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

    // Build response
    let departments = rows
        .into_iter()
        .map(|d| {
            let member_count = *direct_map.get(&d.id).unwrap_or(&0) as i32;
            let child_count = *child_map.get(&d.id).unwrap_or(&0) as i32;

            DepartmentResponse {
                id: d.id,
                name: d.name,
                description: d.description,
                parent_id: d.parent_id,
                path: d.path,
                depth: d.depth,
                leader_ids: vec![],
                member_count,
                total_member_count: 0, // keep for detail endpoint to avoid heavy list queries
                child_count,
                budget_allocated: 0.0,
                budget_distributed: 0.0,
                budget_available: 0.0,
                budget_used: 0.0,
                budget_period: BudgetPeriod::Daily,
                created_at: d.created_at,
                updated_at: d.updated_at,
            }
        })
        .collect();

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
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
    }

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

    let mut nodes: HashMap<Uuid, DepartmentTreeNode> = HashMap::with_capacity(rows.len());
    let mut children_map: HashMap<Uuid, Vec<Uuid>> = HashMap::with_capacity(rows.len());

    for d in rows {
        let member_count = *direct_map.get(&d.id).unwrap_or(&0) as i32;

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
                leader_ids: vec![],
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
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
    }

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

    let resp = DepartmentResponse {
        id: dept.id,
        name: dept.name,
        description: dept.description,
        parent_id: dept.parent_id,
        path: dept.path,
        depth: dept.depth,
        leader_ids: vec![],
        member_count,
        total_member_count,
        child_count,
        budget_allocated: 0.0,
        budget_distributed: 0.0,
        budget_available: 0.0,
        budget_used: 0.0,
        budget_period: BudgetPeriod::Daily,
        created_at: dept.created_at,
        updated_at: dept.updated_at,
    };

    Ok((StatusCode::OK, Json(resp)))
}

#[utoipa::path( 
    put, path = "/admin/departments/{department_id}",
    tag = "admin", params( ("department_id" = Uuid, Path, description = "Department id") ),
    request_body = DepartmentRequest,
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
    Json(req): Json<DepartmentRequest>,
) -> Result<(StatusCode, Json<DepartmentResponse>), AuthError> {
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
    }

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

    // Recompute path/depth if parent changes
    let (path, depth) = if req.parent_id != dept.parent_id {
        if let Some(parent_id) = req.parent_id {
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
            (departments::Column::Name, req.name.clone().into()),
            (departments::Column::Description, req.description.clone().into()),
            (departments::Column::ParentId, req.parent_id.into()),
            (departments::Column::Depth, depth.into()),
            (
                departments::Column::Path,
                Expr::val(path.clone())
                    .cast_as(Alias::new("ltree"))
                    .into(),
            ),
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

    // Fetch updated row for response
    let updated = departments_base_select()
        .filter(departments::Column::Id.eq(department_id))
        .into_model::<DepartmentRow>()
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::DbNotFound)?;

    let resp = DepartmentResponse {
        id: updated.id,
        name: updated.name,
        description: updated.description,
        parent_id: updated.parent_id,
        path: updated.path,
        depth: updated.depth,
        leader_ids: req.leader_ids,
        member_count: 0,
        total_member_count: 0,
        child_count: 0,
        budget_allocated: 0.0,
        budget_distributed: 0.0,
        budget_available: 0.0,
        budget_used: 0.0,
        budget_period: BudgetPeriod::Daily,
        created_at: updated.created_at,
        updated_at: updated.updated_at,
    };

    Ok((StatusCode::OK, Json(resp)))
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
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
    }

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
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
    }
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
     match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
     }
    let response = get_department_by_id(claims,State(app_state.clone()),Path(department_id.clone()))
        .await
        .map_err(|_|{
          AuthError::DbTimeout  
        })?;
     users::Entity::update_many()
        .filter(users::Column::Id.is_in(user_ids))
        .col_expr(users::Column::DepartmentId, Expr::value(department_id))
        .col_expr(users::Column::UpdatedAt, Expr::value(Utc::now()))
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db update many error: {e}");
            AuthError::DbTimeout
        })?;
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
     match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
     }
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
    .and_where(Expr::col((users_t.clone(), users::Column::Id)).is_in(user_ids))
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
        ("role" = Option<UserRole>, Query, description = "UserRole superadmin,admin,user,observer"),
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
     match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
     }
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
    if let Some(role) = query.role  {
       select = select.filter(users::Column::Role.eq(role));
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
     response.members = users_row
       .into_iter()
       .map(|user| UserDetails{ 
        id:user.id,
        sub: user.azure_id.unwrap_or(user.google_id.unwrap_or(user.email.clone())),
        email:user.email,
        name: user.name,
        picture:user.picture,
        hd:user.hd,
        role:user.role,
        status:user.status,
        department:None,
        department_id:user.department_id,
        is_super_admin:user.role == UserRole::SuperAdmin,
        has_password:user.password.is_some(),
        mfa_enabled:user.mfa_enabled,
        last_login_at:Some(user.last_login_at),
        password_changed_at:user.password_changed_at,
        created_at:user.created_at,
        updated_at:user.updated_at 
    }).collect();
    response.total = response.members.len() as i32;
  Ok((StatusCode::OK,Json(response)))
}
