// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::error::AuthError,
    handlers::admin_department_budgets::departments_budget_select,
    models::{conversations, departments, messages, users},
};
use chrono::{DateTime, Datelike, TimeZone, Utc, Weekday};
use rust_decimal::Decimal;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, FromQueryResult, JoinType, QueryFilter,
    QuerySelect, RelationTrait, sea_query::Expr,
};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

struct BudgetNode {
    parent: Option<Uuid>,
    allocated: Decimal,
}

pub async fn budget_owner_id(db: &DatabaseConnection, id: Uuid) -> Result<Uuid, sea_orm::DbErr> {
    effective_budget_owner(&load_budget_nodes(db).await?, id)
}

async fn load_budget_nodes(
    db: &DatabaseConnection,
) -> Result<HashMap<Uuid, BudgetNode>, sea_orm::DbErr> {
    let rows = departments::Entity::find()
        .select_only()
        .column(departments::Column::Id)
        .column(departments::Column::ParentId)
        .column(departments::Column::BudgetAllocated)
        .into_tuple::<(Uuid, Option<Uuid>, Decimal)>()
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(id, parent, allocated)| (id, BudgetNode { parent, allocated }))
        .collect())
}

fn effective_budget_owner(
    nodes: &HashMap<Uuid, BudgetNode>,
    mut id: Uuid,
) -> Result<Uuid, sea_orm::DbErr> {
    let mut seen = HashSet::new();
    loop {
        if !seen.insert(id) {
            return Err(sea_orm::DbErr::Custom(
                "cyclic department budget hierarchy".into(),
            ));
        }
        let node = nodes
            .get(&id)
            .ok_or_else(|| sea_orm::DbErr::Custom("department budget not found".into()))?;
        if node.allocated > Decimal::ZERO {
            return Ok(id);
        }
        match node.parent {
            Some(parent) => id = parent,
            None => return Ok(id),
        }
    }
}

fn budget_spending_members(nodes: &HashMap<Uuid, BudgetNode>, root: Uuid) -> Vec<Uuid> {
    let mut included = HashSet::from([root]);
    let mut pending = vec![root];
    while let Some(parent) = pending.pop() {
        for (&id, node) in nodes {
            // A funded child spends its reserved allocation; unfunded children share this budget.
            if node.parent == Some(parent) && node.allocated <= Decimal::ZERO && included.insert(id)
            {
                pending.push(id);
            }
        }
    }
    included.into_iter().collect()
}

// A negative allocation would be counted as funds handed back to the parent, raising what the
// parent and its other children can spend and lifting a block on the parent.
pub fn requested_budget_allocation(value: f32) -> Result<Decimal, AuthError> {
    if value < 0.0 {
        return Err(AuthError::InvalidRequest {
            field: "budget_allocated",
        });
    }
    Decimal::from_f32_retain(value).ok_or(AuthError::ServiceTemporarilyUnavailable)
}

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
    let members = budget_spending_members(&load_budget_nodes(db).await?, dept_id);
    sum_spending_cost(db, members, start, end).await
}

pub async fn sum_parent_cost_after_allocation(
    db: &DatabaseConnection,
    parent: Uuid,
    child: Uuid,
    allocated: Decimal,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Decimal, sea_orm::DbErr> {
    let mut nodes = load_budget_nodes(db).await?;
    let members = spending_members_after_allocation(&mut nodes, parent, child, allocated);
    sum_spending_cost(db, members, start, end).await
}

fn spending_members_after_allocation(
    nodes: &mut HashMap<Uuid, BudgetNode>,
    parent: Uuid,
    child: Uuid,
    allocated: Decimal,
) -> Vec<Uuid> {
    nodes.insert(
        child,
        BudgetNode {
            parent: Some(parent),
            allocated,
        },
    );
    budget_spending_members(nodes, parent)
}

async fn sum_spending_cost(
    db: &DatabaseConnection,
    members: Vec<Uuid>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Decimal, sea_orm::DbErr> {
    let sum: Option<Decimal> = messages::Entity::find()
        .join(JoinType::InnerJoin, messages::Relation::Conversations.def()) // messages -> conversations
        .join(JoinType::InnerJoin, conversations::Relation::Users.def()) // conversations -> users
        .filter(users::Column::DepartmentId.is_in(members))
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
    let dept_id = budget_owner_id(db, dept_id).await?;
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
    let dept_id = budget_owner_id(db, dept_id).await?;
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
    use super::{
        BudgetHealth, BudgetNode, budget_spending_members, effective_budget_owner,
        requested_budget_allocation,
    };
    use crate::auth::error::AuthError;
    use rust_decimal::Decimal;
    use std::collections::{HashMap, HashSet};
    use uuid::Uuid;

    #[test]
    fn unfunded_children_inherit_the_nearest_funded_ancestor() {
        let root = Uuid::new_v4();
        let child = Uuid::new_v4();
        let grandchild = Uuid::new_v4();
        let funded = Uuid::new_v4();
        let nodes = HashMap::from([
            (
                root,
                BudgetNode {
                    parent: None,
                    allocated: Decimal::from(100),
                },
            ),
            (
                child,
                BudgetNode {
                    parent: Some(root),
                    allocated: Decimal::ZERO,
                },
            ),
            (
                grandchild,
                BudgetNode {
                    parent: Some(child),
                    allocated: Decimal::ZERO,
                },
            ),
            (
                funded,
                BudgetNode {
                    parent: Some(root),
                    allocated: Decimal::from(20),
                },
            ),
        ]);
        assert_eq!(effective_budget_owner(&nodes, grandchild).unwrap(), root);
        assert_eq!(effective_budget_owner(&nodes, funded).unwrap(), funded);
        let members = budget_spending_members(&nodes, root)
            .into_iter()
            .collect::<HashSet<_>>();
        assert_eq!(members, HashSet::from([root, child, grandchild]));
    }

    #[test]
    fn entirely_unbudgeted_trees_remain_unlimited_and_cycles_fail_closed() {
        let root = Uuid::new_v4();
        let child = Uuid::new_v4();
        let mut nodes = HashMap::from([
            (
                root,
                BudgetNode {
                    parent: None,
                    allocated: Decimal::ZERO,
                },
            ),
            (
                child,
                BudgetNode {
                    parent: Some(root),
                    allocated: Decimal::ZERO,
                },
            ),
        ]);
        assert_eq!(effective_budget_owner(&nodes, child).unwrap(), root);
        nodes.get_mut(&root).unwrap().parent = Some(child);
        assert!(effective_budget_owner(&nodes, child).is_err());
        assert!(effective_budget_owner(&nodes, Uuid::new_v4()).is_err());
    }

    #[test]
    fn funding_a_child_moves_its_existing_usage_out_of_the_parents_spend() {
        let parent = Uuid::new_v4();
        let child = Uuid::new_v4();
        let mut nodes = HashMap::from([
            (
                parent,
                BudgetNode {
                    parent: None,
                    allocated: Decimal::from(100),
                },
            ),
            (
                child,
                BudgetNode {
                    parent: Some(parent),
                    allocated: Decimal::ZERO,
                },
            ),
        ]);
        assert_eq!(budget_spending_members(&nodes, parent).len(), 2);
        assert_eq!(
            super::spending_members_after_allocation(&mut nodes, parent, child, Decimal::from(80)),
            vec![parent]
        );
        assert_eq!(
            super::spending_members_after_allocation(&mut nodes, parent, child, Decimal::ZERO)
                .len(),
            2
        );
    }

    #[test]
    fn negative_budget_allocations_are_rejected() {
        assert!(matches!(
            requested_budget_allocation(-100.0),
            Err(AuthError::InvalidRequest {
                field: "budget_allocated"
            })
        ));
        assert!(requested_budget_allocation(-0.01).is_err());
    }

    #[test]
    fn zero_and_positive_budget_allocations_are_kept() {
        assert_eq!(requested_budget_allocation(0.0).ok(), Some(Decimal::ZERO));
        assert_eq!(
            requested_budget_allocation(12.5).ok(),
            Some(Decimal::new(125, 1))
        );
    }

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
