// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::{
        error::AuthError,
        permissions::{PERMISSION_ROLES_ASSIGN, ROLE_DEPARTMENT_ADMIN},
    },
    dto::admin_department::{DepartmentModelKey, RoleAssignmentPayload},
    models::{
        department_allowed_models,
        departments::{self, BudgetPeriod},
        roles, user_role_assignments, users,
    },
    services::{
        auth_audit::{build_audit_payload, record_auth_event},
        authorization::{AuthorizationService, PermissionScopeMode},
        budget_allocation::{period_bounds, sum_child_allocations, sum_department_cost_in_range},
    },
    utils::ltree::ltree_label_from_uuid,
};
use chrono::Utc;
use migration::{Alias, BinOper, Func};
use rust_decimal::Decimal;
use sea_orm::{
    ActiveModelTrait,
    ActiveValue::Set,
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect,
    sea_query::Expr,
    sqlx::postgres::types::{PgLTree, PgLTreeLabel},
};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

pub fn build_ltree_path(parent_path: Option<&str>, id: Uuid) -> Result<String, AuthError> {
    let mut tree = if let Some(p) = parent_path {
        p.parse::<PgLTree>()
            .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?
    } else {
        PgLTree::new()
    };

    let label_str = ltree_label_from_uuid(id);
    let label =
        PgLTreeLabel::new(label_str).map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    tree.push(label);

    Ok(tree.to_string())
}

pub async fn max_subtree_depth(db: &DatabaseConnection, path: &str) -> Result<i32, AuthError> {
    let row = departments::Entity::find()
        .select_only()
        .expr_as(
            Func::max(Expr::col(departments::Column::Depth)),
            "max_depth",
        )
        .filter(Expr::col(departments::Column::Path).binary(
            BinOper::Custom("<@".into()),
            Expr::val(path.to_string()).cast_as(Alias::new("ltree")),
        ))
        .into_tuple::<(Option<i32>,)>()
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("subtree depth query error: {e}");
            AuthError::DbTimeout
        })?;

    Ok(row.and_then(|(value,)| value).unwrap_or(0))
}

pub async fn sync_department_admin_assignments(
    authz: &AuthorizationService<'_>,
    db: &DatabaseConnection,
    actor_id: Uuid,
    department_id: Uuid,
    department_admin_ids: &[Uuid],
    permission_scope: Option<Uuid>,
) -> Result<(), AuthError> {
    authz
        .ensure_permission(
            actor_id,
            PERMISSION_ROLES_ASSIGN,
            permission_scope,
            PermissionScopeMode::RequireOrgWide,
            Some(department_id),
        )
        .await?;

    let role = roles::Entity::find()
        .filter(roles::Column::Name.eq(ROLE_DEPARTMENT_ADMIN))
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("role lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    let desired_ids: HashSet<Uuid> = department_admin_ids.iter().copied().collect();

    if !desired_ids.is_empty() {
        let existing_users = users::Entity::find()
            .select_only()
            .column(users::Column::Id)
            .filter(users::Column::Id.is_in(desired_ids.iter().copied()))
            .into_tuple::<Uuid>()
            .all(db)
            .await
            .map_err(|e| {
                eprintln!("user lookup error: {e}");
                AuthError::DbTimeout
            })?;

        if existing_users.len() != desired_ids.len() {
            return Err(AuthError::ResourceNotFound);
        }
    }

    let existing_assignments = user_role_assignments::Entity::find()
        .filter(user_role_assignments::Column::RoleId.eq(role.id))
        .filter(user_role_assignments::Column::ScopeDepartmentId.eq(department_id))
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("role assignment lookup error: {e}");
            AuthError::DbTimeout
        })?;

    let existing_ids: HashSet<Uuid> = existing_assignments
        .iter()
        .map(|assignment| assignment.user_id)
        .collect();

    let to_add: Vec<Uuid> = desired_ids.difference(&existing_ids).copied().collect();
    let to_remove: Vec<user_role_assignments::Model> = existing_assignments
        .into_iter()
        .filter(|assignment| !desired_ids.contains(&assignment.user_id))
        .collect();

    let now = Utc::now();
    let mut affected_users: HashSet<Uuid> = HashSet::new();

    for user_id in to_add {
        let assignment_id = Uuid::new_v4();
        let assignment = user_role_assignments::ActiveModel {
            id: Set(assignment_id),
            user_id: Set(user_id),
            role_id: Set(role.id),
            scope_department_id: Set(Some(department_id)),
            assigned_by: Set(actor_id),
            created_at: Set(now),
            updated_at: Set(now),
        };

        assignment.insert(db).await.map_err(|e| {
            let s = e.to_string();
            if s.contains("duplicate key value violates unique constraint") {
                AuthError::DbConflict
            } else {
                eprintln!("role assignment insert error: {e}");
                AuthError::DbTimeout
            }
        })?;

        if let Some(payload) = build_audit_payload(RoleAssignmentPayload {
            assignment_id,
            user_id,
            role_id: role.id,
            scope_department_id: Some(department_id),
        }) {
            let _ = record_auth_event(db, "auth.role_assigned", Some(actor_id), payload).await;
        }

        affected_users.insert(user_id);
    }

    for assignment in to_remove {
        let assignment_id = assignment.id;
        let user_id = assignment.user_id;

        user_role_assignments::Entity::delete_by_id(assignment_id)
            .exec(db)
            .await
            .map_err(|e| {
                eprintln!("role assignment delete error: {e}");
                AuthError::DbTimeout
            })?;

        if let Some(payload) = build_audit_payload(RoleAssignmentPayload {
            assignment_id,
            user_id,
            role_id: role.id,
            scope_department_id: Some(department_id),
        }) {
            let _ = record_auth_event(db, "auth.role_unassigned", Some(actor_id), payload).await;
        }

        affected_users.insert(user_id);
    }

    if !affected_users.is_empty() {
        let affected: Vec<Uuid> = affected_users.into_iter().collect();
        let _ = authz
            .recompute_effective_permissions_for_users(&affected)
            .await;
    }

    Ok(())
}

pub async fn sync_department_allowed_models(
    db: &DatabaseConnection,
    department_id: Uuid,
    models: Option<&[DepartmentModelKey]>,
) -> Result<(), AuthError> {
    let Some(models) = models else {
        return Ok(());
    };

    department_allowed_models::Entity::delete_many()
        .filter(department_allowed_models::Column::DepartmentId.eq(department_id))
        .exec(db)
        .await
        .map_err(|e| {
            eprintln!("department allowed models delete error: {e}");
            AuthError::DbTimeout
        })?;

    if models.is_empty() {
        return Ok(());
    }

    let now = Utc::now();
    let inserts = models
        .iter()
        .map(|model| department_allowed_models::ActiveModel {
            id: Set(Uuid::new_v4()),
            department_id: Set(department_id),
            provider: Set(model.provider.clone()),
            model: Set(model.model.clone()),
            created_at: Set(now),
            updated_at: Set(now),
        })
        .collect::<Vec<_>>();

    department_allowed_models::Entity::insert_many(inserts)
        .exec(db)
        .await
        .map_err(|e| {
            eprintln!("department allowed models insert error: {e}");
            AuthError::DbTimeout
        })?;

    Ok(())
}

pub async fn load_department_admin_ids_map(
    db: &DatabaseConnection,
    department_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<Uuid>>, AuthError> {
    if department_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let role = roles::Entity::find()
        .filter(roles::Column::Name.eq(ROLE_DEPARTMENT_ADMIN))
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("role lookup error: {e}");
            AuthError::DbTimeout
        })?;

    let role = match role {
        Some(role) => role,
        None => return Ok(HashMap::new()),
    };

    let rows = user_role_assignments::Entity::find()
        .select_only()
        .column(user_role_assignments::Column::ScopeDepartmentId)
        .column(user_role_assignments::Column::UserId)
        .filter(user_role_assignments::Column::RoleId.eq(role.id))
        .filter(
            user_role_assignments::Column::ScopeDepartmentId.is_in(department_ids.iter().copied()),
        )
        .into_tuple::<(Option<Uuid>, Uuid)>()
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("role assignment lookup error: {e}");
            AuthError::DbTimeout
        })?;

    let mut map: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (scope_id, user_id) in rows {
        if let Some(dept_id) = scope_id {
            map.entry(dept_id).or_default().push(user_id);
        }
    }

    Ok(map)
}

pub fn departments_base_select() -> sea_orm::Select<departments::Entity> {
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
        .column(departments::Column::RetentionDays)
        .column(departments::Column::CreatedAt)
        .column(departments::Column::UpdatedAt)
}

pub fn departments_tree_select() -> sea_orm::Select<departments::Entity> {
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
        .column(departments::Column::RetentionDays)
        .column(departments::Column::CreatedAt)
        .column(departments::Column::UpdatedAt)
}

pub async fn department_budget_snapshot(
    db: &DatabaseConnection,
    department_id: Uuid,
    budget_allocated: Decimal,
    budget_period: BudgetPeriod,
) -> Result<(f64, f64, f64), AuthError> {
    let budget_distributed = sum_child_allocations(db, department_id, None)
        .await
        .map_err(|e| {
            eprintln!("sum_child_allocations error: {e}");
            AuthError::DbTimeout
        })?;

    let now = Utc::now();
    let (period_start, period_end) = period_bounds(&budget_period, now);
    let budget_used = sum_department_cost_in_range(db, department_id, period_start, period_end)
        .await
        .map_err(|e| {
            eprintln!("sum_department_cost_in_range error: {e}");
            AuthError::DbTimeout
        })?;

    let budget_available = (budget_allocated - budget_distributed - budget_used).max(Decimal::ZERO);

    Ok((
        budget_distributed.to_string().parse().unwrap_or(0.0),
        budget_available.to_string().parse().unwrap_or(0.0),
        budget_used.to_string().parse().unwrap_or(0.0),
    ))
}
