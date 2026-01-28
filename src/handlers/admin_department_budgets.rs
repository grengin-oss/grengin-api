use axum::{extract::{Path, State}, Json};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use migration::Expr;
use rust_decimal::Decimal;
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait, FromQueryResult, QueryFilter, QuerySelect, Statement, sea_query::{Query as SqlQuery, PostgresQueryBuilder}};
use uuid::Uuid;
use crate::{auth::{claims::Claims, error::{AuthError, AuthErrorResponse}}, dto::admin_department_budget::{DepartmentBudgetStatusDto, DepartmentBudgetUpdatedDto, SetDepartmentBudgetRequest, SubDepartmentBudgetDto}, models::{departments::{self, ActionOnExceed, BudgetPeriod}, users::UserRole}, services::budget_allocation::{period_bounds, sum_child_allocations, sum_department_cost_in_range, sum_department_cost_total}, state::SharedState};

#[derive(Debug, Clone, FromQueryResult)]
pub struct DepartmentBudgetRow {
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

fn departments_budget_select() -> sea_orm::Select<departments::Entity> {
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


#[utoipa::path(
    get,
    path = "/admin/departments/{department_id}/budget",
    tag = "admin",
    params(
        ("department_id" = Uuid, Path, description = "Unique identifier for the department")
    ),
    responses(
        (status = 200, content_type = "application/json", body = DepartmentBudgetStatusDto, description = "Budget status"),
        (status = 401, content_type = "application/json", body = AuthErrorResponse, description = "Invalid/expired token (code=6103)"),
        (status = 403, content_type = "application/json", body = AuthErrorResponse, description = "Forbidden - Admin role required"),
        (status = 404, content_type = "application/json", body = AuthErrorResponse, description = "Resource not found"),
        (status = 503, content_type = "application/json", body = AuthErrorResponse, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)")
    )
)]
pub async fn get_department_budget(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(department_id): Path<uuid::Uuid>,
) -> Result<(StatusCode, Json<DepartmentBudgetStatusDto>), AuthError> {
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => (),
        _ => return Err(AuthError::PermissionDenied),
    }

    let dept = departments_budget_select()
        .filter(departments::Column::Id.eq(department_id))
        .into_model::<DepartmentBudgetRow>()
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("find department error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    let now = chrono::Utc::now();
    let (period_start, period_end) = period_bounds(&dept.budget_period, now);

    let budget_distributed = sum_child_allocations(&app_state.database, dept.id, None)
        .await
        .map_err(|e| {
            eprintln!("sum_child_allocations error: {e}");
            AuthError::DbTimeout
        })?;

    let budget_used = sum_department_cost_in_range(&app_state.database, dept.id, period_start, period_end)
        .await
        .map_err(|e| {
            eprintln!("sum_department_cost_in_range error: {e}");
            AuthError::DbTimeout
        })?;

    let budget_used_total = sum_department_cost_total(&app_state.database, dept.id)
        .await
        .map_err(|e| {
            eprintln!("sum_department_cost_total error: {e}");
            AuthError::DbTimeout
        })?;

    let budget_available = (dept.budget_allocated - budget_distributed).max(rust_decimal::Decimal::ZERO);

    // direct children budgets
    let children = departments_budget_select()
        .filter(departments::Column::ParentId.eq(dept.id))
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("find children error: {e}");
            AuthError::DbTimeout
        })?;

    let mut sub_department_budgets = Vec::with_capacity(children.len());
    for c in children {
        let used = sum_department_cost_in_range(&app_state.database, c.id, period_start, period_end)
            .await
            .map_err(|e| {
                eprintln!("sum child dept used error: {e}");
                AuthError::DbTimeout
            })?;

        sub_department_budgets.push(SubDepartmentBudgetDto {
            department_id: c.id,
            name: c.name,
            allocated: c.budget_allocated.to_string().parse().unwrap(),
            used:used.to_string().parse().unwrap(),
        });
    }

    Ok((
        StatusCode::OK,
        Json(DepartmentBudgetStatusDto {
            department_id: dept.id,
            budget_allocated: dept.budget_allocated.to_string().parse().unwrap(),
            budget_distributed:budget_distributed.to_string().parse().unwrap(),
            budget_available:budget_available.to_string().parse().unwrap(),
            budget_used:budget_used.to_string().parse().unwrap(),
            budget_used_total:budget_used_total.to_string().parse().unwrap(),
            period: dept.budget_period,
            period_start,
            period_end,
            sub_department_budgets,
        }),
    ))
}

#[utoipa::path(
    put,
    path = "/admin/departments/{department_id}/budget",
    tag = "admin",
    request_body = SetDepartmentBudgetRequest,
    params(
        ("department_id" = Uuid, Path, description = "Unique identifier for the department")
    ),
    responses(
        (status = 200, content_type = "application/json", body = DepartmentBudgetUpdatedDto, description = "Budget updated"),
        (status = 400, content_type = "application/json", body = AuthErrorResponse, description = "Exceeds parent's available budget"),
        (status = 401, content_type = "application/json", body = AuthErrorResponse, description = "Invalid/expired token (code=6103)"),
        (status = 403, content_type = "application/json", body = AuthErrorResponse, description = "Forbidden - Admin role required"),
        (status = 404, content_type = "application/json", body = AuthErrorResponse, description = "Resource not found"),
        (status = 503, content_type = "application/json", body = AuthErrorResponse, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)")
    )
)]
pub async fn set_department_budget(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(department_id): Path<uuid::Uuid>,
    Json(req): Json<SetDepartmentBudgetRequest>,
) -> Result<(StatusCode, Json<DepartmentBudgetUpdatedDto>), AuthError> {
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => (),
        _ => return Err(AuthError::PermissionDenied),
    }

    let dept = departments_budget_select()
        .filter(departments::Column::Id.eq(department_id))
        .into_model::<DepartmentBudgetRow>()
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("find department error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    // parent constraint
    if let Some(parent_id) = dept.parent_id {
        let parent = departments_budget_select()
            .filter(departments::Column::Id.eq(parent_id))
            .into_model::<DepartmentBudgetRow>()
            .one(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("find parent error: {e}");
                AuthError::DbTimeout
            })?
            .ok_or(AuthError::ResourceNotFound)?;

        let other_children_alloc = sum_child_allocations(&app_state.database, parent.id, Some(dept.id))
            .await
            .map_err(|e| {
                eprintln!("sum other children error: {e}");
                AuthError::DbTimeout
            })?;

        // subtract parent's own usage (same period as parent)
        let now = chrono::Utc::now();
        let (p_start, p_end) = period_bounds(&parent.budget_period, now);
        let parent_used = sum_department_cost_in_range(&app_state.database, parent.id, p_start, p_end)
            .await
            .map_err(|e| {
                eprintln!("sum parent used error: {e}");
                AuthError::DbTimeout
            })?;

        let parent_available_for_children =
            (parent.budget_allocated - other_children_alloc - parent_used).max(rust_decimal::Decimal::ZERO);

        if req.budget_allocated > parent_available_for_children.to_string().parse::<f32>().unwrap() {
            // use your existing 400 mapping; replace with your project’s exact variant if needed
            return Err(AuthError::BudgetExceedsParentAvailable);
        }
    }

    // Use a SeaQuery UPDATE to avoid decoding the ltree column on write.
    let updated_at = chrono::Utc::now();
    let stmt = SqlQuery::update()
        .table(departments::Entity)
        .values([
            (
                departments::Column::BudgetAllocated,
                Decimal::from_f32_retain(req.budget_allocated).unwrap().into(),
            ),
            (departments::Column::BudgetPeriod, req.budget_period.clone().into()),
            (departments::Column::ActionOnExceed, req.action_on_exceed.clone().into()),
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
            eprintln!("update department budget error: {e}");
            AuthError::DbTimeout
        })?;

    // Re-read using the custom row parser so `path::text` decodes safely.
    let updated = departments_budget_select()
        .filter(departments::Column::Id.eq(department_id))
        .into_model::<DepartmentBudgetRow>()
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("reload department budget error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    // computed response fields
    let budget_distributed = sum_child_allocations(&app_state.database, updated.id, None)
        .await
        .map_err(|e| {
            eprintln!("sum distributed error: {e}");
            AuthError::DbTimeout
        })?;

    let budget_available = (updated.budget_allocated - budget_distributed).max(rust_decimal::Decimal::ZERO);

    let now = chrono::Utc::now();
    let (start, end) = period_bounds(&updated.budget_period, now);
    let budget_used = sum_department_cost_in_range(&app_state.database, updated.id, start, end)
        .await
        .map_err(|e| {
            eprintln!("sum used error: {e}");
            AuthError::DbTimeout
        })?;

    Ok((
        StatusCode::OK,
        Json(DepartmentBudgetUpdatedDto {
            id: updated.id,
            name: updated.name,
            description: updated.description,
            parent_id: updated.parent_id,
            path: updated.path,
            depth: updated.depth,
            budget_allocated: updated.budget_allocated.to_string().parse().unwrap(),
            budget_distributed:budget_distributed.to_string().parse().unwrap(),
            budget_available:budget_available.to_string().parse().unwrap(),
            budget_used:budget_used.to_string().parse().unwrap(),
            budget_period: updated.budget_period,
            created_at: updated.created_at,
            updated_at: updated.updated_at,
        }),
    ))
}
