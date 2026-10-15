// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::fmt;

use rust_decimal::{Decimal, RoundingStrategy};
use sea_orm::{DatabaseConnection, EntityTrait, QuerySelect};
use uuid::Uuid;

use crate::{
    auth::{
        error::AuthError,
        permissions::{PERMISSION_BUDGET_ALLOCATE, PERMISSION_DEPARTMENTS_MANAGE},
    },
    dto::admin_department::DepartmentChangeDeniedPayload,
    models::departments::{self, ActionOnExceed, BudgetPeriod},
    services::{
        auth_audit::{build_audit_payload, record_auth_event},
        permission_grants::{
            DepartmentScope, PermissionGrants, load_department_path, load_user_department_paths,
        },
    },
};

const BUDGET_STORAGE_SCALE: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepartmentChangeDenial {
    DepartmentOutsideScope,
    MemberOutsideScope,
    NewParentOutsideScope,
    BudgetAllocatorOutsideScope,
}

impl fmt::Display for DepartmentChangeDenial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            Self::DepartmentOutsideScope => "department_outside_scope",
            Self::MemberOutsideScope => "member_outside_scope",
            Self::NewParentOutsideScope => "new_parent_outside_scope",
            Self::BudgetAllocatorOutsideScope => "budget_allocator_outside_scope",
        };
        f.write_str(reason)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetSettings {
    pub allocated: Decimal,
    pub period: BudgetPeriod,
    pub action_on_exceed: ActionOnExceed,
}

pub fn check_member_move(
    grants: &PermissionGrants,
    target: DepartmentScope<'_>,
    sources: &[DepartmentScope<'_>],
) -> Result<(), DepartmentChangeDenial> {
    if !grants.holds(PERMISSION_DEPARTMENTS_MANAGE, target) {
        return Err(DepartmentChangeDenial::DepartmentOutsideScope);
    }
    if sources
        .iter()
        .any(|source| !grants.holds(PERMISSION_DEPARTMENTS_MANAGE, *source))
    {
        return Err(DepartmentChangeDenial::MemberOutsideScope);
    }
    Ok(())
}

pub fn check_reparent(
    grants: &PermissionGrants,
    department: DepartmentScope<'_>,
    new_parent: DepartmentScope<'_>,
) -> Result<(), DepartmentChangeDenial> {
    if !grants.holds(PERMISSION_DEPARTMENTS_MANAGE, department) {
        return Err(DepartmentChangeDenial::DepartmentOutsideScope);
    }
    if !grants.holds(PERMISSION_DEPARTMENTS_MANAGE, new_parent) {
        return Err(DepartmentChangeDenial::NewParentOutsideScope);
    }
    Ok(())
}

pub fn check_budget_change(
    grants: &PermissionGrants,
    allocator: DepartmentScope<'_>,
) -> Result<(), DepartmentChangeDenial> {
    if grants.holds(PERMISSION_BUDGET_ALLOCATE, allocator) {
        Ok(())
    } else {
        Err(DepartmentChangeDenial::BudgetAllocatorOutsideScope)
    }
}

pub fn budget_change_requires_allocate(
    current: &BudgetSettings,
    updated: &BudgetSettings,
    parent_changed: bool,
) -> bool {
    let stored_allocation = updated
        .allocated
        .round_dp_with_strategy(BUDGET_STORAGE_SCALE, RoundingStrategy::MidpointAwayFromZero);
    stored_allocation != current.allocated
        || updated.period != current.period
        || updated.action_on_exceed != current.action_on_exceed
        || (parent_changed && stored_allocation > Decimal::ZERO)
}

pub async fn ensure_member_move_allowed(
    db: &DatabaseConnection,
    actor_id: Uuid,
    target_department_id: Uuid,
    user_ids: &[Uuid],
) -> Result<(), AuthError> {
    let grants = PermissionGrants::load(db, actor_id).await?;
    let target_path = load_department_path(db, Some(target_department_id)).await?;
    let user_paths = load_user_department_paths(db, user_ids).await?;
    let sources: Vec<DepartmentScope<'_>> = user_paths
        .values()
        .map(|path| DepartmentScope::from_path(path.as_deref()))
        .collect();
    let result = check_member_move(
        &grants,
        DepartmentScope::from_path(target_path.as_deref()),
        &sources,
    );
    finish(db, actor_id, Some(target_department_id), result).await
}

pub async fn ensure_member_release_to_parent_allowed(
    db: &DatabaseConnection,
    actor_id: Uuid,
    department_id: Uuid,
) -> Result<(), AuthError> {
    let parent_id = departments::Entity::find_by_id(department_id)
        .select_only()
        .column(departments::Column::ParentId)
        .into_tuple::<Option<Uuid>>()
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("department parent lookup error: {e}");
            AuthError::DbTimeout
        })?
        .flatten();
    let Some(parent_id) = parent_id else {
        return Ok(());
    };
    let grants = PermissionGrants::load(db, actor_id).await?;
    let parent_path = load_department_path(db, Some(parent_id)).await?;
    let department_path = load_department_path(db, Some(department_id)).await?;
    let result = check_member_move(
        &grants,
        DepartmentScope::from_path(parent_path.as_deref()),
        &[DepartmentScope::from_path(department_path.as_deref())],
    );
    finish(db, actor_id, Some(parent_id), result).await
}

pub async fn ensure_reparent_allowed(
    db: &DatabaseConnection,
    actor_id: Uuid,
    department_id: Uuid,
    new_parent_id: Option<Uuid>,
) -> Result<(), AuthError> {
    let grants = PermissionGrants::load(db, actor_id).await?;
    let department_path = load_department_path(db, Some(department_id)).await?;
    let parent_path = load_department_path(db, new_parent_id).await?;
    let result = check_reparent(
        &grants,
        DepartmentScope::from_path(department_path.as_deref()),
        DepartmentScope::from_path(parent_path.as_deref()),
    );
    finish(db, actor_id, Some(department_id), result).await
}

pub async fn ensure_budget_change_allowed(
    db: &DatabaseConnection,
    actor_id: Uuid,
    department_id: Uuid,
    allocator_department_id: Option<Uuid>,
) -> Result<(), AuthError> {
    let grants = PermissionGrants::load(db, actor_id).await?;
    let allocator_path = load_department_path(db, allocator_department_id).await?;
    let result = check_budget_change(
        &grants,
        DepartmentScope::from_path(allocator_path.as_deref()),
    );
    finish(db, actor_id, Some(department_id), result).await
}

async fn finish(
    db: &DatabaseConnection,
    actor_id: Uuid,
    department_id: Option<Uuid>,
    result: Result<(), DepartmentChangeDenial>,
) -> Result<(), AuthError> {
    let Err(denial) = result else {
        return Ok(());
    };
    if let Some(payload) = build_audit_payload(DepartmentChangeDeniedPayload {
        reason: denial.to_string(),
        department_id,
    }) {
        let _ = record_auth_event(db, "auth.permission_denied", Some(actor_id), payload).await;
    }
    Err(AuthError::PermissionDenied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::permission_grants::tests::grant;

    fn department_admin(scope: &str) -> PermissionGrants {
        PermissionGrants::from_rows(
            &[],
            vec![
                grant("departments:manage", true, Some(scope)),
                grant("budget:allocate", true, Some(scope)),
            ],
        )
    }

    fn org_wide_admin() -> PermissionGrants {
        PermissionGrants::from_rows(
            &[],
            vec![
                grant("departments:manage", true, None),
                grant("budget:allocate", true, None),
            ],
        )
    }

    fn settings(allocated: Decimal) -> BudgetSettings {
        BudgetSettings {
            allocated,
            period: BudgetPeriod::Monthly,
            action_on_exceed: ActionOnExceed::Block,
        }
    }

    #[test]
    fn scoped_admin_can_move_member_between_departments_in_scope() {
        let result = check_member_move(
            &department_admin("a.b"),
            DepartmentScope::Department("a.b.c"),
            &[
                DepartmentScope::Department("a.b"),
                DepartmentScope::Department("a.b.d"),
            ],
        );
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn scoped_admin_cannot_pull_user_from_outside_scope() {
        let result = check_member_move(
            &department_admin("a.b"),
            DepartmentScope::Department("a.b"),
            &[
                DepartmentScope::Department("a.b.c"),
                DepartmentScope::Department("a.x"),
            ],
        );
        assert_eq!(result, Err(DepartmentChangeDenial::MemberOutsideScope));
    }

    #[test]
    fn scoped_admin_cannot_pull_unassigned_user() {
        let result = check_member_move(
            &department_admin("a.b"),
            DepartmentScope::Department("a.b"),
            &[DepartmentScope::OrgWide],
        );
        assert_eq!(result, Err(DepartmentChangeDenial::MemberOutsideScope));
    }

    #[test]
    fn scoped_admin_cannot_push_members_into_department_outside_scope() {
        let result = check_member_move(
            &department_admin("a.b"),
            DepartmentScope::Department("a"),
            &[DepartmentScope::Department("a.b")],
        );
        assert_eq!(result, Err(DepartmentChangeDenial::DepartmentOutsideScope));
    }

    #[test]
    fn org_wide_admin_can_move_any_user_including_unassigned() {
        let result = check_member_move(
            &org_wide_admin(),
            DepartmentScope::Department("z"),
            &[DepartmentScope::OrgWide, DepartmentScope::Department("a.b")],
        );
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn scoped_admin_can_reparent_within_scope() {
        let result = check_reparent(
            &department_admin("a"),
            DepartmentScope::Department("a.b.c"),
            DepartmentScope::Department("a.d"),
        );
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn scoped_admin_cannot_reparent_under_department_outside_scope() {
        let result = check_reparent(
            &department_admin("a.b"),
            DepartmentScope::Department("a.b.c"),
            DepartmentScope::Department("a.x"),
        );
        assert_eq!(result, Err(DepartmentChangeDenial::NewParentOutsideScope));
    }

    #[test]
    fn scoped_admin_cannot_move_department_to_top_level() {
        let result = check_reparent(
            &department_admin("a.b"),
            DepartmentScope::Department("a.b.c"),
            DepartmentScope::OrgWide,
        );
        assert_eq!(result, Err(DepartmentChangeDenial::NewParentOutsideScope));
    }

    #[test]
    fn scoped_admin_cannot_reparent_department_outside_scope() {
        let result = check_reparent(
            &department_admin("a.b"),
            DepartmentScope::Department("a.c"),
            DepartmentScope::Department("a.b"),
        );
        assert_eq!(result, Err(DepartmentChangeDenial::DepartmentOutsideScope));
    }

    #[test]
    fn org_wide_admin_can_reparent_anywhere() {
        let result = check_reparent(
            &org_wide_admin(),
            DepartmentScope::Department("a.b"),
            DepartmentScope::OrgWide,
        );
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn scoped_admin_can_change_child_budget_but_not_own() {
        let admin = department_admin("a.b");
        assert_eq!(
            check_budget_change(&admin, DepartmentScope::Department("a.b")),
            Ok(())
        );
        assert_eq!(
            check_budget_change(&admin, DepartmentScope::Department("a")),
            Err(DepartmentChangeDenial::BudgetAllocatorOutsideScope)
        );
    }

    #[test]
    fn top_level_budget_change_needs_org_wide_allocate() {
        assert_eq!(
            check_budget_change(&department_admin("a"), DepartmentScope::OrgWide),
            Err(DepartmentChangeDenial::BudgetAllocatorOutsideScope)
        );
        assert_eq!(
            check_budget_change(&org_wide_admin(), DepartmentScope::OrgWide),
            Ok(())
        );
    }

    #[test]
    fn departments_manage_without_budget_allocate_cannot_change_budget() {
        let grants =
            PermissionGrants::from_rows(&[], vec![grant("departments:manage", true, None)]);
        assert_eq!(
            check_budget_change(&grants, DepartmentScope::Department("a")),
            Err(DepartmentChangeDenial::BudgetAllocatorOutsideScope)
        );
    }

    #[test]
    fn unchanged_budget_fields_do_not_require_allocate() {
        let current = settings(Decimal::new(123456, 2));
        let resent = settings(Decimal::from_f32_retain(1234.56).unwrap_or_default());
        assert!(!budget_change_requires_allocate(&current, &resent, false));
    }

    #[test]
    fn budget_amount_period_or_mode_change_requires_allocate() {
        let current = settings(Decimal::new(10000, 2));
        assert!(budget_change_requires_allocate(
            &current,
            &settings(Decimal::new(20000, 2)),
            false
        ));

        let mut period = current;
        period.period = BudgetPeriod::Yearly;
        assert!(budget_change_requires_allocate(&current, &period, false));

        let mut mode = current;
        mode.action_on_exceed = ActionOnExceed::Warn;
        assert!(budget_change_requires_allocate(&current, &mode, false));
    }

    #[test]
    fn moving_funded_department_requires_allocate_at_new_parent() {
        let funded = settings(Decimal::new(500, 0));
        assert!(budget_change_requires_allocate(&funded, &funded, true));

        let unfunded = settings(Decimal::ZERO);
        assert!(!budget_change_requires_allocate(&unfunded, &unfunded, true));
    }

    #[test]
    fn department_change_denials_are_stable_audit_strings() {
        assert_eq!(
            DepartmentChangeDenial::MemberOutsideScope.to_string(),
            "member_outside_scope"
        );
        assert_eq!(
            DepartmentChangeDenial::BudgetAllocatorOutsideScope.to_string(),
            "budget_allocator_outside_scope"
        );
    }
}
