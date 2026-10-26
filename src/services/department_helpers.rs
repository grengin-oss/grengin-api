// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::{
        error::AuthError,
        permissions::{
            PERMISSION_DEPARTMENTS_MANAGE, PERMISSION_ROLES_ASSIGN, ROLE_DEPARTMENT_ADMIN,
        },
    },
    dto::admin_department::{DepartmentModelKey, RoleAssignmentPayload},
    models::{
        department_allowed_models,
        departments::{self, BudgetPeriod},
        roles, user_role_assignments, users,
    },
    services::{
        auth_audit::record_required_auth_event,
        authorization::{AuthorizationService, PermissionScopeMode, is_path_within_scope},
        budget_allocation::{period_bounds, sum_child_allocations, sum_department_cost_in_range},
        permission_grants::{DepartmentScope, PermissionGrants, load_department_path},
        role_assignment::{
            ensure_role_assignment_allowed, ensure_role_assignment_allowed_at,
            load_role_permission_keys,
        },
    },
    utils::ltree::ltree_label_from_uuid,
};
use chrono::{DateTime, Utc};
use migration::{Alias, BinOper};
use rust_decimal::Decimal;
use sea_orm::{
    ActiveModelTrait,
    ActiveValue::Set,
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, QueryFilter,
    QuerySelect, Statement, TransactionTrait,
    sea_query::Expr,
    sqlx::postgres::types::{PgLTree, PgLTreeLabel},
};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

const MAX_DEPARTMENT_DEPTH: i32 = 9;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepartmentNode {
    pub id: Uuid,
    pub path: String,
    pub depth: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReparentError {
    Cycle,
    TooDeep,
}

impl From<ReparentError> for AuthError {
    fn from(error: ReparentError) -> Self {
        match error {
            ReparentError::Cycle => AuthError::DbConflict,
            ReparentError::TooDeep => AuthError::ServiceTemporarilyUnavailable,
        }
    }
}

pub fn plan_department_reparent(
    department: &DepartmentNode,
    new_parent: Option<&DepartmentNode>,
    subtree: &[DepartmentNode],
) -> Result<Vec<DepartmentNode>, ReparentError> {
    let label = ltree_label_from_uuid(department.id);
    let (root_path, root_depth) = match new_parent {
        Some(parent)
            if parent.id == department.id
                || is_path_within_scope(&department.path, &parent.path) =>
        {
            return Err(ReparentError::Cycle);
        }
        Some(parent) => (format!("{}.{label}", parent.path), parent.depth + 1),
        None => (label, 0),
    };

    subtree
        .iter()
        .filter(|node| is_path_within_scope(&department.path, &node.path))
        .map(|node| {
            let depth = root_depth + (node.depth - department.depth);
            if depth > MAX_DEPARTMENT_DEPTH {
                return Err(ReparentError::TooDeep);
            }
            Ok(DepartmentNode {
                id: node.id,
                path: format!("{root_path}{}", &node.path[department.path.len()..]),
                depth,
            })
        })
        .collect()
}

async fn lock_department_nodes<C: ConnectionTrait>(
    conn: &C,
    select: sea_orm::Select<departments::Entity>,
) -> Result<Vec<DepartmentNode>, AuthError> {
    let rows = select
        .select_only()
        .column(departments::Column::Id)
        .column_as(Expr::cust("path::text"), "path")
        .column(departments::Column::Depth)
        .lock_exclusive()
        .into_tuple::<(Uuid, String, i32)>()
        .all(conn)
        .await
        .map_err(|e| {
            eprintln!("department lock error: {e}");
            AuthError::DbTimeout
        })?;
    Ok(rows
        .into_iter()
        .map(|(id, path, depth)| DepartmentNode { id, path, depth })
        .collect())
}

async fn lock_department_node<C: ConnectionTrait>(
    conn: &C,
    department_id: Uuid,
) -> Result<DepartmentNode, AuthError> {
    lock_department_nodes(
        conn,
        departments::Entity::find().filter(departments::Column::Id.eq(department_id)),
    )
    .await?
    .pop()
    .ok_or(AuthError::DbNotFound)
}

// Updates of one department queue on this lock, so each plans from what the previous one
// committed rather than from a snapshot taken before it.
pub async fn lock_department<C: ConnectionTrait>(
    conn: &C,
    department_id: Uuid,
) -> Result<(), AuthError> {
    lock_department_hierarchy(conn).await?;
    lock_department_node(conn, department_id).await.map(|_| ())
}

pub async fn lock_department_hierarchy<C: ConnectionTrait>(conn: &C) -> Result<(), AuthError> {
    // A queued request holds a pooled connection while it waits, so the wait is bounded rather
    // than letting a pile-up starve the request that holds the lock.
    conn.execute(Statement::from_string(
        DatabaseBackend::Postgres,
        DEPARTMENT_LOCK_TIMEOUT_SQL,
    ))
    .await
    .map_err(|e| {
        eprintln!("department lock timeout setting error: {e}");
        AuthError::DbTimeout
    })?;
    // SeaORM cannot express advisory locks. Serialize hierarchy mutations before reading paths;
    // row locks alone miss children inserted while an ancestor is being moved.
    conn.execute(Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT pg_advisory_xact_lock(1769084371, 1)",
    ))
    .await
    .map_err(|e| {
        eprintln!("department hierarchy lock error: {e}");
        AuthError::DbTimeout
    })?;
    Ok(())
}

// SeaORM has no typed API for transaction-scoped settings.
const DEPARTMENT_LOCK_TIMEOUT_SQL: &str = "SELECT set_config('lock_timeout', '5s', true)";

pub async fn reparent_department<C: ConnectionTrait>(
    conn: &C,
    department_id: Uuid,
    new_parent_id: Option<Uuid>,
    updated_at: DateTime<Utc>,
) -> Result<(), AuthError> {
    lock_department_hierarchy(conn).await?;
    let department = lock_department_node(conn, department_id).await?;
    let new_parent = match new_parent_id {
        Some(parent_id) => Some(lock_department_node(conn, parent_id).await?),
        None => None,
    };
    let subtree = lock_department_nodes(
        conn,
        departments::Entity::find().filter(Expr::col(departments::Column::Path).binary(
            BinOper::Custom("<@".into()),
            Expr::val(department.path.clone()).cast_as(Alias::new("ltree")),
        )),
    )
    .await?;

    let moved = plan_department_reparent(&department, new_parent.as_ref(), &subtree)?;

    departments::Entity::update_many()
        .col_expr(departments::Column::ParentId, Expr::value(new_parent_id))
        .filter(departments::Column::Id.eq(department_id))
        .exec(conn)
        .await
        .map_err(|e| {
            eprintln!("department parent update error: {e}");
            AuthError::DbTimeout
        })?;

    for node in &moved {
        departments::Entity::update_many()
            .col_expr(
                departments::Column::Path,
                Expr::val(node.path.clone()).cast_as(Alias::new("ltree")),
            )
            .col_expr(departments::Column::Depth, Expr::value(node.depth))
            .col_expr(departments::Column::UpdatedAt, Expr::value(updated_at))
            .filter(departments::Column::Id.eq(node.id))
            .exec(conn)
            .await
            .map_err(|e| {
                eprintln!("department path update error: {e}");
                AuthError::DbTimeout
            })?;
    }
    Ok(())
}

pub async fn delete_department_with_scoped_assignments(
    db: &DatabaseConnection,
    department_id: Uuid,
    actor_id: Uuid,
) -> Result<(), AuthError> {
    let txn = db.begin().await.map_err(|e| {
        eprintln!("department delete transaction error: {e}");
        AuthError::DbTimeout
    })?;
    lock_department(&txn, department_id).await?;
    AuthorizationService::new(db)
        .ensure_permission(
            actor_id,
            PERMISSION_DEPARTMENTS_MANAGE,
            Some(department_id),
            PermissionScopeMode::RequireOrgWide,
            Some(department_id),
        )
        .await?;
    let assignments = user_role_assignments::Entity::find()
        .filter(user_role_assignments::Column::ScopeDepartmentId.eq(department_id))
        .all(&txn)
        .await
        .map_err(|e| {
            eprintln!("department grants lookup error: {e}");
            AuthError::DbTimeout
        })?;
    for assignment in assignments {
        record_required_auth_event(
            &txn,
            "auth.role_unassigned",
            Some(actor_id),
            RoleAssignmentPayload {
                assignment_id: assignment.id,
                user_id: assignment.user_id,
                role_id: assignment.role_id,
                scope_department_id: assignment.scope_department_id,
            },
        )
        .await?;
    }
    user_role_assignments::Entity::delete_many()
        .filter(user_role_assignments::Column::ScopeDepartmentId.eq(department_id))
        .exec(&txn)
        .await
        .map_err(|e| {
            eprintln!("role assignment delete error: {e}");
            AuthError::DbTimeout
        })?;
    let res = departments::Entity::delete_by_id(department_id)
        .exec(&txn)
        .await
        .map_err(|e| {
            eprintln!("delete error: {e}");
            AuthError::DbTimeout
        })?;
    if res.rows_affected == 0 {
        return Err(AuthError::DbNotFound);
    }
    txn.commit().await.map_err(|e| {
        eprintln!("department delete commit error: {e}");
        AuthError::DbTimeout
    })
}

// PUT checks the new parent's budget before moving anything, so a move under the department
// itself or one of its descendants is refused here first and answers 409, as /move does.
pub async fn ensure_parent_outside_subtree(
    db: &DatabaseConnection,
    department_id: Uuid,
    new_parent_id: Option<Uuid>,
) -> Result<(), AuthError> {
    let department_path = load_department_path(db, Some(department_id)).await?;
    let parent_path = load_department_path(db, new_parent_id).await?;
    match (department_path, parent_path) {
        (Some(department), Some(parent)) if is_path_within_scope(&department, &parent) => {
            Err(ReparentError::Cycle.into())
        }
        _ => Ok(()),
    }
}

// For a department being created: it isn't visible to other connections until the creating
// transaction commits, so the checks run against its path instead of reading it back by id.
// The creator's automatic grant is planned here too, so it lands in the same transaction.
pub async fn plan_new_department_admins(
    db: &DatabaseConnection,
    actor_id: Uuid,
    department_id: Uuid,
    department_path: &str,
    department_admin_ids: &[Uuid],
    assign_creator: bool,
) -> Result<DepartmentAdminChanges, AuthError> {
    let role = department_admin_role(db).await?;
    let desired_ids: HashSet<Uuid> = department_admin_ids.iter().copied().collect();
    ensure_users_exist(db, &desired_ids).await?;
    let mut to_add: Vec<Uuid> = desired_ids.into_iter().collect();
    ensure_role_assignment_allowed_at(
        db,
        actor_id,
        &role,
        Some(department_id),
        Some(department_path),
        &to_add,
    )
    .await?;
    if assign_creator && !to_add.contains(&actor_id) {
        let grants = PermissionGrants::load(db, actor_id).await?;
        let role_permissions = load_role_permission_keys(db, role.id).await?;
        if creator_may_receive_department_admin(
            &grants,
            &role_permissions,
            DepartmentScope::Department(department_path),
        ) {
            to_add.push(actor_id);
        }
    }
    Ok(DepartmentAdminChanges {
        role_id: role.id,
        department_id,
        to_add,
        to_remove: Vec::new(),
    })
}

async fn department_admin_role(db: &DatabaseConnection) -> Result<roles::Model, AuthError> {
    roles::Entity::find()
        .filter(roles::Column::Name.eq(ROLE_DEPARTMENT_ADMIN))
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("role lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)
}

async fn ensure_users_exist(
    db: &DatabaseConnection,
    user_ids: &HashSet<Uuid>,
) -> Result<(), AuthError> {
    if user_ids.is_empty() {
        return Ok(());
    }
    let existing_users = users::Entity::find()
        .select_only()
        .column(users::Column::Id)
        .filter(users::Column::Id.is_in(user_ids.iter().copied()))
        .into_tuple::<Uuid>()
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("user lookup error: {e}");
            AuthError::DbTimeout
        })?;
    if existing_users.len() != user_ids.len() {
        return Err(AuthError::ResourceNotFound);
    }
    Ok(())
}

pub struct DepartmentAdminChanges {
    role_id: Uuid,
    department_id: Uuid,
    to_add: Vec<Uuid>,
    to_remove: Vec<user_role_assignments::Model>,
}

// Runs every check and works out the changes without writing, so a caller can refuse the
// request before committing its other updates.
pub async fn plan_department_admin_sync(
    authz: &AuthorizationService<'_>,
    db: &DatabaseConnection,
    actor_id: Uuid,
    department_id: Uuid,
    department_admin_ids: &[Uuid],
) -> Result<DepartmentAdminChanges, AuthError> {
    authz
        .ensure_permission(
            actor_id,
            PERMISSION_ROLES_ASSIGN,
            Some(department_id),
            PermissionScopeMode::RequireOrgWide,
            Some(department_id),
        )
        .await?;

    let role = department_admin_role(db).await?;
    let desired_ids: HashSet<Uuid> = department_admin_ids.iter().copied().collect();
    ensure_users_exist(db, &desired_ids).await?;

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

    let changed_users: Vec<Uuid> = to_add
        .iter()
        .copied()
        .chain(to_remove.iter().map(|assignment| assignment.user_id))
        .collect();
    ensure_role_assignment_allowed(db, actor_id, &role, Some(department_id), &changed_users)
        .await?;

    Ok(DepartmentAdminChanges {
        role_id: role.id,
        department_id,
        to_add,
        to_remove,
    })
}

impl DepartmentAdminChanges {
    // Writes inside the caller's transaction, so the grants change together with whatever else
    // that transaction holds, or not at all.
    pub async fn write<C: ConnectionTrait>(
        self,
        conn: &C,
        actor_id: Uuid,
    ) -> Result<WrittenAdminChanges, AuthError> {
        let Self {
            role_id,
            department_id,
            to_add,
            to_remove,
        } = self;
        let now = Utc::now();
        let mut written = WrittenAdminChanges::default();

        for user_id in to_add {
            let assignment_id = Uuid::new_v4();
            let assignment = user_role_assignments::ActiveModel {
                id: Set(assignment_id),
                user_id: Set(user_id),
                role_id: Set(role_id),
                scope_department_id: Set(Some(department_id)),
                assigned_by: Set(actor_id),
                created_at: Set(now),
                updated_at: Set(now),
            };

            assignment.insert(conn).await.map_err(|e| {
                let s = e.to_string();
                if s.contains("duplicate key value violates unique constraint") {
                    AuthError::DbConflict
                } else {
                    eprintln!("role assignment insert error: {e}");
                    AuthError::DbTimeout
                }
            })?;
            written
                .record(
                    conn,
                    actor_id,
                    "auth.role_assigned",
                    RoleAssignmentPayload {
                        assignment_id,
                        user_id,
                        role_id,
                        scope_department_id: Some(department_id),
                    },
                )
                .await?;
        }

        for assignment in to_remove {
            user_role_assignments::Entity::delete_by_id(assignment.id)
                .exec(conn)
                .await
                .map_err(|e| {
                    eprintln!("role assignment delete error: {e}");
                    AuthError::DbTimeout
                })?;
            written
                .record(
                    conn,
                    actor_id,
                    "auth.role_unassigned",
                    RoleAssignmentPayload {
                        assignment_id: assignment.id,
                        user_id: assignment.user_id,
                        role_id,
                        scope_department_id: Some(department_id),
                    },
                )
                .await?;
        }

        Ok(written)
    }
}

// The audit events are written in the same transaction as the grants, so a grant never commits
// without its event; the cached permissions are refreshed once it has committed.
#[derive(Default)]
pub struct WrittenAdminChanges {
    affected_users: HashSet<Uuid>,
}

impl WrittenAdminChanges {
    async fn record<C: ConnectionTrait>(
        &mut self,
        conn: &C,
        actor_id: Uuid,
        event: &str,
        payload: RoleAssignmentPayload,
    ) -> Result<(), AuthError> {
        self.affected_users.insert(payload.user_id);
        record_required_auth_event(conn, event, Some(actor_id), payload).await
    }

    pub async fn finish(self, authz: &AuthorizationService<'_>) {
        if !self.affected_users.is_empty() {
            let affected: Vec<Uuid> = self.affected_users.into_iter().collect();
            let _ = authz
                .recompute_effective_permissions_for_users(&affected)
                .await;
        }
    }
}

pub fn creator_may_receive_department_admin(
    creator: &PermissionGrants,
    role_permissions: &[String],
    department: DepartmentScope<'_>,
) -> bool {
    creator.is_super_admin() || creator.missing(role_permissions, department).is_empty()
}

pub async fn sync_department_allowed_models<C: ConnectionTrait>(
    db: &C,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        auth::permissions::ROLE_SUPER_ADMIN,
        services::permission_grants::tests::{grant, role},
    };

    fn node(id: Uuid, path: &str, depth: i32) -> DepartmentNode {
        DepartmentNode {
            id,
            path: path.to_string(),
            depth,
        }
    }

    fn label(id: Uuid) -> String {
        ltree_label_from_uuid(id)
    }

    struct Tree {
        root: DepartmentNode,
        child: DepartmentNode,
        grandchild: DepartmentNode,
        other: DepartmentNode,
    }

    fn tree() -> Tree {
        let (root_id, child_id, grandchild_id, other_id) = (
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        );
        let root_path = label(root_id);
        let child_path = format!("{root_path}.{}", label(child_id));
        let grandchild_path = format!("{child_path}.{}", label(grandchild_id));
        Tree {
            root: node(root_id, &root_path, 0),
            child: node(child_id, &child_path, 1),
            grandchild: node(grandchild_id, &grandchild_path, 2),
            other: node(other_id, &label(other_id), 0),
        }
    }

    #[test]
    fn reparent_moves_department_and_all_descendants_to_new_prefix() {
        let t = tree();
        let subtree = vec![t.child.clone(), t.grandchild.clone()];
        let moved = plan_department_reparent(&t.child, Some(&t.other), &subtree).unwrap();

        let new_child_path = format!("{}.{}", t.other.path, label(t.child.id));
        assert_eq!(
            moved,
            vec![
                node(t.child.id, &new_child_path, 1),
                node(
                    t.grandchild.id,
                    &format!("{new_child_path}.{}", label(t.grandchild.id)),
                    2
                ),
            ]
        );
    }

    #[test]
    fn reparent_to_top_level_rebases_subtree_at_depth_zero() {
        let t = tree();
        let subtree = vec![t.child.clone(), t.grandchild.clone()];
        let moved = plan_department_reparent(&t.child, None, &subtree).unwrap();

        let new_child_path = label(t.child.id);
        assert_eq!(moved[0], node(t.child.id, &new_child_path, 0));
        assert_eq!(
            moved[1],
            node(
                t.grandchild.id,
                &format!("{new_child_path}.{}", label(t.grandchild.id)),
                1
            )
        );
    }

    #[test]
    fn reparent_top_level_department_under_another_top_level_department() {
        let t = tree();
        let subtree = vec![t.root.clone(), t.child.clone(), t.grandchild.clone()];
        let moved = plan_department_reparent(&t.root, Some(&t.other), &subtree).unwrap();

        let new_root_path = format!("{}.{}", t.other.path, t.root.path);
        assert_eq!(moved[0], node(t.root.id, &new_root_path, 1));
        assert_eq!(
            moved[2],
            node(
                t.grandchild.id,
                &format!(
                    "{new_root_path}.{}.{}",
                    label(t.child.id),
                    label(t.grandchild.id)
                ),
                3
            )
        );
    }

    #[test]
    fn reparent_under_itself_is_a_cycle() {
        let t = tree();
        let subtree = vec![t.child.clone(), t.grandchild.clone()];
        assert_eq!(
            plan_department_reparent(&t.child, Some(&t.child), &subtree),
            Err(ReparentError::Cycle)
        );
    }

    #[test]
    fn reparent_under_descendant_is_a_cycle() {
        let t = tree();
        let subtree = vec![t.root.clone(), t.child.clone(), t.grandchild.clone()];
        assert_eq!(
            plan_department_reparent(&t.root, Some(&t.grandchild), &subtree),
            Err(ReparentError::Cycle)
        );
        assert_eq!(
            plan_department_reparent(&t.root, Some(&t.child), &subtree),
            Err(ReparentError::Cycle)
        );
    }

    #[test]
    fn reparent_under_sibling_sharing_a_path_prefix_is_not_a_cycle() {
        let t = tree();
        let sibling = node(Uuid::new_v4(), &format!("{}x", t.child.path), 1);
        let subtree = vec![t.child.clone()];
        assert!(plan_department_reparent(&t.child, Some(&sibling), &subtree).is_ok());
    }

    #[test]
    fn reparent_rejects_moves_that_exceed_max_depth() {
        let t = tree();
        let subtree = vec![t.child.clone(), t.grandchild.clone()];
        let deep_parent = node(Uuid::new_v4(), &format!("{}.deep", t.other.path), 8);
        assert_eq!(
            plan_department_reparent(&t.child, Some(&deep_parent), &subtree),
            Err(ReparentError::TooDeep)
        );

        let ok_parent = node(Uuid::new_v4(), &format!("{}.ok", t.other.path), 7);
        assert!(plan_department_reparent(&t.child, Some(&ok_parent), &subtree).is_ok());
    }

    #[test]
    fn reparent_ignores_rows_outside_the_moved_subtree() {
        let t = tree();
        let subtree = vec![t.child.clone(), t.other.clone()];
        let moved = plan_department_reparent(&t.child, Some(&t.other), &subtree).unwrap();
        assert_eq!(moved.len(), 1);
        assert_eq!(moved[0].id, t.child.id);
    }

    #[test]
    fn reparent_errors_map_to_existing_api_errors() {
        assert!(matches!(
            AuthError::from(ReparentError::Cycle),
            AuthError::DbConflict
        ));
        assert!(matches!(
            AuthError::from(ReparentError::TooDeep),
            AuthError::ServiceTemporarilyUnavailable
        ));
    }

    #[test]
    fn creator_receives_department_admin_only_when_already_holding_its_permissions() {
        let role_permissions = vec![
            "departments:manage".to_string(),
            "budget:allocate".to_string(),
        ];
        let department_admin_of_parent = PermissionGrants::from_rows(
            &[],
            vec![
                grant("departments:manage", true, Some("p")),
                grant("budget:allocate", true, Some("p")),
            ],
        );
        assert!(creator_may_receive_department_admin(
            &department_admin_of_parent,
            &role_permissions,
            DepartmentScope::Department("p.new"),
        ));

        let manage_only =
            PermissionGrants::from_rows(&[], vec![grant("departments:manage", true, Some("p"))]);
        assert!(!creator_may_receive_department_admin(
            &manage_only,
            &role_permissions,
            DepartmentScope::Department("p.new"),
        ));

        let super_admin = PermissionGrants::from_rows(&[role(ROLE_SUPER_ADMIN, None)], vec![]);
        assert!(creator_may_receive_department_admin(
            &super_admin,
            &role_permissions,
            DepartmentScope::Department("p.new"),
        ));
    }
}
