use chrono::{DateTime, Datelike, TimeZone, Utc, Weekday};
use rust_decimal::Decimal;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, FromQueryResult, JoinType, QueryFilter, QuerySelect,
    RelationTrait, sea_query::Expr,
};
use uuid::Uuid;
use crate::{handlers::admin_department_budgets::departments_budget_select, models::{conversations, departments, messages, users}};

pub fn period_bounds(period: &departments::BudgetPeriod, now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    match period {
        departments::BudgetPeriod::Daily => {
            let start = Utc.with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0).unwrap();
            (start, start + chrono::Duration::days(1))
        }
        departments::BudgetPeriod::Weekly => {
            let today0 = Utc.with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0).unwrap();
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
            let start = Utc.with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0).unwrap();
            let (ny, nm) = if now.month() == 12 { (now.year() + 1, 1) } else { (now.year(), now.month() + 1) };
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
        .column_as(Expr::col(departments::Column::BudgetAllocated).sum(), "sum_alloc");

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
        .join(JoinType::InnerJoin, messages::Relation::Conversations.def())        // messages -> conversations
        .join(JoinType::InnerJoin, conversations::Relation::Users.def())          // conversations -> users
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

pub async fn sum_department_cost_total(db: &DatabaseConnection, dept_id: Uuid) -> Result<Decimal, sea_orm::DbErr> {
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
        .col_expr(departments::Column::BudgetAvailable, Expr::val(budget_available).into())
        .filter(departments::Column::Id.eq(dept_id))
        .exec(db)
        .await?;

    Ok(budget_available)
}

pub async fn get_department_budget_status(
    db: &DatabaseConnection,
    dept_id: Uuid,
) -> Result<(Decimal, departments::ActionOnExceed), sea_orm::DbErr> {
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
        return Ok((Decimal::ZERO, departments::ActionOnExceed::Warn));
    };

    let now = Utc::now();
    let (period_start, period_end) = period_bounds(&dept.budget_period, now);
    let budget_distributed = sum_child_allocations(db, dept_id, None).await?;
    let budget_used = sum_department_cost_in_range(db, dept_id, period_start, period_end).await?;
    let budget_available =
        (dept.budget_allocated - budget_distributed - budget_used).max(Decimal::ZERO);

    departments::Entity::update_many()
        .col_expr(departments::Column::BudgetAvailable, Expr::val(budget_available).into())
        .filter(departments::Column::Id.eq(dept_id))
        .exec(db)
        .await?;

    Ok((budget_available, dept.action_on_exceed))
}
