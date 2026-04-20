use std::collections::HashSet;

use chrono::Utc;
use rust_decimal::Decimal;
use sea_orm::{
    ColumnTrait, EntityTrait, JoinType, PaginatorTrait, QueryFilter, QuerySelect, RelationTrait,
    Set,
};
use serde_json::json;
use uuid::Uuid;

use crate::{
    dto::notifications::NotificationDto,
    error::AppError,
    models::{
        departments, notifications, permissions, role_permissions, roles, user_role_assignments,
        users::{self, UserStatus},
    },
    services::budget_allocation::period_bounds,
    state::SharedState,
};

const BUDGET_LOW_KIND: &str = "budget.low";
const BUDGET_EXHAUSTED_KIND: &str = "budget.exhausted";

#[derive(Clone, Debug)]
pub struct NotificationEvent {
    pub user_id: Uuid,
    pub notification: NotificationDto,
}

#[derive(Debug, sea_orm::FromQueryResult)]
struct DeptBudgetRow {
    pub name: String,
    #[sea_orm(from_alias = "budgetAllocated")]
    pub budget_allocated: Decimal,
    #[sea_orm(from_alias = "budgetAvailable")]
    pub budget_available: Decimal,
    #[sea_orm(from_alias = "budgetPeriod")]
    pub budget_period: departments::BudgetPeriod,
    #[sea_orm(from_alias = "actionOnExceed")]
    pub action_on_exceed: departments::ActionOnExceed,
}

pub fn to_notification_dto(model: &notifications::Model) -> NotificationDto {
    NotificationDto {
        id: model.id,
        department_id: model.department_id,
        kind: model.kind.clone(),
        title: model.title.clone(),
        body: model.body.clone(),
        payload: model.payload.clone(),
        period_start: model.period_start,
        created_at: model.created_at,
        read_at: model.read_at,
    }
}

pub async fn emit_budget_alerts(state: &SharedState, department_id: Uuid) -> Result<(), AppError> {
    let dept = departments::Entity::find()
        .select_only()
        .column(departments::Column::Name)
        .column(departments::Column::BudgetAllocated)
        .column(departments::Column::BudgetAvailable)
        .column(departments::Column::BudgetPeriod)
        .column(departments::Column::ActionOnExceed)
        .filter(departments::Column::Id.eq(department_id))
        .into_model::<DeptBudgetRow>()
        .one(&state.database)
        .await
        .map_err(|e| {
            eprintln!("department budget lookup error: {e}");
            AppError::DbTimeout
        })?
        .ok_or(AppError::ResourceNotFound)?;

    let budget_available = dept.budget_available;
    let budget_allocated = dept.budget_allocated;
    let kind = if budget_available <= Decimal::ZERO {
        BUDGET_EXHAUSTED_KIND
    } else {
        let low_threshold =
            budget_allocated * Decimal::from_f32_retain(0.2).unwrap_or(Decimal::ZERO);
        if budget_allocated > Decimal::ZERO && budget_available <= low_threshold {
            BUDGET_LOW_KIND
        } else {
            return Ok(());
        }
    };

    let now = Utc::now();
    let (period_start, _period_end) = period_bounds(&dept.budget_period, now);

    let existing = notifications::Entity::find()
        .filter(notifications::Column::DepartmentId.eq(department_id))
        .filter(notifications::Column::Kind.eq(kind))
        .filter(notifications::Column::PeriodStart.eq(period_start))
        .count(&state.database)
        .await
        .map_err(|e| {
            eprintln!("notification dedupe lookup error: {e}");
            AppError::DbTimeout
        })?;
    if existing > 0 {
        return Ok(());
    }

    let mut recipient_ids: HashSet<Uuid> = HashSet::new();

    let department_members = users::Entity::find()
        .select_only()
        .column(users::Column::Id)
        .filter(users::Column::DepartmentId.eq(department_id))
        .filter(users::Column::Status.eq(UserStatus::Active))
        .into_tuple::<Uuid>()
        .all(&state.database)
        .await
        .map_err(|e| {
            eprintln!("department members lookup error: {e}");
            AppError::DbTimeout
        })?;
    recipient_ids.extend(department_members);

    let orgwide_view_users = user_role_assignments::Entity::find()
        .select_only()
        .column(user_role_assignments::Column::UserId)
        .join(
            JoinType::InnerJoin,
            user_role_assignments::Relation::Users.def(),
        )
        .join(
            JoinType::InnerJoin,
            user_role_assignments::Relation::Roles.def(),
        )
        .join(JoinType::InnerJoin, roles::Relation::RolePermissions.def())
        .join(
            JoinType::InnerJoin,
            role_permissions::Relation::Permissions.def(),
        )
        .filter(user_role_assignments::Column::ScopeDepartmentId.is_null())
        .filter(permissions::Column::Domain.eq("departments"))
        .filter(permissions::Column::Action.eq("view"))
        .filter(users::Column::Status.eq(UserStatus::Active))
        .into_tuple::<Uuid>()
        .all(&state.database)
        .await
        .map_err(|e| {
            eprintln!("org-wide departments:view lookup error: {e}");
            AppError::DbTimeout
        })?;
    recipient_ids.extend(orgwide_view_users);

    if recipient_ids.is_empty() {
        return Ok(());
    }

    let (title, body) = match kind {
        BUDGET_EXHAUSTED_KIND => (
            format!("Budget exhausted for {}", dept.name),
            "Department budget is exhausted for the current period.".to_string(),
        ),
        _ => (
            format!("Budget low for {}", dept.name),
            "Department budget has fallen below 20% for the current period.".to_string(),
        ),
    };

    let payload = json!({
        "department_id": department_id,
        "budget_allocated": budget_allocated.to_string(),
        "budget_available": budget_available.to_string(),
        "budget_period": dept.budget_period,
        "action_on_exceed": dept.action_on_exceed,
        "kind": kind,
    });

    let created_at = Utc::now();
    let mut inserts = Vec::new();
    let mut events = Vec::new();

    for user_id in recipient_ids {
        let id = Uuid::new_v4();
        inserts.push(notifications::ActiveModel {
            id: Set(id),
            user_id: Set(user_id),
            department_id: Set(Some(department_id)),
            kind: Set(kind.to_string()),
            title: Set(title.clone()),
            body: Set(body.clone()),
            payload: Set(payload.clone()),
            period_start: Set(period_start),
            created_at: Set(created_at),
            read_at: Set(None),
        });
        events.push(NotificationEvent {
            user_id,
            notification: NotificationDto {
                id,
                department_id: Some(department_id),
                kind: kind.to_string(),
                title: title.clone(),
                body: body.clone(),
                payload: payload.clone(),
                period_start,
                created_at,
                read_at: None,
            },
        });
    }

    notifications::Entity::insert_many(inserts)
        .exec(&state.database)
        .await
        .map_err(|e| {
            eprintln!("notification insert error: {e}");
            AppError::DbTimeout
        })?;

    for event in events {
        let _ = state.notification_hub.send(event);
    }

    Ok(())
}
