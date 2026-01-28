use chrono::{DateTime, Datelike, TimeZone, Utc, Weekday};
use rust_decimal::Decimal;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, JoinType, QueryFilter, QuerySelect, RelationTrait, sea_query::Expr
};
use uuid::Uuid;
use crate::models::{conversations, departments, messages, users};

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
    let mut q = departments::Entity::find()
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

