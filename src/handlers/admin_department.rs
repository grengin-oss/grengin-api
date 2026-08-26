// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
        permissions::{
            PERMISSION_DEPARTMENTS_MANAGE, PERMISSION_DEPARTMENTS_VIEW, PERMISSION_USERS_VIEW,
            ROLE_SUPER_ADMIN,
        },
    },
    dto::{
        admin_department::{
            ChildCountRow, Department, DepartmentCreate, DepartmentListQuery,
            DepartmentMembersResponse, DepartmentMemeberListQuery, DepartmentMove, DepartmentRow,
            DepartmentSortRule, DepartmentTree, DepartmentTreeNode, DepartmentTreeQuery,
            DepartmentTreeRow, DepartmentUpdate, DepartmentsListResponse, DeptCountRow,
        },
        admin_user::User,
        common::SortRule,
    },
    models::{
        departments, user_role_assignments,
        users::{self, UserStatus},
    },
    services::{
        authorization::{AuthorizationService, PermissionScopeMode},
        budget_allocation::{
            period_bounds, refresh_department_budget_available, sum_child_allocations,
            sum_department_cost_in_range,
        },
        department_helpers::{
            build_ltree_path, department_budget_snapshot, departments_base_select,
            departments_tree_select, ensure_department_admin_assignment,
            load_department_admin_ids_map, max_subtree_depth, sync_department_admin_assignments,
            sync_department_allowed_models,
        },
        department_policies::{
            load_allowed_models_map, validate_allowed_models_subset, validate_retention_days,
        },
        notifications::emit_budget_alerts,
    },
    state::SharedState,
    utils::ltree::ltree_label_from_uuid,
};
use axum::{
    Json,
    extract::{Path, Query, State},
};
use chrono::Utc;
use migration::{Alias, BinOper, Func, SimpleExpr, extension::postgres::PgExpr};
use reqwest::StatusCode;
use rust_decimal::Decimal;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait as _, QueryFilter, Statement,
};
use sea_orm::{
    Condition, EntityName as _, JoinType, Order, PaginatorTrait, QueryOrder, QuerySelect,
    RelationTrait,
    sea_query::{Expr, PostgresQueryBuilder, Query as SqlQuery},
};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

fn department_admin_plan(
    requested_admin_ids: Option<&[Uuid]>,
    creator_id: Uuid,
    is_super_admin: bool,
) -> (Vec<Uuid>, bool) {
    let mut requested = requested_admin_ids.unwrap_or_default().to_vec();
    requested.retain(|user_id| *user_id != creator_id);
    requested.sort_unstable();
    requested.dedup();
    (requested, !is_super_admin)
}

#[utoipa::path(
    post,
    path = "/admin/departments",
    tag = "admin",
    request_body = DepartmentCreate,
    responses(
       (status = 201, body = Department),
       (status = 401, content_type = "application/json", body = Error),
       (status = 404, content_type = "application/json", body = Error, description = "Parent department not found"),
       (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn create_department(
    claims: Claims,
    State(app_state): State<SharedState>,
    Json(req): Json<DepartmentCreate>,
) -> Result<(StatusCode, Json<Department>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_DEPARTMENTS_MANAGE,
            req.parent_id,
            PermissionScopeMode::RequireOrgWide,
            req.parent_id,
        )
        .await?;

    let is_super_admin = authz
        .user_has_role_name(claims.user_id, ROLE_SUPER_ADMIN)
        .await?;
    let (requested_admin_ids, assign_creator) =
        department_admin_plan(req.admin_ids.as_deref(), claims.user_id, is_super_admin);

    let id = Uuid::new_v4();
    let created_at = Utc::now();
    let updated_at = created_at;

    if let Some(models) = req.allowed_models.as_deref() {
        validate_allowed_models_subset(&app_state.database, req.parent_id, models)
            .await
            .map_err(|e| {
                eprintln!("allowed models validation error: {e}");
                AuthError::DbConflict
            })?;
    }

    validate_retention_days(&app_state.database, req.parent_id, req.retention_days)
        .await
        .map_err(|e| {
            eprintln!("retention_days validation error: {e}");
            AuthError::DbConflict
        })?;

    // Fetch parent (if any) to compute path/depth
    let (parent_path, depth) = if let Some(parent_id) = req.parent_id {
        let parent = departments_base_select()
            .filter(departments::Column::Id.eq(parent_id))
            .into_model::<DepartmentRow>()
            .one(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("db error: {e}");
                AuthError::DbTimeout
            })?
            .ok_or(AuthError::DbNotFound)?;

        (Some(parent.path), parent.depth + 1)
    } else {
        (None, 0)
    };

    let path_str = build_ltree_path(parent_path.as_deref(), id)?;

    if depth > 9 {
        return Err(AuthError::ServiceTemporarilyUnavailable);
    }

    let insert = SqlQuery::insert()
        .into_table(departments::Entity)
        .columns([
            departments::Column::Id,
            departments::Column::Name,
            departments::Column::Description,
            departments::Column::ParentId,
            departments::Column::Depth,
            departments::Column::Path,
            departments::Column::RetentionDays,
            departments::Column::CreatedAt,
            departments::Column::UpdatedAt,
        ])
        .values_panic([
            id.into(),
            req.name.clone().into(),
            req.description.clone().into(),
            req.parent_id.into(),
            depth.into(),
            Expr::val(path_str.clone())
                .cast_as(Alias::new("ltree"))
                .into(),
            req.retention_days.into(),
            created_at.into(),
            updated_at.into(),
        ])
        .to_owned();
    let (sql, values) = insert.build(PostgresQueryBuilder);
    app_state
        .database
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            values,
        ))
        .await
        .map_err(|e| {
            eprintln!("insert error: {e}");
            AuthError::DbTimeout
        })?;

    sync_department_allowed_models(&app_state.database, id, req.allowed_models.as_deref()).await?;

    if !requested_admin_ids.is_empty() {
        sync_department_admin_assignments(
            &authz,
            &app_state.database,
            claims.user_id,
            id,
            &requested_admin_ids,
            req.parent_id,
        )
        .await?;
    }
    if assign_creator {
        ensure_department_admin_assignment(
            &authz,
            &app_state.database,
            claims.user_id,
            claims.user_id,
            id,
        )
        .await?;
    }
    let (_, Json(response)) = get_department_by_id(claims, State(app_state), Path(id)).await?;
    Ok((StatusCode::CREATED, Json(response)))
}

#[utoipa::path(
    get,
    path = "/admin/departments",
    tag = "admin",
    params(
        ("parent_id" = Option<String>, Query, description = "Filter by parent department id (use \"root\" for top-level)"),
        ("include_children" = Option<bool>, Query, description = "Include descendant departments when parent_id is set (default: false)"),
        ("search" = Option<String>, Query, description = "Search by department name"),
        ("sort" = Option<DepartmentSortRule>, Query, description = "Sort by name, created_at, updated_at, members, or sub_departments"),
        ("ascending" = Option<bool>, Query, description = "Sort ascending when true (default: false)"),
    ),
    responses(
       (status = 200, body = DepartmentsListResponse),
       (status = 401, content_type = "application/json", body = Error),
       (status = 404, content_type = "application/json", body = Error),
       (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn list_departments(
    claims: Claims,
    State(app_state): State<SharedState>,
    Query(q): Query<DepartmentListQuery>,
) -> Result<(StatusCode, Json<DepartmentsListResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    let target_scope = q.parent_id.as_deref().and_then(|parent| {
        if parent.eq_ignore_ascii_case("root") {
            None
        } else {
            Uuid::parse_str(parent).ok()
        }
    });
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_DEPARTMENTS_VIEW,
            target_scope,
            PermissionScopeMode::RequireOrgWide,
            target_scope,
        )
        .await?;

    let mut query = departments_base_select();

    if let Some(parent) = q.parent_id.as_deref() {
        if parent.eq_ignore_ascii_case("root") {
            query = query.filter(departments::Column::ParentId.is_null());
        } else {
            let parent_uuid =
                Uuid::parse_str(parent).map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;

            if q.include_children {
                let parent_dept = departments_base_select()
                    .filter(departments::Column::Id.eq(parent_uuid))
                    .into_model::<DepartmentRow>()
                    .one(&app_state.database)
                    .await
                    .map_err(|e| {
                        eprintln!("db error: {e}");
                        AuthError::DbTimeout
                    })?
                    .ok_or(AuthError::DbNotFound)?;

                // path <@ CAST(parent_path AS ltree)  (no raw string)
                query = query.filter(Expr::col(departments::Column::Path).binary(
                    BinOper::Custom("<@".into()),
                    Expr::val(parent_dept.path.clone()).cast_as(Alias::new("ltree")),
                ));
            } else {
                query = query.filter(departments::Column::ParentId.eq(parent_uuid));
            }
        }
    }

    if let Some(search) = q.search.as_deref() {
        query = query.filter(
            departments::Column::Name
                .into_expr()
                .ilike(format!("%{}%", search)),
        );
    }

    query = query.order_by_asc(departments::Column::Name);

    let rows = query
        .into_model::<DepartmentRow>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error: {e}");
            AuthError::DbTimeout
        })?;

    let total = rows.len() as i64;

    // If no departments, return early
    if rows.is_empty() {
        return Ok((
            StatusCode::OK,
            Json(DepartmentsListResponse {
                departments: vec![],
                total,
            }),
        ));
    }

    // Collect department IDs
    let dept_ids: Vec<Uuid> = rows.iter().map(|d| d.id).collect();

    // -------- member_count (direct users) in ONE query --------
    let direct_counts: Vec<DeptCountRow> = users::Entity::find()
        .select_only()
        .column(users::Column::DepartmentId)
        .expr_as(Func::count(Expr::col(users::Column::Id)), "cnt")
        .filter(users::Column::DepartmentId.is_in(dept_ids.clone()))
        .filter(users::Column::Status.ne(UserStatus::Deleted))
        .group_by(users::Column::DepartmentId)
        .into_model::<DeptCountRow>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("direct member count error: {e}");
            AuthError::DbTimeout
        })?;

    let direct_map: HashMap<Uuid, i64> = direct_counts
        .into_iter()
        .map(|r| (r.department_id, r.cnt))
        .collect();

    // -------- child_count (direct children) in ONE query --------
    let child_counts: Vec<ChildCountRow> = departments::Entity::find()
        .select_only()
        .column(departments::Column::ParentId)
        .expr_as(Func::count(Expr::col(departments::Column::Id)), "cnt")
        .filter(departments::Column::ParentId.is_in(dept_ids.clone()))
        .group_by(departments::Column::ParentId)
        .into_model::<ChildCountRow>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("child count error: {e}");
            AuthError::DbTimeout
        })?;

    let child_map: HashMap<Uuid, i64> = child_counts
        .into_iter()
        .map(|r| (r.parent_id, r.cnt))
        .collect();

    let admin_map = load_department_admin_ids_map(&app_state.database, &dept_ids).await?;
    let allowed_models_map = load_allowed_models_map(&app_state.database, &dept_ids)
        .await
        .map_err(|e| {
            eprintln!("allowed models load error: {e}");
            AuthError::DbTimeout
        })?;

    // Build response
    let mut departments = Vec::with_capacity(rows.len());
    for d in rows {
        let member_count = *direct_map.get(&d.id).unwrap_or(&0) as i32;
        let child_count = *child_map.get(&d.id).unwrap_or(&0) as i32;
        let (budget_distributed, budget_available, budget_used) = department_budget_snapshot(
            &app_state.database,
            d.id,
            d.budget_allocated,
            d.budget_period,
        )
        .await?;
        let admin_ids = admin_map.get(&d.id).cloned().unwrap_or_default();
        let allowed_models = allowed_models_map.get(&d.id).cloned().unwrap_or_default();

        departments.push(Department {
            id: d.id,
            name: d.name,
            description: d.description,
            parent_id: d.parent_id,
            path: d.path,
            depth: d.depth,
            admin_ids,
            member_count,
            total_member_count: 0, // keep for detail endpoint to avoid heavy list queries
            child_count,
            budget_allocated: d.budget_allocated.to_string().parse().unwrap_or(0.0),
            budget_distributed,
            budget_available,
            budget_used,
            budget_period: d.budget_period,
            retention_days: d.retention_days,
            allowed_models,
            created_at: d.created_at,
            updated_at: d.updated_at,
        });
    }

    let sort_rule = q.sort.unwrap_or(DepartmentSortRule::Name);
    let ascending = q.ascending.unwrap_or(false);
    departments.sort_by(|a, b| {
        let ordering = match sort_rule {
            DepartmentSortRule::Name => a.name.cmp(&b.name),
            DepartmentSortRule::CreatedAt => a.created_at.cmp(&b.created_at),
            DepartmentSortRule::UpdatedAt => a.updated_at.cmp(&b.updated_at),
            DepartmentSortRule::Members => a.member_count.cmp(&b.member_count),
            DepartmentSortRule::SubDepartments => a.child_count.cmp(&b.child_count),
        };
        if ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });

    Ok((
        StatusCode::OK,
        Json(DepartmentsListResponse { departments, total }),
    ))
}

#[utoipa::path(
    get,
    path = "/admin/departments/tree",
    tag = "admin",
    params(
        ("root_id" = Option<Uuid>, Query, description = "Start from this department (default: entire org)"),
        ("max_depth" = Option<i32>, Query, description = "Default value : 10")
    ),
    responses(
       (status = 200, body = DepartmentTree, description = "Department tree"),
       (status = 401, content_type = "application/json", body = Error, description = "Unauthorized"),
       (status = 403, content_type = "application/json", body = Error, description = "Forbidden - Admin role required"),
       (status = 404, content_type = "application/json", body = Error, description = "Resource not found"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable")
    )
)]
pub async fn get_departments_tree(
    claims: Claims,
    State(app_state): State<SharedState>,
    Query(q): Query<DepartmentTreeQuery>,
) -> Result<(StatusCode, Json<DepartmentTree>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_DEPARTMENTS_VIEW,
            q.root_id,
            PermissionScopeMode::RequireOrgWide,
            q.root_id,
        )
        .await?;

    let max_depth = q.max_depth.unwrap_or(10).max(0);

    let root = if let Some(root_id) = q.root_id {
        Some(
            departments_base_select()
                .filter(departments::Column::Id.eq(root_id))
                .into_model::<DepartmentRow>()
                .one(&app_state.database)
                .await
                .map_err(|e| {
                    eprintln!("db error: {e}");
                    AuthError::DbTimeout
                })?
                .ok_or(AuthError::DbNotFound)?,
        )
    } else {
        None
    };

    let mut query = departments_tree_select();

    if let Some(root_dept) = &root {
        query = query.filter(Expr::col(departments::Column::Path).binary(
            BinOper::Custom("<@".into()),
            Expr::val(root_dept.path.clone()).cast_as(Alias::new("ltree")),
        ));
        query = query.filter(departments::Column::Depth.lte(root_dept.depth + max_depth));
    } else {
        query = query.filter(departments::Column::Depth.lte(max_depth));
    }

    let rows = query
        .into_model::<DepartmentTreeRow>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error: {e}");
            AuthError::DbTimeout
        })?;

    if rows.is_empty() {
        return Ok((StatusCode::OK, Json(DepartmentTree { tree: vec![] })));
    }

    let dept_ids: Vec<Uuid> = rows.iter().map(|d| d.id).collect();

    let direct_counts: Vec<DeptCountRow> = users::Entity::find()
        .select_only()
        .column(users::Column::DepartmentId)
        .expr_as(Func::count(Expr::col(users::Column::Id)), "cnt")
        .filter(users::Column::DepartmentId.is_in(dept_ids.clone()))
        .filter(users::Column::Status.ne(UserStatus::Deleted))
        .group_by(users::Column::DepartmentId)
        .into_model::<DeptCountRow>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("direct member count error: {e}");
            AuthError::DbTimeout
        })?;

    let direct_map: HashMap<Uuid, i64> = direct_counts
        .into_iter()
        .map(|r| (r.department_id, r.cnt))
        .collect();

    let admin_map = load_department_admin_ids_map(&app_state.database, &dept_ids).await?;
    let allowed_models_map = load_allowed_models_map(&app_state.database, &dept_ids)
        .await
        .map_err(|e| {
            eprintln!("allowed models load error: {e}");
            AuthError::DbTimeout
        })?;

    let mut nodes: HashMap<Uuid, DepartmentTreeNode> = HashMap::with_capacity(rows.len());
    let mut children_map: HashMap<Uuid, Vec<Uuid>> = HashMap::with_capacity(rows.len());

    for d in rows {
        let member_count = *direct_map.get(&d.id).unwrap_or(&0) as i32;
        let admin_ids = admin_map.get(&d.id).cloned().unwrap_or_default();
        let allowed_models = allowed_models_map.get(&d.id).cloned().unwrap_or_default();

        if let Some(pid) = d.parent_id {
            children_map.entry(pid).or_default().push(d.id);
        }

        nodes.insert(
            d.id,
            DepartmentTreeNode {
                id: d.id,
                name: d.name,
                description: d.description,
                parent_id: d.parent_id,
                path: d.path,
                depth: d.depth,
                admin_ids,
                member_count,
                total_member_count: member_count,
                child_count: 0,
                budget_allocated: d.budget_allocated.to_string().parse().unwrap_or(0.0),
                budget_distributed: 0.0,
                budget_available: 0.0,
                budget_used: 0.0,
                budget_period: d.budget_period,
                retention_days: d.retention_days,
                allowed_models,
                created_at: d.created_at,
                updated_at: d.updated_at,
                children: vec![],
            },
        );
    }

    let root_ids: Vec<Uuid> = if let Some(root_dept) = &root {
        vec![root_dept.id]
    } else {
        nodes
            .values()
            .filter_map(|n| match n.parent_id {
                None => Some(n.id),
                Some(pid) => (!nodes.contains_key(&pid)).then_some(n.id),
            })
            .collect()
    };

    fn attach(
        id: Uuid,
        nodes: &mut HashMap<Uuid, DepartmentTreeNode>,
        children_map: &mut HashMap<Uuid, Vec<Uuid>>,
        visiting: &mut HashSet<Uuid>,
    ) -> Option<DepartmentTreeNode> {
        // Guard against accidental cycles in the hierarchy to avoid infinite recursion.
        if !visiting.insert(id) {
            eprintln!("cycle detected while building department tree at: {id}");
            return None;
        }
        let mut node = nodes.remove(&id)?;
        let child_ids = children_map.remove(&id).unwrap_or_default();

        let mut total = node.member_count;
        let mut children = Vec::with_capacity(child_ids.len());

        for cid in child_ids {
            if let Some(child) = attach(cid, nodes, children_map, visiting) {
                total += child.total_member_count;
                children.push(child);
            }
        }

        node.child_count = children.len() as i32;
        node.total_member_count = total;
        node.children = children;
        visiting.remove(&id);
        Some(node)
    }

    let mut tree = Vec::with_capacity(root_ids.len());
    let mut visiting = HashSet::with_capacity(root_ids.len());
    for id in root_ids {
        if let Some(node) = attach(id, &mut nodes, &mut children_map, &mut visiting) {
            tree.push(node);
        }
    }

    Ok((StatusCode::OK, Json(DepartmentTree { tree })))
}

#[utoipa::path(
    get,
    path = "/admin/departments/{department_id}",
    tag = "admin",
    params(
        ("department_id" = Uuid, Path, description = "Department id")
    ),
    responses(
       (status = 200, body = Department),
       (status = 401, content_type = "application/json", body = Error),
       (status = 404, content_type = "application/json", body = Error),
       (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn get_department_by_id(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(department_id): Path<Uuid>,
) -> Result<(StatusCode, Json<Department>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_DEPARTMENTS_VIEW,
            Some(department_id),
            PermissionScopeMode::RequireOrgWide,
            Some(department_id),
        )
        .await?;

    let dept = departments_base_select()
        .filter(departments::Column::Id.eq(department_id))
        .into_model::<DepartmentRow>()
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::DbNotFound)?;

    let mut admin_map = load_department_admin_ids_map(&app_state.database, &[dept.id]).await?;
    let admin_ids = admin_map.remove(&dept.id).unwrap_or_default();

    // 1) Direct members count
    let member_count = users::Entity::find()
        .filter(users::Column::DepartmentId.eq(dept.id))
        .filter(users::Column::Status.ne(UserStatus::Deleted))
        .count(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("count error: {e}");
            AuthError::DbTimeout
        })? as i32;

    // 2) Subtree department IDs (self + descendants), using ltree: d.path <@ (dept.path::ltree)
    let subtree_ids: Vec<Uuid> = departments::Entity::find()
        .select_only()
        .column(departments::Column::Id)
        .filter(Expr::col(departments::Column::Path).binary(
            BinOper::Custom("<@".into()),
            Expr::val(dept.path.clone()).cast_as(Alias::new("ltree")),
        ))
        .into_tuple::<(Uuid,)>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("subtree query error: {e}");
            AuthError::DbTimeout
        })?
        .into_iter()
        .map(|(id,)| id)
        .collect();

    // 3) Total members count (self + descendants)
    let total_member_count = users::Entity::find()
        .filter(users::Column::DepartmentId.is_in(subtree_ids))
        .filter(users::Column::Status.ne(UserStatus::Deleted))
        .count(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("total count error: {e}");
            AuthError::DbTimeout
        })? as i32;

    // 4) Optional: direct child count
    let child_count = departments::Entity::find()
        .filter(departments::Column::ParentId.eq(dept.id))
        .count(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("child count error: {e}");
            AuthError::DbTimeout
        })? as i32;

    let (budget_distributed, budget_available, budget_used) = department_budget_snapshot(
        &app_state.database,
        dept.id,
        dept.budget_allocated,
        dept.budget_period,
    )
    .await?;

    let allowed_models_map = load_allowed_models_map(&app_state.database, &[dept.id])
        .await
        .map_err(|e| {
            eprintln!("allowed models load error: {e}");
            AuthError::DbTimeout
        })?;
    let allowed_models = allowed_models_map
        .get(&dept.id)
        .cloned()
        .unwrap_or_default();

    let resp = Department {
        id: dept.id,
        name: dept.name,
        description: dept.description,
        parent_id: dept.parent_id,
        path: dept.path,
        depth: dept.depth,
        admin_ids,
        member_count,
        total_member_count,
        child_count,
        budget_allocated: dept.budget_allocated.to_string().parse().unwrap_or(0.0),
        budget_distributed,
        budget_available,
        budget_used,
        budget_period: dept.budget_period,
        retention_days: dept.retention_days,
        allowed_models,
        created_at: dept.created_at,
        updated_at: dept.updated_at,
    };

    Ok((StatusCode::OK, Json(resp)))
}

#[utoipa::path(
    put, path = "/admin/departments/{department_id}",
    tag = "admin", params( ("department_id" = Uuid, Path, description = "Department id") ),
    request_body = DepartmentUpdate,
    responses(
        (status = 200, body = Department),
        (status = 401, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error), 
    )
)]
pub async fn update_department(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(department_id): Path<Uuid>,
    Json(req): Json<DepartmentUpdate>,
) -> Result<(StatusCode, Json<Department>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_DEPARTMENTS_MANAGE,
            Some(department_id),
            PermissionScopeMode::RequireOrgWide,
            Some(department_id),
        )
        .await?;

    let dept = departments_base_select()
        .filter(departments::Column::Id.eq(department_id))
        .into_model::<DepartmentRow>()
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::DbNotFound)?;

    let name = req.name.clone().unwrap_or(dept.name.clone());
    let description = req.description.clone().unwrap_or(dept.description.clone());
    let parent_id = req.parent_id.or(dept.parent_id);
    let parent_changed = parent_id != dept.parent_id;
    let budget_period = req.budget_period.unwrap_or(dept.budget_period);
    let action_on_exceed = req.action_on_exceed.unwrap_or(dept.action_on_exceed);
    let retention_days = req.retention_days.or(dept.retention_days);
    let budget_allocated = if let Some(value) = req.budget_allocated {
        Decimal::from_f32_retain(value).ok_or(AuthError::ServiceTemporarilyUnavailable)?
    } else {
        dept.budget_allocated
    };

    if req.parent_id.is_some() || req.budget_allocated.is_some() {
        if let Some(parent_id) = parent_id {
            let parent = departments_base_select()
                .filter(departments::Column::Id.eq(parent_id))
                .into_model::<DepartmentRow>()
                .one(&app_state.database)
                .await
                .map_err(|e| {
                    eprintln!("find parent error: {e}");
                    AuthError::DbTimeout
                })?
                .ok_or(AuthError::DbNotFound)?;

            let other_children_alloc =
                sum_child_allocations(&app_state.database, parent.id, Some(dept.id))
                    .await
                    .map_err(|e| {
                        eprintln!("sum other children error: {e}");
                        AuthError::DbTimeout
                    })?;

            let now = Utc::now();
            let (p_start, p_end) = period_bounds(&parent.budget_period, now);
            let parent_used =
                sum_department_cost_in_range(&app_state.database, parent.id, p_start, p_end)
                    .await
                    .map_err(|e| {
                        eprintln!("sum parent used error: {e}");
                        AuthError::DbTimeout
                    })?;

            let parent_available_for_children =
                (parent.budget_allocated - other_children_alloc - parent_used).max(Decimal::ZERO);

            if budget_allocated > parent_available_for_children {
                return Err(AuthError::BudgetExceedsParentAvailable);
            }
        }
    }

    if let Some(models) = req.allowed_models.as_deref() {
        validate_allowed_models_subset(&app_state.database, parent_id, models)
            .await
            .map_err(|e| {
                eprintln!("allowed models validation error: {e}");
                AuthError::DbConflict
            })?;
    } else if parent_changed {
        let current_allowed = load_allowed_models_map(&app_state.database, &[department_id])
            .await
            .map_err(|e| {
                eprintln!("allowed models load error: {e}");
                AuthError::DbTimeout
            })?
            .remove(&department_id)
            .unwrap_or_default();
        validate_allowed_models_subset(&app_state.database, parent_id, &current_allowed)
            .await
            .map_err(|e| {
                eprintln!("allowed models validation error: {e}");
                AuthError::DbConflict
            })?;
    }

    validate_retention_days(&app_state.database, parent_id, retention_days)
        .await
        .map_err(|e| {
            eprintln!("retention_days validation error: {e}");
            AuthError::DbConflict
        })?;

    // Recompute path/depth if parent changes
    let (path, depth) = if parent_changed {
        if let Some(parent_id) = parent_id {
            let parent = departments_base_select()
                .filter(departments::Column::Id.eq(parent_id))
                .into_model::<DepartmentRow>()
                .one(&app_state.database)
                .await
                .map_err(|e| {
                    eprintln!("db error: {e}");
                    AuthError::DbTimeout
                })?
                .ok_or(AuthError::DbNotFound)?;

            let label = ltree_label_from_uuid(department_id);
            (format!("{}.{}", parent.path, label), parent.depth + 1)
        } else {
            (ltree_label_from_uuid(department_id), 0)
        }
    } else {
        (dept.path.clone(), dept.depth)
    };

    if parent_changed {
        let subtree_max = max_subtree_depth(&app_state.database, &dept.path).await?;
        let new_root_depth = depth;
        let subtree_height = subtree_max - dept.depth;
        if new_root_depth + subtree_height > 9 {
            return Err(AuthError::ServiceTemporarilyUnavailable);
        }
    }

    let updated_at = Utc::now();

    // ✅ SeaQuery UPDATE with CAST(... AS ltree)
    let stmt = SqlQuery::update()
        .table(departments::Entity)
        .values([
            (departments::Column::Name, name.into()),
            (departments::Column::Description, description.into()),
            (departments::Column::ParentId, parent_id.into()),
            (departments::Column::Depth, depth.into()),
            (
                departments::Column::Path,
                Expr::val(path.clone()).cast_as(Alias::new("ltree")).into(),
            ),
            (departments::Column::RetentionDays, retention_days.into()),
            (
                departments::Column::BudgetAllocated,
                budget_allocated.into(),
            ),
            (departments::Column::BudgetPeriod, budget_period.into()),
            (departments::Column::ActionOnExceed, action_on_exceed.into()),
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
            eprintln!("update error: {e}");
            AuthError::DbTimeout
        })?;

    if req.allowed_models.is_some() {
        sync_department_allowed_models(
            &app_state.database,
            department_id,
            req.allowed_models.as_deref(),
        )
        .await?;
    }

    if let Some(admin_ids) = req.admin_ids.as_deref() {
        sync_department_admin_assignments(
            &authz,
            &app_state.database,
            claims.user_id,
            department_id,
            admin_ids,
            Some(department_id),
        )
        .await?;
    }

    if let Err(e) = refresh_department_budget_available(&app_state.database, department_id).await {
        eprintln!("refresh budget available error: {e}");
    } else if let Err(e) = emit_budget_alerts(&app_state, department_id).await {
        eprintln!("emit budget alert error: {:?}", e);
    }

    if parent_changed {
        let _ = authz
            .recompute_effective_permissions_for_department_scope(department_id)
            .await;
    }

    get_department_by_id(claims, State(app_state), Path(department_id)).await
}

#[utoipa::path(
    post,
    path = "/admin/departments/{department_id}/move",
    tag = "admin",
    params(
        ("department_id" = Uuid, Path, description = "Department id")
    ),
    request_body = DepartmentMove,
    responses(
        (status = 200, body = Department, description = "Department moved successfully"),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error, description = "Forbidden - Admin role required"),
        (status = 404, content_type = "application/json", body = Error),
        (status = 409, content_type = "application/json", body = Error, description = "Invalid move target"),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn move_department(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(department_id): Path<Uuid>,
    Json(req): Json<DepartmentMove>,
) -> Result<(StatusCode, Json<Department>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_DEPARTMENTS_MANAGE,
            Some(department_id),
            PermissionScopeMode::RequireOrgWide,
            Some(department_id),
        )
        .await?;
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_DEPARTMENTS_MANAGE,
            Some(req.new_parent_id),
            PermissionScopeMode::RequireOrgWide,
            Some(req.new_parent_id),
        )
        .await?;

    let dept = departments_base_select()
        .filter(departments::Column::Id.eq(department_id))
        .into_model::<DepartmentRow>()
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::DbNotFound)?;

    let new_parent = departments_base_select()
        .filter(departments::Column::Id.eq(req.new_parent_id))
        .into_model::<DepartmentRow>()
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::DbNotFound)?;

    let current_allowed = load_allowed_models_map(&app_state.database, &[department_id])
        .await
        .map_err(|e| {
            eprintln!("allowed models load error: {e}");
            AuthError::DbTimeout
        })?
        .remove(&department_id)
        .unwrap_or_default();
    validate_allowed_models_subset(&app_state.database, Some(new_parent.id), &current_allowed)
        .await
        .map_err(|e| {
            eprintln!("allowed models validation error: {e}");
            AuthError::DbConflict
        })?;
    validate_retention_days(
        &app_state.database,
        Some(new_parent.id),
        dept.retention_days,
    )
    .await
    .map_err(|e| {
        eprintln!("retention_days validation error: {e}");
        AuthError::DbConflict
    })?;

    let subtree_max = max_subtree_depth(&app_state.database, &dept.path).await?;
    let new_root_depth = new_parent.depth + 1;
    let subtree_height = subtree_max - dept.depth;
    if new_root_depth + subtree_height > 9 {
        return Err(AuthError::ServiceTemporarilyUnavailable);
    }

    // Prevent moving under itself or one of its descendants.
    if req.new_parent_id == department_id || new_parent.path.starts_with(&dept.path) {
        return Err(AuthError::DbConflict);
    }

    let label = ltree_label_from_uuid(department_id);
    let new_path = format!("{}.{}", new_parent.path, label);
    let new_depth = new_parent.depth + 1;
    let depth_delta = new_depth - dept.depth;
    let updated_at = Utc::now();

    // Update the root department's parent reference.
    let parent_stmt = SqlQuery::update()
        .table(departments::Entity)
        .values([
            (departments::Column::ParentId, req.new_parent_id.into()),
            (departments::Column::UpdatedAt, updated_at.into()),
        ])
        .and_where(Expr::col(departments::Column::Id).eq(department_id))
        .to_owned();

    let (parent_sql, parent_values) = parent_stmt.build(PostgresQueryBuilder);
    app_state
        .database
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            parent_sql,
            parent_values,
        ))
        .await
        .map_err(|e| {
            eprintln!("db update error: {e}");
            AuthError::DbTimeout
        })?;

    // Update the entire subtree's path + depth using a prefix replace.
    let subtree_stmt = SqlQuery::update()
        .table(departments::Entity)
        .values([
            (
                departments::Column::Depth,
                Expr::col(departments::Column::Depth)
                    .add(depth_delta)
                    .into(),
            ),
            (
                departments::Column::Path,
                Expr::cust(format!(
                    "replace(path::text, '{}', '{}')::ltree",
                    dept.path, new_path
                ))
                .into(),
            ),
            (departments::Column::UpdatedAt, updated_at.into()),
        ])
        .and_where(Expr::col(departments::Column::Path).binary(
            BinOper::Custom("<@".into()),
            Expr::val(dept.path.clone()).cast_as(Alias::new("ltree")),
        ))
        .to_owned();

    let (subtree_sql, subtree_values) = subtree_stmt.build(PostgresQueryBuilder);
    app_state
        .database
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            subtree_sql,
            subtree_values,
        ))
        .await
        .map_err(|e| {
            eprintln!("db subtree update error: {e}");
            AuthError::DbTimeout
        })?;

    let _ = authz
        .recompute_effective_permissions_for_department_scope(department_id)
        .await;

    get_department_by_id(claims, State(app_state), Path(department_id)).await
}

#[utoipa::path(
    delete,
    path = "/admin/departments/{department_id}",
    tag = "admin",
    params(
        ("department_id" = Uuid, Path, description = "Department id")
    ),
    responses(
       (status = 204, description = "Deleted"),
       (status = 401, content_type = "application/json", body = Error),
       (status = 404, content_type = "application/json", body = Error),
       (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn delete_department(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(department_id): Path<Uuid>,
) -> Result<StatusCode, AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_DEPARTMENTS_MANAGE,
            Some(department_id),
            PermissionScopeMode::RequireOrgWide,
            Some(department_id),
        )
        .await?;
    // Remove scoped role assignments for this department to avoid FK set-null conflicts
    let _ = user_role_assignments::Entity::delete_many()
        .filter(user_role_assignments::Column::ScopeDepartmentId.eq(department_id))
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role assignment delete error: {e}");
            AuthError::DbTimeout
        })?;
    let res = departments::Entity::delete_by_id(department_id)
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("delete error: {e}");
            AuthError::DbTimeout
        })?;
    if res.rows_affected == 0 {
        return Err(AuthError::DbNotFound);
    }
    let _ = authz.recompute_effective_permissions_for_all_users().await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/admin/departments/{department_id}/members",
    tag = "admin",
    request_body = Vec<Uuid>,
    params(
        ("department_id" = Uuid, Path, description = "Department id")
    ),
    responses(
       (status = 200, body = Department),
       (status = 401, content_type = "application/json", body = Error),
       (status = 404, content_type = "application/json", body = Error),
       (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn add_users_in_department(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(department_id): Path<Uuid>,
    Json(user_ids): Json<Vec<Uuid>>,
) -> Result<(StatusCode, Json<Department>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_DEPARTMENTS_MANAGE,
            Some(department_id),
            PermissionScopeMode::RequireOrgWide,
            Some(department_id),
        )
        .await?;
    let response = get_department_by_id(
        claims,
        State(app_state.clone()),
        Path(department_id.clone()),
    )
    .await
    .map_err(|_| AuthError::DbTimeout)?;
    users::Entity::update_many()
        .filter(users::Column::Id.is_in(user_ids.clone()))
        .col_expr(users::Column::DepartmentId, Expr::value(department_id))
        .col_expr(users::Column::UpdatedAt, Expr::value(Utc::now()))
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db update many error: {e}");
            AuthError::DbTimeout
        })?;
    let _ = authz
        .recompute_effective_permissions_for_users(&user_ids)
        .await;
    Ok(response)
}

#[utoipa::path(
    delete,
    path = "/admin/departments/{department_id}/members",
    tag = "admin",
    params(
        ("department_id" = Uuid, Path, description = "Department id"),
        ("department_id" = Uuid, Path, description = "Department id"),
    ),
    responses(
       (status = 200, body = Department),
       (status = 401, content_type = "application/json", body = Error),
       (status = 404, content_type = "application/json", body = Error),
       (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn remove_users_from_department(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(department_id): Path<Uuid>,
    Query(query): Query<DepartmentMemeberListQuery>,
    Json(user_ids): Json<Vec<Uuid>>,
) -> Result<(StatusCode, Json<Department>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_DEPARTMENTS_MANAGE,
            Some(department_id),
            PermissionScopeMode::RequireOrgWide,
            Some(department_id),
        )
        .await?;
    let force = query.force.unwrap_or(false);

    let users_t = Alias::new(users::Entity.table_name());
    let depts_t = Alias::new(departments::Entity.table_name());

    // SELECT parent_id FROM departments WHERE id = ?
    let parent_select = SqlQuery::select()
        .column((depts_t.clone(), departments::Column::ParentId))
        .from(depts_t.clone())
        .and_where(Expr::col((depts_t.clone(), departments::Column::Id)).eq(department_id))
        .to_owned();

    let parent_subexpr: SimpleExpr =
        SimpleExpr::SubQuery(None, Box::new(parent_select.into_sub_query_statement()));

    let new_dept_expr: SimpleExpr = if force {
        parent_subexpr
    } else {
        // department_id = NULL
        Expr::value(Option::<Uuid>::None).into()
    };

    // UPDATE users SET department_id = (subquery or NULL) WHERE ...
    let update_stmt = SqlQuery::update()
        .table(users_t.clone())
        .value(users::Column::DepartmentId, new_dept_expr)
        .value(users::Column::UpdatedAt, Expr::value(Utc::now()))
        .and_where(Expr::col((users_t.clone(), users::Column::Id)).is_in(user_ids.clone()))
        .and_where(Expr::col((users_t.clone(), users::Column::DepartmentId)).eq(department_id))
        .to_owned();

    // execute (no manual SQL text; SQL is generated + values bound)
    let (sql, values) = update_stmt.build(PostgresQueryBuilder);

    app_state
        .database
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            values,
        ))
        .await
        .map_err(|e| {
            eprintln!("db update error: {e}");
            AuthError::DbTimeout
        })?;
    let _ = authz
        .recompute_effective_permissions_for_users(&user_ids)
        .await;
    get_department_by_id(claims, State(app_state), Path(department_id)).await
}

#[utoipa::path(
    get,
    path = "/admin/departments/{department_id}/members",
    tag = "admin",
    params(
        ("department_id" = Uuid, Path, description = "department_id Uuid"),
        ("include_sub_department" = bool, Query, description = "Default value : false"),
        ("search" = Option<String>, Query, description = "Search by name,email,department"),
        ("status" = Option<UserStatus>, Query, description = "Account status"),
        ("role_id" = Option<Uuid>, Query, description = "Filter by RBAC role id"),
        ("sort" = Option<SortRule>, Query, description = "Sort by column example 'name','updated_at','created_at','email','last_login_at'"),
        ("order" = Option<String>, Query, description = "Sort order (asc/desc)"),
    ),
    responses(
       (status = 200, body = DepartmentMembersResponse),
       (status = 401, content_type = "application/json", body = Error),
       (status = 404, content_type = "application/json", body = Error),
       (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn get_users_from_department(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(department_id): Path<Uuid>,
    Query(query): Query<DepartmentMemeberListQuery>,
) -> Result<(StatusCode, Json<DepartmentMembersResponse>), AuthError> {
    let include_sub_department = query.include_sub_department.unwrap_or(false);
    let mut response = DepartmentMembersResponse {
        total: 0,
        members: Vec::new(),
    };
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_USERS_VIEW,
            Some(department_id),
            PermissionScopeMode::RequireOrgWide,
            Some(department_id),
        )
        .await?;
    let root = departments_base_select()
        .filter(departments::Column::Id.eq(department_id))
        .into_model::<DepartmentRow>()
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::DbNotFound)?;

    let subtree_dept_ids: Vec<Uuid> = departments::Entity::find()
        .select_only()
        .column(departments::Column::Id)
        .filter(Expr::col(departments::Column::Path).binary(
            BinOper::Custom("<@".into()),
            // RHS must be ltree-typed
            Expr::val(root.path.clone()).cast_as(Alias::new("ltree")),
        ))
        .into_tuple() // returns Vec<(Uuid,)>
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error: {e}");
            AuthError::DbTimeout
        })?
        .into_iter()
        .map(|(id,)| id)
        .collect();
    let mut select = users::Entity::find()
        .filter(users::Column::Status.ne(UserStatus::Deleted))
        .join(JoinType::LeftJoin, users::Relation::Departments.def());
    if let Some(role_id) = query.role_id {
        let role_user_ids: Vec<Uuid> = user_role_assignments::Entity::find()
            .select_only()
            .column(user_role_assignments::Column::UserId)
            .filter(user_role_assignments::Column::RoleId.eq(role_id))
            .into_tuple::<Uuid>()
            .all(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("role filter lookup error: {e}");
                AuthError::DbTimeout
            })?;
        if role_user_ids.is_empty() {
            return Ok((
                StatusCode::OK,
                Json(DepartmentMembersResponse {
                    total: 0,
                    members: Vec::new(),
                }),
            ));
        }
        select = select.filter(users::Column::Id.is_in(role_user_ids));
    }
    if let Some(status) = query.status {
        select = select.filter(users::Column::Status.eq(status))
    }
    let order = query.order.as_deref().unwrap_or("desc");
    let ord = if order.eq_ignore_ascii_case("asc") {
        Order::Asc
    } else {
        Order::Desc
    };
    if let Some(sort) = query.sort {
        select = match sort {
            SortRule::Name => select.order_by(users::Column::Name, ord),
            SortRule::Email => select.order_by(users::Column::Email, ord),
            SortRule::CreatedAt => select.order_by(users::Column::CreatedAt, ord),
            SortRule::UpdatedAt => select.order_by(users::Column::UpdatedAt, ord),
            SortRule::LastLoginAt => select.order_by(users::Column::LastLoginAt, ord),
            _ => select.order_by(users::Column::CreatedAt, ord),
        };
    }
    if let Some(search) = &query.search {
        select = select.filter(
            Condition::any()
                .add(
                    users::Column::Name
                        .into_expr()
                        .ilike(format!("%{}%", search)),
                )
                .add(
                    users::Column::Email
                        .into_expr()
                        .ilike(format!("%{}%", search)),
                )
                .add(
                    departments::Column::Name
                        .into_expr()
                        .ilike(format!("%{}%", search)),
                ),
        );
    }
    let users_row = if include_sub_department {
        // subtree_dept_ids logic above...
        select
            .filter(users::Column::DepartmentId.is_in(subtree_dept_ids))
            .all(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("db error: {e}");
                AuthError::DbTimeout
            })?
    } else {
        select
            .filter(users::Column::DepartmentId.eq(department_id))
            .all(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("db error: {e}");
                AuthError::DbTimeout
            })?
    };
    let user_ids: Vec<Uuid> = users_row.iter().map(|u| u.id).collect();
    let roles_map = authz.user_roles_map(&user_ids).await?;
    response.members = users_row
        .into_iter()
        .map(|user| {
            let roles = roles_map.get(&user.id).cloned().unwrap_or_default();
            let is_super_admin = roles.iter().any(|r| r == "Super Admin");
            User {
                id: user.id,
                sub: user
                    .azure_id
                    .unwrap_or(user.google_id.unwrap_or(user.email.clone())),
                email: user.email,
                name: user.name,
                picture: user.picture,
                hd: user.hd,
                roles,
                status: user.status,
                department: None,
                department_id: user.department_id,
                is_super_admin,
                has_password: user.password.is_some(),
                mfa_enabled: user.mfa_enabled,
                last_login_at: Some(user.last_login_at),
                password_changed_at: user.password_changed_at,
                created_at: user.created_at,
                updated_at: user.updated_at,
                effective_permissions: user.effective_permissions,
            }
        })
        .collect();
    response.total = response.members.len() as i32;
    Ok((StatusCode::OK, Json(response)))
}

#[cfg(test)]
mod tests {
    use super::department_admin_plan;
    use uuid::Uuid;

    #[test]
    fn non_super_admin_creator_is_assigned_and_removed_from_explicit_list() {
        let creator = Uuid::new_v4();
        let selected = Uuid::new_v4();
        let (requested, assign_creator) =
            department_admin_plan(Some(&[creator, selected, selected]), creator, false);

        assert_eq!(requested, vec![selected]);
        assert!(assign_creator);
    }

    #[test]
    fn super_admin_creator_is_not_assigned_to_the_department() {
        let creator = Uuid::new_v4();
        let selected = Uuid::new_v4();
        let (requested, assign_creator) =
            department_admin_plan(Some(&[creator, selected]), creator, true);

        assert_eq!(requested, vec![selected]);
        assert!(!assign_creator);
    }
}
