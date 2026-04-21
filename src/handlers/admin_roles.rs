use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;
use reqwest::StatusCode;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, JoinType, QueryFilter, QuerySelect, RelationTrait,
    Set,
};
use uuid::Uuid;

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
        permissions::{
            PERMISSION_ROLES_ASSIGN, PERMISSION_ROLES_MANAGE, PERMISSION_ROLES_VIEW,
            ROLE_DEPARTMENT_ADMIN,
        },
    },
    dto::admin_roles::{
        PermissionDto, PermissionsResponse, RoleAssignmentPayload, RoleCreatedPayload,
        RoleDeletedPayload, RoleDto, RoleRequest, RoleUpdateRequest, RoleUpdatedPayload,
        RolesResponse, UserRoleAssignmentDto, UserRoleAssignmentInput, UserRoleAssignmentsResponse,
    },
    models::{permissions, role_permissions, roles, user_role_assignments, users},
    services::{
        auth_audit::{build_audit_payload, record_auth_event},
        authorization::{AuthorizationService, PermissionScopeMode},
    },
    state::SharedState,
};

#[derive(Debug, sea_orm::FromQueryResult)]
struct RolePermissionRow {
    #[sea_orm(from_alias = "roleId")]
    role_id: Uuid,
    domain: String,
    action: String,
}

#[derive(Debug, sea_orm::FromQueryResult)]
struct RoleUserCountRow {
    #[sea_orm(from_alias = "role_id")]
    role_id: Uuid,
    #[sea_orm(from_alias = "user_count")]
    user_count: i64,
}

#[utoipa::path(
    get,
    path = "/admin/permissions",
    tag = "admin",
    responses(
        (status = 200, body = PermissionsResponse),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error)
    )
)]
pub async fn get_permissions(
    claims: Claims,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<PermissionsResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_ROLES_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let permissions_list = permissions::Entity::find()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("permissions lookup error: {e}");
            AuthError::DbTimeout
        })?;

    let permissions = permissions_list
        .into_iter()
        .map(|perm| PermissionDto {
            id: perm.id,
            domain: perm.domain,
            action: perm.action,
            is_scopeable: perm.is_scopeable,
            description_key: perm.description_key,
        })
        .collect();

    Ok((StatusCode::OK, Json(PermissionsResponse { permissions })))
}

#[utoipa::path(
    get,
    path = "/admin/roles",
    tag = "admin",
    responses(
        (status = 200, body = RolesResponse),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error)
    )
)]
pub async fn list_roles(
    claims: Claims,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<RolesResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_ROLES_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let roles_list = roles::Entity::find()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("roles lookup error: {e}");
            AuthError::DbTimeout
        })?;

    let role_permissions_rows = role_permissions::Entity::find()
        .select_only()
        .column(role_permissions::Column::RoleId)
        .column_as(permissions::Column::Domain, "domain")
        .column_as(permissions::Column::Action, "action")
        .join(
            JoinType::InnerJoin,
            role_permissions::Relation::Permissions.def(),
        )
        .into_model::<RolePermissionRow>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role permissions lookup error: {e}");
            AuthError::DbTimeout
        })?;

    let mut permission_map: std::collections::HashMap<Uuid, Vec<String>> =
        std::collections::HashMap::new();
    for row in role_permissions_rows {
        permission_map
            .entry(row.role_id)
            .or_default()
            .push(format!("{}:{}", row.domain, row.action));
    }

    let role_user_counts = user_role_assignments::Entity::find()
        .select_only()
        .column_as(user_role_assignments::Column::RoleId, "role_id")
        .column_as(
            Expr::col(user_role_assignments::Column::UserId).count_distinct(),
            "user_count",
        )
        .group_by(user_role_assignments::Column::RoleId)
        .into_model::<RoleUserCountRow>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role user count lookup error: {e}");
            AuthError::DbTimeout
        })?;
    let role_user_count_map: std::collections::HashMap<Uuid, u64> = role_user_counts
        .into_iter()
        .map(|row| (row.role_id, row.user_count.max(0) as u64))
        .collect();

    let roles_response = roles_list
        .into_iter()
        .map(|role| RoleDto {
            id: role.id,
            name: role.name,
            is_system: role.is_system,
            permissions: permission_map.remove(&role.id).unwrap_or_default(),
            user_count: role_user_count_map.get(&role.id).copied().unwrap_or(0),
        })
        .collect();

    Ok((
        StatusCode::OK,
        Json(RolesResponse {
            roles: roles_response,
        }),
    ))
}

#[utoipa::path(
    post,
    path = "/admin/roles",
    tag = "admin",
    request_body = RoleRequest,
    responses(
        (status = 201, body = RoleDto),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 409, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error)
    )
)]
pub async fn create_role(
    claims: Claims,
    State(app_state): State<SharedState>,
    Json(req): Json<RoleRequest>,
) -> Result<(StatusCode, Json<RoleDto>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_ROLES_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    if roles::Entity::find()
        .filter(roles::Column::Name.eq(req.name.clone()))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role lookup error: {e}");
            AuthError::DbTimeout
        })?
        .is_some()
    {
        return Err(AuthError::DbConflict);
    }

    let permission_models = permissions::Entity::find()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("permissions lookup error: {e}");
            AuthError::DbTimeout
        })?;

    let mut permission_lookup = std::collections::HashMap::new();
    for permission in permission_models {
        permission_lookup.insert(
            format!("{}:{}", permission.domain, permission.action),
            permission.id,
        );
    }

    let role_id = Uuid::new_v4();
    let now = Utc::now();
    let role_model = roles::ActiveModel {
        id: Set(role_id),
        name: Set(req.name.clone()),
        is_system: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    };

    role_model.insert(&app_state.database).await.map_err(|e| {
        eprintln!("role insert error: {e}");
        AuthError::DbTimeout
    })?;

    let mut assigned_permissions = Vec::new();
    let mut permission_ids = Vec::new();
    for key in &req.permissions {
        let permission_id = permission_lookup
            .get(key)
            .ok_or(AuthError::ResourceNotFound)?;
        permission_ids.push(*permission_id);
        assigned_permissions.push(key.clone());
    }

    if !permission_ids.is_empty() {
        let inserts: Vec<role_permissions::ActiveModel> = permission_ids
            .iter()
            .map(|permission_id| role_permissions::ActiveModel {
                role_id: Set(role_id),
                permission_id: Set(*permission_id),
            })
            .collect();

        role_permissions::Entity::insert_many(inserts)
            .exec(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("role permissions insert error: {e}");
                AuthError::DbTimeout
            })?;
    }

    if let Some(payload) = build_audit_payload(RoleCreatedPayload {
        role_id,
        name: req.name.clone(),
        permissions: assigned_permissions.clone(),
    }) {
        let _ = record_auth_event(
            &app_state.database,
            "auth.role_created",
            Some(claims.user_id),
            payload,
        )
        .await;
    }

    Ok((
        StatusCode::CREATED,
        Json(RoleDto {
            id: role_id,
            name: req.name,
            is_system: false,
            permissions: assigned_permissions,
            user_count: 0,
        }),
    ))
}

#[utoipa::path(
    get,
    path = "/admin/roles/{role_id}",
    tag = "admin",
    params(
        ("role_id" = Uuid, Path, description = "Role id")
    ),
    responses(
        (status = 200, body = RoleDto),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error)
    )
)]
pub async fn get_role_by_id(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(role_id): Path<Uuid>,
) -> Result<(StatusCode, Json<RoleDto>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_ROLES_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let role = roles::Entity::find_by_id(role_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    let permission_rows = role_permissions::Entity::find()
        .select_only()
        .column(role_permissions::Column::RoleId)
        .column_as(permissions::Column::Domain, "domain")
        .column_as(permissions::Column::Action, "action")
        .join(
            JoinType::InnerJoin,
            role_permissions::Relation::Permissions.def(),
        )
        .filter(role_permissions::Column::RoleId.eq(role_id))
        .into_model::<RolePermissionRow>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role permissions lookup error: {e}");
            AuthError::DbTimeout
        })?;

    let permissions = permission_rows
        .into_iter()
        .map(|row| format!("{}:{}", row.domain, row.action))
        .collect();
    let user_count = user_role_assignments::Entity::find()
        .select_only()
        .column_as(
            Expr::col(user_role_assignments::Column::UserId).count_distinct(),
            "user_count",
        )
        .filter(user_role_assignments::Column::RoleId.eq(role_id))
        .into_tuple::<i64>()
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role assignment count error: {e}");
            AuthError::DbTimeout
        })?
        .unwrap_or(0)
        .max(0) as u64;

    Ok((
        StatusCode::OK,
        Json(RoleDto {
            id: role.id,
            name: role.name,
            is_system: role.is_system,
            permissions,
            user_count,
        }),
    ))
}

#[utoipa::path(
    put,
    path = "/admin/roles/{role_id}",
    tag = "admin",
    request_body = RoleUpdateRequest,
    params(
        ("role_id" = Uuid, Path, description = "Role id")
    ),
    responses(
        (status = 200, body = RoleDto),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error)
    )
)]
pub async fn update_role(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(role_id): Path<Uuid>,
    Json(req): Json<RoleUpdateRequest>,
) -> Result<(StatusCode, Json<RoleDto>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_ROLES_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let role = roles::Entity::find_by_id(role_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    if role.is_system {
        return Err(AuthError::PermissionDenied);
    }

    let RoleUpdateRequest { name, permissions } = req;
    let mut role_active: roles::ActiveModel = role.clone().into();
    if let Some(name) = &name {
        role_active.name = Set(name.clone());
    }
    role_active.updated_at = Set(Utc::now());
    role_active.update(&app_state.database).await.map_err(|e| {
        eprintln!("role update error: {e}");
        AuthError::DbTimeout
    })?;

    let permissions_list: Vec<String>;

    if let Some(permission_keys) = permissions {
        role_permissions::Entity::delete_many()
            .filter(role_permissions::Column::RoleId.eq(role_id))
            .exec(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("role permissions delete error: {e}");
                AuthError::DbTimeout
            })?;

        let permission_models = permissions::Entity::find()
            .all(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("permissions lookup error: {e}");
                AuthError::DbTimeout
            })?;
        let mut permission_lookup = std::collections::HashMap::new();
        for permission in permission_models {
            permission_lookup.insert(
                format!("{}:{}", permission.domain, permission.action),
                permission.id,
            );
        }

        let mut inserts = Vec::new();
        for key in &permission_keys {
            let permission_id = permission_lookup
                .get(key)
                .ok_or(AuthError::ResourceNotFound)?;
            inserts.push(role_permissions::ActiveModel {
                role_id: Set(role_id),
                permission_id: Set(*permission_id),
            });
        }
        if !inserts.is_empty() {
            role_permissions::Entity::insert_many(inserts)
                .exec(&app_state.database)
                .await
                .map_err(|e| {
                    eprintln!("role permissions insert error: {e}");
                    AuthError::DbTimeout
                })?;
        }
        permissions_list = permission_keys;

        let _ = authz
            .recompute_effective_permissions_for_role(role_id)
            .await;
    } else {
        let permission_rows = role_permissions::Entity::find()
            .select_only()
            .column_as(permissions::Column::Domain, "domain")
            .column_as(permissions::Column::Action, "action")
            .join(
                JoinType::InnerJoin,
                role_permissions::Relation::Permissions.def(),
            )
            .filter(role_permissions::Column::RoleId.eq(role_id))
            .into_tuple::<(String, String)>()
            .all(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("role permissions lookup error: {e}");
                AuthError::DbTimeout
            })?;

        permissions_list = permission_rows
            .into_iter()
            .map(|(domain, action)| format!("{}:{}", domain, action))
            .collect();
    }

    let response_name = name.clone().unwrap_or(role.name.clone());
    if let Some(payload) = build_audit_payload(RoleUpdatedPayload {
        role_id,
        name: name.clone(),
        permissions: permissions_list.clone(),
    }) {
        let _ = record_auth_event(
            &app_state.database,
            "auth.role_updated",
            Some(claims.user_id),
            payload,
        )
        .await;
    }

    let user_count = user_role_assignments::Entity::find()
        .select_only()
        .column_as(
            Expr::col(user_role_assignments::Column::UserId).count_distinct(),
            "user_count",
        )
        .filter(user_role_assignments::Column::RoleId.eq(role_id))
        .into_tuple::<i64>()
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role assignment count error: {e}");
            AuthError::DbTimeout
        })?
        .unwrap_or(0)
        .max(0) as u64;

    Ok((
        StatusCode::OK,
        Json(RoleDto {
            id: role_id,
            name: response_name,
            is_system: role.is_system,
            permissions: permissions_list,
            user_count,
        }),
    ))
}

#[utoipa::path(
    delete,
    path = "/admin/roles/{role_id}",
    tag = "admin",
    params(
        ("role_id" = Uuid, Path, description = "Role id")
    ),
    responses(
        (status = 204),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error)
    )
)]
pub async fn delete_role(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(role_id): Path<Uuid>,
) -> Result<StatusCode, AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_ROLES_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let role = roles::Entity::find_by_id(role_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    if role.is_system {
        return Err(AuthError::PermissionDenied);
    }

    let assigned_users = user_role_assignments::Entity::find()
        .select_only()
        .column(user_role_assignments::Column::UserId)
        .filter(user_role_assignments::Column::RoleId.eq(role_id))
        .into_tuple::<Uuid>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role assignment lookup error: {e}");
            AuthError::DbTimeout
        })?;

    roles::Entity::delete_by_id(role_id)
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role delete error: {e}");
            AuthError::DbTimeout
        })?;

    let _ = authz
        .recompute_effective_permissions_for_users(&assigned_users)
        .await;

    if let Some(payload) = build_audit_payload(RoleDeletedPayload {
        role_id,
        name: role.name.clone(),
    }) {
        let _ = record_auth_event(
            &app_state.database,
            "auth.role_deleted",
            Some(claims.user_id),
            payload,
        )
        .await;
    }

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/admin/users/{user_id}/roles",
    tag = "admin",
    params(
        ("user_id" = Uuid, Path, description = "User id")
    ),
    responses(
        (status = 200, body = UserRoleAssignmentsResponse),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error)
    )
)]
pub async fn list_user_role_assignments(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(user_id): Path<Uuid>,
) -> Result<(StatusCode, Json<UserRoleAssignmentsResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_ROLES_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            Some(user_id),
        )
        .await?;

    let assignments = user_role_assignments::Entity::find()
        .filter(user_role_assignments::Column::UserId.eq(user_id))
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role assignment lookup error: {e}");
            AuthError::DbTimeout
        })?;

    let role_map: std::collections::HashMap<Uuid, String> = roles::Entity::find()
        .select_only()
        .column(roles::Column::Id)
        .column(roles::Column::Name)
        .into_tuple::<(Uuid, String)>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("roles lookup error: {e}");
            AuthError::DbTimeout
        })?
        .into_iter()
        .collect();

    let assignments_response = assignments
        .into_iter()
        .map(|assignment| UserRoleAssignmentDto {
            id: assignment.id,
            role_id: assignment.role_id,
            role_name: role_map
                .get(&assignment.role_id)
                .cloned()
                .unwrap_or_default(),
            scope_department_id: assignment.scope_department_id,
            assigned_by: assignment.assigned_by,
            created_at: assignment.created_at,
        })
        .collect();

    Ok((
        StatusCode::OK,
        Json(UserRoleAssignmentsResponse {
            assignments: assignments_response,
        }),
    ))
}

#[utoipa::path(
    post,
    path = "/admin/users/{user_id}/roles",
    tag = "admin",
    request_body = UserRoleAssignmentInput,
    params(
        ("user_id" = Uuid, Path, description = "User id")
    ),
    responses(
        (status = 201, body = UserRoleAssignmentDto),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
        (status = 409, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error)
    )
)]
pub async fn assign_role_to_user(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<UserRoleAssignmentInput>,
) -> Result<(StatusCode, Json<UserRoleAssignmentDto>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);

    let target_user = users::Entity::find_by_id(user_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("user lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    let role = roles::Entity::find_by_id(req.role_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    if role.name == ROLE_DEPARTMENT_ADMIN && req.scope_department_id.is_none() {
        return Err(AuthError::DbConflict);
    }

    let target_scope = req.scope_department_id.or(target_user.department_id);

    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_ROLES_ASSIGN,
            target_scope,
            PermissionScopeMode::RequireOrgWide,
            Some(user_id),
        )
        .await?;

    if role.name == ROLE_DEPARTMENT_ADMIN {
        authz
            .ensure_permission(
                claims.user_id,
                PERMISSION_ROLES_ASSIGN,
                req.scope_department_id,
                PermissionScopeMode::RequireOrgWide,
                Some(user_id),
            )
            .await?;
    }

    let assignment_id = Uuid::new_v4();
    let now = Utc::now();
    let assignment = user_role_assignments::ActiveModel {
        id: Set(assignment_id),
        user_id: Set(user_id),
        role_id: Set(req.role_id),
        scope_department_id: Set(req.scope_department_id),
        assigned_by: Set(claims.user_id),
        created_at: Set(now),
        updated_at: Set(now),
    };

    assignment.insert(&app_state.database).await.map_err(|e| {
        let s = e.to_string();
        if s.contains("duplicate key value violates unique constraint") {
            AuthError::DbConflict
        } else {
            eprintln!("role assignment insert error: {e}");
            AuthError::DbTimeout
        }
    })?;

    let _ = authz.recompute_effective_permissions(user_id).await;

    if let Some(payload) = build_audit_payload(RoleAssignmentPayload {
        assignment_id,
        user_id,
        role_id: req.role_id,
        scope_department_id: req.scope_department_id,
    }) {
        let _ = record_auth_event(
            &app_state.database,
            "auth.role_assigned",
            Some(claims.user_id),
            payload,
        )
        .await;
    }

    Ok((
        StatusCode::CREATED,
        Json(UserRoleAssignmentDto {
            id: assignment_id,
            role_id: req.role_id,
            role_name: role.name,
            scope_department_id: req.scope_department_id,
            assigned_by: claims.user_id,
            created_at: now,
        }),
    ))
}

#[utoipa::path(
    delete,
    path = "/admin/users/{user_id}/roles/{assignment_id}",
    tag = "admin",
    params(
        ("user_id" = Uuid, Path, description = "User id"),
        ("assignment_id" = Uuid, Path, description = "Assignment id")
    ),
    responses(
        (status = 204),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error)
    )
)]
pub async fn remove_role_from_user(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path((user_id, assignment_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AuthError> {
    let authz = AuthorizationService::new(&app_state.database);

    let assignment = user_role_assignments::Entity::find_by_id(assignment_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("assignment lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    if assignment.user_id != user_id {
        return Err(AuthError::ResourceNotFound);
    }

    let target_scope = assignment.scope_department_id;

    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_ROLES_ASSIGN,
            target_scope,
            PermissionScopeMode::RequireOrgWide,
            Some(user_id),
        )
        .await?;

    user_role_assignments::Entity::delete_by_id(assignment_id)
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("assignment delete error: {e}");
            AuthError::DbTimeout
        })?;

    let _ = authz.recompute_effective_permissions(user_id).await;

    if let Some(payload) = build_audit_payload(RoleAssignmentPayload {
        assignment_id,
        user_id,
        role_id: assignment.role_id,
        scope_department_id: assignment.scope_department_id,
    }) {
        let _ = record_auth_event(
            &app_state.database,
            "auth.role_unassigned",
            Some(claims.user_id),
            payload,
        )
        .await;
    }

    Ok(StatusCode::NO_CONTENT)
}
