use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
        permissions::PERMISSION_BUDGET_VIEW,
    },
    dto::admin_department_budget::{DepartmentBudgetStatus, SubDepartmentBudgetDto},
    models::departments::{self, ActionOnExceed, BudgetPeriod},
    services::{
        authorization::{AuthorizationService, PermissionScopeMode},
        budget_allocation::{
            period_bounds, sum_child_allocations, sum_department_cost_in_range,
            sum_department_cost_total,
        },
    },
    state::SharedState,
};
use axum::http::StatusCode;
use axum::{
    Json,
    extract::{Path, State},
};
use chrono::{DateTime, Utc};
use migration::Expr;
use rust_decimal::Decimal;
use sea_orm::{ColumnTrait, EntityTrait, FromQueryResult, QueryFilter, QuerySelect};
use uuid::Uuid;

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

pub fn departments_budget_select() -> sea_orm::Select<departments::Entity> {
    departments::Entity::find()
        .select_only()
        .column(departments::Column::Id)
        .column(departments::Column::Name)
        .column(departments::Column::Description)
        .column(departments::Column::ParentId)
        .column(departments::Column::Depth)
        .expr_as(Expr::cust("path::text"), "path")
        .column(departments::Column::BudgetAvailable)
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
        (status = 200, content_type = "application/json", body = DepartmentBudgetStatus, description = "Budget status"),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
        (status = 403, content_type = "application/json", body = Error, description = "Forbidden - Admin role required"),
        (status = 404, content_type = "application/json", body = Error, description = "Resource not found"),
        (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)")
    )
)]
pub async fn get_department_budget(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(department_id): Path<uuid::Uuid>,
) -> Result<(StatusCode, Json<DepartmentBudgetStatus>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_BUDGET_VIEW,
            Some(department_id),
            PermissionScopeMode::RequireOrgWide,
            Some(department_id),
        )
        .await?;

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

    let budget_used =
        sum_department_cost_in_range(&app_state.database, dept.id, period_start, period_end)
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

    let budget_available =
        (dept.budget_allocated - budget_distributed - budget_used).max(rust_decimal::Decimal::ZERO);

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
        let used =
            sum_department_cost_in_range(&app_state.database, c.id, period_start, period_end)
                .await
                .map_err(|e| {
                    eprintln!("sum child dept used error: {e}");
                    AuthError::DbTimeout
                })?;

        sub_department_budgets.push(SubDepartmentBudgetDto {
            department_id: c.id,
            name: c.name,
            allocated: c.budget_allocated.to_string().parse().unwrap(),
            used: used.to_string().parse().unwrap(),
        });
    }

    Ok((
        StatusCode::OK,
        Json(DepartmentBudgetStatus {
            department_id: dept.id,
            budget_allocated: dept.budget_allocated.to_string().parse().unwrap(),
            budget_distributed: budget_distributed.to_string().parse().unwrap(),
            budget_available: budget_available.to_string().parse().unwrap(),
            budget_used: budget_used.to_string().parse().unwrap(),
            budget_used_total: budget_used_total.to_string().parse().unwrap(),
            period: dept.budget_period,
            period_start,
            period_end,
            sub_department_budgets,
        }),
    ))
}
