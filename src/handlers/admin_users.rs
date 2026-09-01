// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
        permissions::{PERMISSION_USERS_MANAGE, PERMISSION_USERS_VIEW},
    },
    dto::{
        admin_user::{
            PaginatedUsers, User, UserCreate, UserDepartmentRow, UserPatchRequest, UserUpdate,
        },
        common::{PaginationQuery, SortRule},
    },
    models::{
        departments, roles, user_role_assignments,
        users::{self, UserStatus},
    },
    services::authorization::{AuthorizationService, PermissionScopeMode},
    state::SharedState,
};
use axum::{
    Json,
    extract::{Path, Query, State},
};
use chrono::Utc;
use migration::extension::postgres::PgExpr;
use reqwest::StatusCode;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, Condition, EntityTrait, IntoActiveModel,
    JoinType, Order, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, RelationTrait,
};
use uuid::Uuid;

#[utoipa::path(
    get,
    path = "/admin/users/{user_id}",
    tag = "admin",
    params(
        ("user_id" = Uuid, Path, description = "User id")
    ),
    responses(
       (status = 200, body = User),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 404, content_type = "application/json", body = Error, description = "User not found (code=5003)"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),
    )
)]
pub async fn get_user_by_id(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(user_id): Path<Uuid>,
) -> Result<(StatusCode, Json<User>), AuthError> {
    let user = users::Entity::find_by_id(user_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("insert error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::EmailDoesNotExist)?;
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_USERS_VIEW,
            user.department_id,
            PermissionScopeMode::RequireOrgWide,
            Some(user_id),
        )
        .await?;
    let mut roles_map = authz.user_roles_map(&[user.id]).await?;
    let roles = roles_map.remove(&user.id).unwrap_or_default();
    let is_super_admin = roles.iter().any(|r| r == "Super Admin");
    let user_response = User {
        id: user.id,
        sub: user
            .google_id
            .unwrap_or(user.azure_id.unwrap_or(user.email.clone())),
        email: user.email,
        name: user.name,
        picture: user.picture,
        hd: user.hd,
        roles,
        status: user.status,
        department_id: user.department_id,
        department: None, // TODO
        is_super_admin,
        has_password: user.password.is_some(), // SSO-only users don't have password
        mfa_enabled: user.mfa_enabled,
        last_login_at: Some(user.last_login_at),
        password_changed_at: None,
        created_at: user.created_at,
        updated_at: user.updated_at,
        effective_permissions: user.effective_permissions,
    };
    Ok((StatusCode::OK, Json(user_response)))
}

#[utoipa::path(
    get,
    path = "/admin/users",
    tag = "admin",
    params(
        ("limit" = Option<u64>, Query, description = "Default value : 20"),
        ("offset" = Option<u64>, Query, description = "Default value : 0"),
        ("search" = Option<String>, Query, description = "Search by name,email,department"),
        ("status" = Option<UserStatus>, Query, description = "Account status"),
        ("role_id" = Option<Uuid>, Query, description = "Filter by RBAC role id"),
        ("ascending" = Option<bool>, Query, description = "Order of users list default false"),
        ("unassigned_department" = Option<bool>, Query, description = "Default false"),
        ("sort" = Option<SortRule>, Query, description = "Sort by column example 'name','updated_at','created_at','email','last_login_at'"),
    ),
    responses(
       (status = 200, body = PaginatedUsers),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 404, content_type = "application/json", body = Error, description = "User not found (code=5003)"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),
     
    )
)]
pub async fn get_users(
    claims: Claims,
    Query(query): Query<PaginationQuery>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<PaginatedUsers>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_USERS_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;
    let limit = query.limit.unwrap_or(30);
    let offset = query.offset.unwrap_or(0);
    let page = offset / limit;
    let mut response = PaginatedUsers {
        users: Vec::new(),
        total: 0,
        limit,
        offset,
    };
    let mut select = users::Entity::find()
        .select_only()
        .column(users::Column::Id)
        .column(users::Column::Email)
        .column(users::Column::Name)
        .column(users::Column::Status)
        .column_as(users::Column::AzureId, "azure_id")
        .column_as(users::Column::GoogleId, "google_id")
        .column_as(users::Column::MfaEnabled, "mfa_enabled")
        .column_as(users::Column::DepartmentId, "department_id")
        .column_as(users::Column::CreatedAt, "created_at")
        .column_as(users::Column::UpdatedAt, "updated_at")
        .column_as(users::Column::LastLoginAt, "last_login_at")
        .join(JoinType::LeftJoin, users::Relation::Departments.def())
        .column_as(departments::Column::Name, "department_name");
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
    if query.unassigned_department.unwrap_or(false) {
        select = select.filter(users::Column::DepartmentId.is_null())
    }
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
                Json(PaginatedUsers {
                    users: Vec::new(),
                    total: 0,
                    limit,
                    offset,
                }),
            ));
        }
        select = select.filter(users::Column::Id.is_in(role_user_ids));
    }
    if let Some(status) = query.status {
        select = select.filter(users::Column::Status.eq(status))
    }
    let sort_type = if query.ascending.unwrap_or(false) {
        Order::Asc
    } else {
        Order::Desc
    };
    if let Some(sort) = query.sort {
        select = match sort {
            SortRule::Name => select.order_by(users::Column::Name, sort_type),
            SortRule::Email => select.order_by(users::Column::Email, sort_type),
            SortRule::CreatedAt => select.order_by(users::Column::CreatedAt, sort_type),
            SortRule::UpdatedAt => select.order_by(users::Column::UpdatedAt, sort_type),
            SortRule::LastLoginAt => select.order_by(users::Column::LastLoginAt, sort_type),
            _ => select.order_by(users::Column::CreatedAt, sort_type),
        };
    }
    let select = select.into_model::<UserDepartmentRow>();
    let paginator = select.paginate(&app_state.database, limit);
    response.total = paginator.num_items().await.map_err(|e| {
        eprintln!("db get many error: {}", e);
        AuthError::DbTimeout
    })?;
    let rows = paginator.fetch_page(page).await.map_err(|e| {
        eprintln!("db get many error: {}", e);
        AuthError::DbTimeout
    })?;
    let user_ids: Vec<Uuid> = rows.iter().map(|u| u.id).collect();
    let roles_map = authz.user_roles_map(&user_ids).await?;
    response.users = rows
        .into_iter()
        .map(|user| {
            let roles = roles_map.get(&user.id).cloned().unwrap_or_default();
            let is_super_admin = roles.iter().any(|r| r == "Super Admin");
            User {
                id: user.id,
                sub: user
                    .google_id
                    .unwrap_or(user.azure_id.unwrap_or(user.email.clone())),
                email: user.email,
                name: user.name,
                picture: user.picture,
                hd: None,
                roles,
                status: user.status,
                department_id: user.department_id,
                department: user.department_name, // TODO
                is_super_admin,
                has_password: user.password.is_some(), // SSO-only users don't have password
                mfa_enabled: user.mfa_enabled,
                last_login_at: Some(user.last_login_at),
                password_changed_at: None,
                created_at: user.created_at,
                updated_at: user.updated_at,
                effective_permissions: None,
            }
        })
        .collect::<Vec<_>>();
    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    post,
    path = "/admin/users",
    tag = "admin",
    request_body = UserCreate,
    responses(
       (status = 201, description = "User added successfully"),
       (status = 409, content_type = "application/json", body = Error, description = "Email already exists (code=6106)"),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 404, content_type = "application/json", body = Error, description = "User not found (code=5003)"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),
     
    )
)]
pub async fn add_new_user(
    claims: Claims,
    State(app_state): State<SharedState>,
    Json(req): Json<UserCreate>,
) -> Result<(StatusCode, &'static str), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_USERS_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;
    let user_id = Uuid::new_v4();
    let now = Utc::now();
    let user = users::ActiveModel {
        id: Set(user_id),
        status: Set(UserStatus::Active),
        picture: Set(None),
        email: Set(req.email.trim().to_string()),
        email_verified: Set(false),
        name: Set(Some(req.name)),
        password: Set(None),
        google_id: Set(None),
        azure_id: Set(None),
        mfa_enabled: Set(false),
        mfa_secret: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        last_login_at: Set(now),
        password_changed_at: Set(None),
        hd: Set(req
            .email
            .trim()
            .split_once("@")
            .map(|splited| splited.1.to_string())),
        department_id: Set(req.department_id),
        is_independent: Set(false),
        effective_permissions: Set(None),
        metadata: Set(None),
        identities: Set(None),
    };
    user.insert(&app_state.database).await.map_err(|e| {
        eprintln!("insert error: {e}");
        AuthError::DbTimeout
    })?;
    let role = roles::Entity::find()
        .filter(roles::Column::Name.eq("User"))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;
    let assignment = user_role_assignments::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(user_id),
        role_id: Set(role.id),
        scope_department_id: Set(None),
        assigned_by: Set(claims.user_id),
        created_at: Set(now),
        updated_at: Set(now),
    };
    assignment.insert(&app_state.database).await.map_err(|e| {
        eprintln!("role assignment insert error: {e}");
        AuthError::DbTimeout
    })?;
    let _ = authz.recompute_effective_permissions(user_id).await;
    Ok((StatusCode::CREATED, "User added successfully"))
}

#[utoipa::path(
    put,
    path = "/admin/users/{user_id}",
    tag = "admin",
    params(
        ("user_id" = Uuid, Path, description = "User id")
    ),
    request_body = UserUpdate,
    responses(
       (status = 200, description = "User updated"),
       (status = 409, content_type = "application/json", body = Error, description = "Email already exists (code=6106)"),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 404, content_type = "application/json", body = Error, description = "User not found (code=5003)"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),
    )
)]
pub async fn update_user(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<UserUpdate>,
) -> Result<StatusCode, AuthError> {
    let model = users::Entity::find_by_id(user_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db find error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::EmailDoesNotExist)?;
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_USERS_MANAGE,
            model.department_id,
            PermissionScopeMode::RequireOrgWide,
            Some(user_id),
        )
        .await?;

    let original_department_id = model.department_id;
    let mut active: users::ActiveModel = model.into();
    let mut department_changed = false;

    if let Some(email) = req.email {
        let email = email.trim().to_string();
        active.email = Set(email.clone());
        active.hd = Set(email.split('@').nth(1).map(|s| s.to_string()));
        active.email_verified = Set(false);
    }
    if let Some(name) = req.name {
        active.name = Set(Some(name));
    }
    if req.unassign_department.unwrap_or(false) {
        if original_department_id.is_some() {
            department_changed = true;
        }
        active.department_id = Set(None);
    } else if let Some(dept) = req.department_id {
        authz
            .ensure_permission(
                claims.user_id,
                PERMISSION_USERS_MANAGE,
                Some(dept),
                PermissionScopeMode::RequireOrgWide,
                Some(user_id),
            )
            .await?;
        if Some(dept) != original_department_id {
            department_changed = true;
        }
        active.department_id = Set(Some(dept));
    }
    active.updated_at = Set(Utc::now());
    active.update(&app_state.database).await.map_err(|e| {
        let s = e.to_string();
        if s.contains("23505") || s.contains("duplicate key value violates unique constraint") {
            AuthError::EmailAlreadyExist
        } else {
            eprintln!("db update error: {e}");
            AuthError::DbTimeout
        }
    })?;

    if department_changed {
        let _ = authz.recompute_effective_permissions(user_id).await;
    }

    Ok(StatusCode::OK)
}

#[utoipa::path(
    patch,
    path = "/admin/users/{user_id}/status",
    tag = "admin",
    params(
        ("user_id" = Uuid, Path, description = "User id")
    ),
    request_body = UserPatchRequest,
    responses(
       (status = 200, description = "User status updated successfully"),
       (status = 409, content_type = "application/json", body = Error, description = "Super admin cannot deactivate/suspend/delete their own account"),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 404, content_type = "application/json", body = Error, description = "User not found (code=5003)"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),

    )
)]
pub async fn patch_user_status(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<UserPatchRequest>,
) -> Result<(StatusCode, &'static str), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    let is_super_admin = authz
        .user_has_role_name(claims.user_id, "Super Admin")
        .await?;
    if is_super_admin
        && claims.user_id == user_id
        && matches!(
            req.status,
            UserStatus::Deactivated | UserStatus::Suspended | UserStatus::Deleted
        )
    {
        return Err(AuthError::SuperAdminSelfStatusConflict);
    }
    match req.status {
        UserStatus::Active | UserStatus::Deactivated | UserStatus::Suspended => {}
        _ => return Err(AuthError::InvalidUserStatus),
    }
    let model = users::Entity::find_by_id(user_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db find error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::EmailDoesNotExist)?;
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_USERS_MANAGE,
            model.department_id,
            PermissionScopeMode::RequireOrgWide,
            Some(user_id),
        )
        .await?;
    let mut active: users::ActiveModel = model.into();
    active.status = Set(req.status);
    active.updated_at = Set(Utc::now());
    active.update(&app_state.database).await.map_err(|e| {
        eprintln!("db update error: {e}");
        AuthError::DbTimeout
    })?;
    Ok((StatusCode::OK, "User status updated successfully"))
}

#[utoipa::path(
    delete,
    path = "/admin/users/{user_id}",
    tag = "admin",
    params(
        ("user_id" = Uuid, Path, description = "User id")
    ),
    responses(
        (status = 204, description = "User deleted"),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 404, content_type = "application/json", body = Error, description = "User not found (code=5003)"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),
    )
)]
pub async fn delete_user(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(user_id): Path<Uuid>,
) -> Result<StatusCode, AuthError> {
    let user = users::Entity::find_by_id(user_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db find error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::EmailDoesNotExist)?;
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_USERS_MANAGE,
            user.department_id,
            PermissionScopeMode::RequireOrgWide,
            Some(user_id),
        )
        .await?;
    let mut active_model = user.into_active_model();
    active_model.updated_at = Set(Utc::now());
    active_model.status = Set(UserStatus::Deleted);
    active_model
        .update(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db find error: {e}");
            AuthError::DbTimeout
        })?;
    Ok(StatusCode::NO_CONTENT)
}
