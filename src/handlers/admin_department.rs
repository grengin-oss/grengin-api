use axum::{Json, extract::{Path, Query, State}};
use chrono::{DateTime, Utc};
use migration::Alias;
use sea_orm::{FromQueryResult, QuerySelect, sea_query::{Expr, PostgresQueryBuilder,Query as SqlQuery}};
use reqwest::StatusCode;
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait as _, QueryFilter, Statement, sqlx::postgres::types::{PgLTree, PgLTreeLabel}};
use uuid::Uuid;
use crate::{auth::{claims::Claims, error::{AuthError, AuthErrorResponse}}, dto::admin_department::{BudgetPeriod, DepartmentListQuery, DepartmentRequest, DepartmentResponse,DepartmentsListResponse}, models::{departments, users::UserRole}, state::SharedState};

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

#[derive(Debug, Clone, FromQueryResult)]
struct DepartmentRow {
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

// Selects all columns but casts path -> text so decoding works
fn departments_base_select() -> sea_orm::Select<departments::Entity> {
    use sea_orm::QuerySelect;

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

#[utoipa::path(
    post,
    path = "/admin/departments",
    tag = "admin",
    request_body = DepartmentRequest,
    responses(
       (status = 201, description = "Department created successfully"),
       (status = 401, content_type = "application/json", body = AuthErrorResponse),
       (status = 404, content_type = "application/json", body = AuthErrorResponse, description = "Parent department not found"),
       (status = 503, content_type = "application/json", body = AuthErrorResponse),
    )
)]
pub async fn create_department(
    claims: Claims,
    State(app_state): State<SharedState>,
    Json(req): Json<DepartmentRequest>,
) -> Result<(StatusCode,&'static str), AuthError> {
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
    }

    let id = Uuid::new_v4();
    let created_at = Utc::now();
    let updated_at = created_at;

    // Fetch parent (if any) to compute path/depth
    let (parent_path, depth) = if let Some(parent_id) = req.parent_id {
        let parent = departments::Entity::find_by_id(parent_id)
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

    Ok((StatusCode::CREATED,"Department created successfull"))
}

#[utoipa::path(
    get,
    path = "/admin/department",
    tag = "admin",
    params(
        ("department_id" = Uuid, Path, description = "Department id")
    ),
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
                // Fetch parent's path as text (casted), so we can bind it
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
                query = query.filter(Expr::cust_with_values(
                    r#"path <@ CAST(? AS ltree)"#,
                    [parent_dept.path],
                ));
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

    let departments = rows
        .into_iter()
        .map(|d| DepartmentResponse {
            id: d.id,
            name: d.name,
            description: d.description,
            parent_id: d.parent_id,
            path: d.path,
            depth: d.depth,
            leader_ids: vec![],
            member_count: 0,
            total_member_count: 0,
            child_count: 0,
            budget_allocated: 0.0,
            budget_distributed: 0.0,
            budget_available: 0.0,
            budget_used: 0.0,
            budget_period: BudgetPeriod::Daily,
            created_at: d.created_at,
            updated_at: d.updated_at,
        })
        .collect();

    Ok((StatusCode::OK, Json(DepartmentsListResponse { departments, total })))
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

    let dept = departments::Entity::find()
        .select_only()
        .column(departments::Column::Id)
        .column(departments::Column::Name)
        .column(departments::Column::Description)
        .column(departments::Column::ParentId)
        .column(departments::Column::Depth)
        .expr_as(Expr::cust("path::text"), "path")
        .column(departments::Column::CreatedAt)
        .column(departments::Column::UpdatedAt)
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
        id: dept.id,
        name: dept.name,
        description: dept.description,
        parent_id: dept.parent_id,
        path: dept.path,
        depth: dept.depth,
        leader_ids: vec![],
        member_count: 0,
        total_member_count: 0,
        child_count: 0,
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
    put, path = "/admin/department/{department_id}",
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

    let dept = departments::Entity::find_by_id(department_id)
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
            let parent = departments::Entity::find_by_id(parent_id)
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
    let updated = departments::Entity::find_by_id(department_id)
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
    delete,
    path = "/admin/department/{department_id}",
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