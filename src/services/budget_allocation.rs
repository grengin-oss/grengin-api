// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    handlers::admin_department_budgets::departments_budget_select,
    models::{conversations, departments, messages, users},
};
use chrono::{DateTime, Datelike, TimeZone, Utc, Weekday};
use rust_decimal::Decimal;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, FromQueryResult, JoinType, QueryFilter,
    QuerySelect, RelationTrait, sea_query::Expr,
};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetHealth {
    Unlimited,
    Healthy,
    Low,
    Exhausted,
}

impl BudgetHealth {
    pub fn classify(allocated: Decimal, available: Decimal) -> Self {
        // budgetAllocated is NOT NULL DEFAULT 0, so zero is how "no budget configured" is stored.
        if allocated <= Decimal::ZERO {
            Self::Unlimited
        } else if available <= Decimal::ZERO {
            Self::Exhausted
        } else if available <= allocated * Decimal::new(2, 1) {
            Self::Low
        } else {
            Self::Healthy
        }
    }
}

pub fn period_bounds(
    period: &departments::BudgetPeriod,
    now: DateTime<Utc>,
) -> (DateTime<Utc>, DateTime<Utc>) {
    match period {
        departments::BudgetPeriod::Daily => {
            let start = Utc
                .with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
                .unwrap();
            (start, start + chrono::Duration::days(1))
        }
        departments::BudgetPeriod::Weekly => {
            let today0 = Utc
                .with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
                .unwrap();
            let days_from_monday = match today0.weekday() {
                Weekday::Mon => 0,
                Weekday::Tue => 1,
                Weekday::Wed => 2,
                Weekday::Thu => 3,
                Weekday::Fri => 4,
                Weekday::Sat => 5,
                Weekday::Sun => 6,
            };
            let start = today0 - chrono::Duration::days(days_from_monday);
            (start, start + chrono::Duration::days(7))
        }
        departments::BudgetPeriod::Monthly => {
            let start = Utc
                .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
                .unwrap();
            let (ny, nm) = if now.month() == 12 {
                (now.year() + 1, 1)
            } else {
                (now.year(), now.month() + 1)
            };
            let end = Utc.with_ymd_and_hms(ny, nm, 1, 0, 0, 0).unwrap();
            (start, end)
        }
        departments::BudgetPeriod::Yearly => {
            let start = Utc.with_ymd_and_hms(now.year(), 1, 1, 0, 0, 0).unwrap();
            let end = Utc.with_ymd_and_hms(now.year() + 1, 1, 1, 0, 0, 0).unwrap();
            (start, end)
        }
    }
}

pub async fn sum_child_allocations(
    db: &DatabaseConnection,
    parent_id: Uuid,
    exclude_child: Option<Uuid>,
) -> Result<Decimal, sea_orm::DbErr> {
    let mut q = departments_budget_select()
        .filter(departments::Column::ParentId.eq(parent_id))
        .select_only()
        .column_as(
            Expr::col(departments::Column::BudgetAllocated).sum(),
            "sum_alloc",
        );

    if let Some(excl) = exclude_child {
        q = q.filter(departments::Column::Id.ne(excl));
    }

    let sum: Option<Decimal> = q.into_tuple::<Option<Decimal>>().one(db).await?.flatten();
    Ok(sum.unwrap_or(Decimal::ZERO))
}

pub async fn sum_department_cost_in_range(
    db: &DatabaseConnection,
    dept_id: Uuid,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Decimal, sea_orm::DbErr> {
    let sum: Option<Decimal> = messages::Entity::find()
        .join(JoinType::InnerJoin, messages::Relation::Conversations.def()) // messages -> conversations
        .join(JoinType::InnerJoin, conversations::Relation::Users.def()) // conversations -> users
        .filter(users::Column::DepartmentId.eq(dept_id))
        .filter(messages::Column::Deleted.eq(false))
        .filter(messages::Column::CreatedAt.gte(start))
        .filter(messages::Column::CreatedAt.lt(end))
        .select_only()
        .column_as(Expr::col(messages::Column::Cost).sum(), "sum_cost")
        .into_tuple::<Option<Decimal>>()
        .one(db)
        .await?
        .flatten();

    Ok(sum.unwrap_or(Decimal::ZERO))
}

pub async fn sum_department_cost_total(
    db: &DatabaseConnection,
    dept_id: Uuid,
) -> Result<Decimal, sea_orm::DbErr> {
    let sum: Option<Decimal> = messages::Entity::find()
        .join(JoinType::InnerJoin, messages::Relation::Conversations.def())
        .join(JoinType::InnerJoin, conversations::Relation::Users.def())
        .filter(users::Column::DepartmentId.eq(dept_id))
        .filter(messages::Column::Deleted.eq(false))
        .select_only()
        .column_as(Expr::col(messages::Column::Cost).sum(), "sum_cost")
        .into_tuple::<Option<Decimal>>()
        .one(db)
        .await?
        .flatten();

    Ok(sum.unwrap_or(Decimal::ZERO))
}

pub async fn refresh_department_budget_available(
    db: &DatabaseConnection,
    dept_id: Uuid,
) -> Result<Decimal, sea_orm::DbErr> {
    #[derive(Debug, FromQueryResult)]
    struct DeptBudgetRow {
        #[sea_orm(from_alias = "budgetAllocated")]
        budget_allocated: Decimal,
        #[sea_orm(from_alias = "budgetPeriod")]
        budget_period: departments::BudgetPeriod,
    }

    let Some(dept) = departments::Entity::find()
        .select_only()
        .column(departments::Column::BudgetAllocated)
        .column(departments::Column::BudgetPeriod)
        .filter(departments::Column::Id.eq(dept_id))
        .into_model::<DeptBudgetRow>()
        .one(db)
        .await?
    else {
        return Ok(Decimal::ZERO);
    };

    let now = Utc::now();
    let (period_start, period_end) = period_bounds(&dept.budget_period, now);
    let budget_distributed = sum_child_allocations(db, dept_id, None).await?;
    let budget_used = sum_department_cost_in_range(db, dept_id, period_start, period_end).await?;
    let budget_available =
        (dept.budget_allocated - budget_distributed - budget_used).max(Decimal::ZERO);

    departments::Entity::update_many()
        .col_expr(
            departments::Column::BudgetAvailable,
            Expr::val(budget_available).into(),
        )
        .filter(departments::Column::Id.eq(dept_id))
        .exec(db)
        .await?;

    Ok(budget_available)
}

pub struct DepartmentBudgetStatus {
    pub allocated: Decimal,
    pub available: Decimal,
    pub action_on_exceed: departments::ActionOnExceed,
}

pub async fn get_department_budget_status(
    db: &DatabaseConnection,
    dept_id: Uuid,
) -> Result<DepartmentBudgetStatus, sea_orm::DbErr> {
    #[derive(Debug, FromQueryResult)]
    struct DeptBudgetPolicyRow {
        #[sea_orm(from_alias = "budgetAllocated")]
        budget_allocated: Decimal,
        #[sea_orm(from_alias = "budgetPeriod")]
        budget_period: departments::BudgetPeriod,
        #[sea_orm(from_alias = "actionOnExceed")]
        action_on_exceed: departments::ActionOnExceed,
    }

    let Some(dept) = departments::Entity::find()
        .select_only()
        .column(departments::Column::BudgetAllocated)
        .column(departments::Column::BudgetPeriod)
        .column(departments::Column::ActionOnExceed)
        .filter(departments::Column::Id.eq(dept_id))
        .into_model::<DeptBudgetPolicyRow>()
        .one(db)
        .await?
    else {
        return Ok(DepartmentBudgetStatus {
            allocated: Decimal::ZERO,
            available: Decimal::ZERO,
            action_on_exceed: departments::ActionOnExceed::Warn,
        });
    };

    let now = Utc::now();
    let (period_start, period_end) = period_bounds(&dept.budget_period, now);
    let budget_distributed = sum_child_allocations(db, dept_id, None).await?;
    let budget_used = sum_department_cost_in_range(db, dept_id, period_start, period_end).await?;
    let budget_available =
        (dept.budget_allocated - budget_distributed - budget_used).max(Decimal::ZERO);

    departments::Entity::update_many()
        .col_expr(
            departments::Column::BudgetAvailable,
            Expr::val(budget_available).into(),
        )
        .filter(departments::Column::Id.eq(dept_id))
        .exec(db)
        .await?;

    Ok(DepartmentBudgetStatus {
        allocated: dept.budget_allocated,
        available: budget_available,
        action_on_exceed: dept.action_on_exceed,
    })
}

#[cfg(test)]
mod tests {
    use super::BudgetHealth;
    use rust_decimal::Decimal;

    fn health(allocated: i64, available: i64) -> BudgetHealth {
        BudgetHealth::classify(Decimal::from(allocated), Decimal::from(available))
    }

    #[test]
    fn department_without_budget_is_unlimited_not_exhausted() {
        assert_eq!(health(0, 0), BudgetHealth::Unlimited);
    }

    #[test]
    fn department_without_budget_stays_unlimited_whatever_available_holds() {
        assert_eq!(health(0, 50), BudgetHealth::Unlimited);
        assert_eq!(health(-10, 0), BudgetHealth::Unlimited);
    }

    #[test]
    fn spent_budget_is_exhausted() {
        assert_eq!(health(100, 0), BudgetHealth::Exhausted);
        assert_eq!(health(100, -5), BudgetHealth::Exhausted);
    }

    #[test]
    fn budget_at_or_below_twenty_percent_is_low() {
        assert_eq!(health(100, 20), BudgetHealth::Low);
        assert_eq!(health(100, 1), BudgetHealth::Low);
    }

    #[test]
    fn budget_above_twenty_percent_is_healthy() {
        assert_eq!(health(100, 21), BudgetHealth::Healthy);
        assert_eq!(health(100, 100), BudgetHealth::Healthy);
    }
}
