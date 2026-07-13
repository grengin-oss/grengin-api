use axum::{
    Json,
    extract::{Path, State},
};
use chrono::Utc;
use reqwest::StatusCode;
use sea_orm::sea_query::{Alias, BinOper, Expr};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, JoinType, QueryFilter, QuerySelect, RelationTrait,
    Set,
};
use uuid::Uuid;

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
        permissions::{PERMISSION_MCP_ADMIN, PERMISSION_MCP_DELEGATE, PERMISSION_MCP_VIEW},
    },
    dto::admin_mcp::{
        McpAccessDefault, McpAccessDefaultChangedPayload, McpAccessRuleCreatedPayload,
        McpAccessRuleDeletedPayload, McpAccessRuleRequest, McpServerAccess,
    },
    dto::mcp::{McpAccessRule, McpAccessRuleInput, McpServerAccessUpdate},
    models::{
        departments, mcp_access_policies,
        mcp_access_policies::{McpAccessTarget, McpAccessType},
        mcp_servers, roles, user_role_assignments, users,
    },
    services::{
        auth_audit::{build_audit_payload, record_auth_event},
        authorization::{AuthorizationService, PermissionScopeMode},
        mcp_access::resolve_role_reference,
        mcp_helpers::build_access_rule_dtos,
    },
    state::SharedState,
};

#[utoipa::path(
    get,
    path = "/admin/mcp-servers/{server_id}/access",
    tag = "admin",
    params(
        ("server_id" = Uuid, Path, description = "MCP server id")
    ),
    responses(
        (status = 200, body = McpServerAccess),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error)
    )
)]
pub async fn get_mcp_server_access(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(server_id): Path<Uuid>,
) -> Result<(StatusCode, Json<McpServerAccess>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_MCP_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            Some(server_id),
        )
        .await?;

    let server = mcp_servers::Entity::find_by_id(server_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp server lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    let rules = mcp_access_policies::Entity::find()
        .filter(mcp_access_policies::Column::TargetType.eq(McpAccessTarget::Server))
        .filter(mcp_access_policies::Column::ServerId.eq(server_id))
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp rules lookup error: {e}");
            AuthError::DbTimeout
        })?;

    let rule_dtos = build_access_rule_dtos(&app_state.database, rules)
        .await
        .map_err(|_| AuthError::DbTimeout)?;

    Ok((
        StatusCode::OK,
        Json(McpServerAccess {
            server_id: server.id,
            default_access: server.default_access,
            rules: rule_dtos,
        }),
    ))
}

#[utoipa::path(
    put,
    path = "/admin/mcp-servers/{server_id}/access",
    tag = "admin",
    request_body = McpServerAccessUpdate,
    params(
        ("server_id" = Uuid, Path, description = "MCP server id")
    ),
    responses(
        (status = 200, body = McpServerAccess),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error)
    )
)]
pub async fn update_mcp_server_access(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(server_id): Path<Uuid>,
    Json(req): Json<McpServerAccessUpdate>,
) -> Result<(StatusCode, Json<McpServerAccess>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_MCP_ADMIN,
            None,
            PermissionScopeMode::RequireOrgWide,
            Some(server_id),
        )
        .await?;

    if let Some(default_access) = req.default_access {
        if let Some(server) = mcp_servers::Entity::find_by_id(server_id)
            .one(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("mcp server lookup error: {e}");
                AuthError::DbTimeout
            })?
        {
            let mut active: mcp_servers::ActiveModel = server.into();
            active.default_access = Set(default_access);
            active.updated_at = Set(Utc::now());
            active.update(&app_state.database).await.map_err(|e| {
                eprintln!("mcp server update error: {e}");
                AuthError::DbTimeout
            })?;
        }
    }

    let _ = mcp_access_policies::Entity::delete_many()
        .filter(mcp_access_policies::Column::TargetType.eq(McpAccessTarget::Server))
        .filter(mcp_access_policies::Column::ServerId.eq(server_id))
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp rule delete error: {e}");
            AuthError::DbTimeout
        })?;

    if let Some(rules) = req.rules {
        for r in rules {
            let inherit_departments = if r.access_type == McpAccessType::Department {
                r.inherit_departments.unwrap_or(true)
            } else {
                true
            };
            let (role_id, _) = resolve_role_reference(
                &app_state.database,
                r.access_type,
                r.role_id,
                r.role_name.clone(),
            )
            .await?;
            let model = mcp_access_policies::ActiveModel {
                id: Set(Uuid::new_v4()),
                target_type: Set(McpAccessTarget::Server),
                server_id: Set(Some(server_id)),
                tool_id: Set(None),
                access_type: Set(r.access_type),
                permission: Set(r.permission),
                role_id: Set(role_id),
                role_name: Set(None),
                department_id: Set(r.department_id),
                user_id: Set(r.user_id),
                inherit_departments: Set(inherit_departments),
                inherit_from_server: Set(None),
                created_at: Set(Utc::now()),
                created_by: Set(Some(claims.user_id)),
            };
            model.insert(&app_state.database).await.map_err(|e| {
                eprintln!("mcp rule insert error: {e}");
                AuthError::DbTimeout
            })?;
        }
    }

    let _ = authz.recompute_effective_permissions_for_all_users().await;

    get_mcp_server_access(claims, State(app_state), Path(server_id)).await
}

#[utoipa::path(
    put,
    path = "/admin/mcp-servers/{server_id}/access/default",
    tag = "admin",
    request_body = McpAccessDefault,
    params(
        ("server_id" = Uuid, Path, description = "MCP server id")
    ),
    responses(
        (status = 200, body = McpServerAccess),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error)
    )
)]
pub async fn update_mcp_server_default(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(server_id): Path<Uuid>,
    Json(req): Json<McpAccessDefault>,
) -> Result<(StatusCode, Json<McpServerAccess>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_MCP_ADMIN,
            None,
            PermissionScopeMode::RequireOrgWide,
            Some(server_id),
        )
        .await?;

    let server = mcp_servers::Entity::find_by_id(server_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp server lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    let mut server_active: mcp_servers::ActiveModel = server.clone().into();
    server_active.default_access = Set(req.default_access);
    server_active.updated_at = Set(Utc::now());
    server_active
        .update(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp server update error: {e}");
            AuthError::DbTimeout
        })?;

    let rules = mcp_access_policies::Entity::find()
        .filter(mcp_access_policies::Column::TargetType.eq(McpAccessTarget::Server))
        .filter(mcp_access_policies::Column::ServerId.eq(server_id))
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp rules lookup error: {e}");
            AuthError::DbTimeout
        })?;

    let rule_dtos = build_access_rule_dtos(&app_state.database, rules)
        .await
        .map_err(|_| AuthError::DbTimeout)?;

    if let Some(payload) = build_audit_payload(McpAccessDefaultChangedPayload {
        server_id,
        default_access: req.default_access,
    }) {
        let _ = record_auth_event(
            &app_state.database,
            "auth.mcp_access_default_changed",
            Some(claims.user_id),
            payload,
        )
        .await;
    }

    let _ = authz.recompute_effective_permissions_for_all_users().await;

    Ok((
        StatusCode::OK,
        Json(McpServerAccess {
            server_id: server_id,
            default_access: req.default_access,
            rules: rule_dtos,
        }),
    ))
}

#[utoipa::path(
    post,
    path = "/admin/mcp-servers/{server_id}/access/rules",
    tag = "admin",
    request_body = McpAccessRuleInput,
    params(
        ("server_id" = Uuid, Path, description = "MCP server id")
    ),
    responses(
        (status = 201, body = McpAccessRule),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
        (status = 409, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error)
    )
)]
pub async fn create_mcp_access_rule(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(server_id): Path<Uuid>,
    Json(req): Json<McpAccessRuleRequest>,
) -> Result<(StatusCode, Json<McpAccessRule>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);

    let mut target_department_scope: Option<Uuid> = None;
    match req.access_type {
        McpAccessType::Role => {
            authz
                .ensure_permission(
                    claims.user_id,
                    PERMISSION_MCP_ADMIN,
                    None,
                    PermissionScopeMode::RequireOrgWide,
                    Some(server_id),
                )
                .await?;
        }
        McpAccessType::User | McpAccessType::Department => {
            if req.access_type == McpAccessType::User {
                let user_id = req.user_id.ok_or(AuthError::ResourceNotFound)?;
                let user = users::Entity::find_by_id(user_id)
                    .one(&app_state.database)
                    .await
                    .map_err(|e| {
                        eprintln!("user lookup error: {e}");
                        AuthError::DbTimeout
                    })?
                    .ok_or(AuthError::ResourceNotFound)?;
                target_department_scope = user.department_id;
            } else if req.access_type == McpAccessType::Department {
                target_department_scope = req.department_id;
            }

            authz
                .ensure_permission(
                    claims.user_id,
                    PERMISSION_MCP_DELEGATE,
                    target_department_scope,
                    PermissionScopeMode::RequireOrgWide,
                    Some(server_id),
                )
                .await?;
        }
    }

    let _ = mcp_servers::Entity::find_by_id(server_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp server lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    let target_user_id = if req.access_type == McpAccessType::User {
        Some(req.user_id.ok_or(AuthError::ResourceNotFound)?)
    } else {
        None
    };
    let target_department_id = if req.access_type == McpAccessType::Department {
        Some(req.department_id.ok_or(AuthError::ResourceNotFound)?)
    } else {
        None
    };
    let (target_role_id, target_role_name) = resolve_role_reference(
        &app_state.database,
        req.access_type,
        req.role_id,
        req.role_name.clone(),
    )
    .await?;
    let inherit_departments = if req.access_type == McpAccessType::Department {
        req.inherit_departments.unwrap_or(true)
    } else {
        true
    };

    if let Some(dept_id) = target_department_id {
        let exists = departments::Entity::find_by_id(dept_id)
            .select_only()
            .column(departments::Column::Id)
            .into_tuple::<Uuid>()
            .one(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("department lookup error: {e}");
                AuthError::DbTimeout
            })?;
        if exists.is_none() {
            return Err(AuthError::ResourceNotFound);
        }
    }

    if req.access_type == McpAccessType::Role && target_role_id.is_none() {
        return Err(AuthError::ResourceNotFound);
    }

    if let Some(user_id) = target_user_id {
        let exists = users::Entity::find_by_id(user_id)
            .select_only()
            .column(users::Column::Id)
            .into_tuple::<Uuid>()
            .one(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("user lookup error: {e}");
                AuthError::DbTimeout
            })?;
        if exists.is_none() {
            return Err(AuthError::ResourceNotFound);
        }
    }

    let rule_id = Uuid::new_v4();
    let now = Utc::now();
    let rule = mcp_access_policies::ActiveModel {
        id: Set(rule_id),
        target_type: Set(McpAccessTarget::Server),
        server_id: Set(Some(server_id)),
        tool_id: Set(None),
        access_type: Set(req.access_type),
        permission: Set(req.permission),
        role_id: Set(target_role_id),
        role_name: Set(None),
        department_id: Set(target_department_id),
        user_id: Set(target_user_id),
        inherit_departments: Set(inherit_departments),
        inherit_from_server: Set(None),
        created_by: Set(Some(claims.user_id)),
        created_at: Set(now),
    };

    let inserted = rule.insert(&app_state.database).await.map_err(|e| {
        eprintln!("mcp rule insert error: {e}");
        AuthError::DbTimeout
    })?;

    if let Some(payload) = build_audit_payload(McpAccessRuleCreatedPayload {
        rule_id,
        server_id,
        access_type: req.access_type,
        permission: req.permission,
        role_id: target_role_id,
        role_name: target_role_name.clone(),
        department_id: target_department_id,
        user_id: target_user_id,
        inherit_departments,
    }) {
        let _ = record_auth_event(
            &app_state.database,
            "auth.mcp_access_rule_created",
            Some(claims.user_id),
            payload,
        )
        .await;
    }

    let affected_users = match req.access_type {
        McpAccessType::User => target_user_id.into_iter().collect(),
        McpAccessType::Department => {
            if let Some(dept_id) = target_department_id {
                let department_path = departments::Entity::find_by_id(dept_id)
                    .select_only()
                    .column_as(Expr::cust("path::text"), "path")
                    .into_tuple::<String>()
                    .one(&app_state.database)
                    .await
                    .map_err(|e| {
                        eprintln!("department lookup error: {e}");
                        AuthError::DbTimeout
                    })?
                    .ok_or(AuthError::ResourceNotFound)?;

                users::Entity::find()
                    .select_only()
                    .column(users::Column::Id)
                    .join(JoinType::InnerJoin, users::Relation::Departments.def())
                    .filter(Expr::col(departments::Column::Path).binary(
                        BinOper::Custom("<@".into()),
                        Expr::val(department_path).cast_as(Alias::new("ltree")),
                    ))
                    .into_tuple::<Uuid>()
                    .all(&app_state.database)
                    .await
                    .map_err(|e| {
                        eprintln!("user lookup error: {e}");
                        AuthError::DbTimeout
                    })?
            } else {
                Vec::new()
            }
        }
        McpAccessType::Role => {
            if let Some(role_id) = target_role_id {
                let assignments = user_role_assignments::Entity::find()
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
                assignments
            } else {
                Vec::new()
            }
        }
    };

    let _ = authz
        .recompute_effective_permissions_for_users(&affected_users)
        .await;

    let rule_dtos = build_access_rule_dtos(&app_state.database, vec![inserted])
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    let rule = rule_dtos.into_iter().next().ok_or(AuthError::DbTimeout)?;
    Ok((StatusCode::CREATED, Json(rule)))
}

#[utoipa::path(
    delete,
    path = "/admin/mcp-servers/{server_id}/access/rules/{rule_id}",
    tag = "admin",
    params(
        ("server_id" = Uuid, Path, description = "MCP server id"),
        ("rule_id" = Uuid, Path, description = "Rule id")
    ),
    responses(
        (status = 204),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error)
    )
)]
pub async fn delete_mcp_access_rule(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path((server_id, rule_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AuthError> {
    let authz = AuthorizationService::new(&app_state.database);

    let rule = mcp_access_policies::Entity::find_by_id(rule_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp rule lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    if rule.target_type != McpAccessTarget::Server || rule.server_id != Some(server_id) {
        return Err(AuthError::ResourceNotFound);
    }

    if rule.access_type == McpAccessType::Role {
        authz
            .ensure_permission(
                claims.user_id,
                PERMISSION_MCP_ADMIN,
                None,
                PermissionScopeMode::RequireOrgWide,
                Some(server_id),
            )
            .await?;
    } else {
        let target_department_id = match rule.access_type {
            McpAccessType::User => {
                let Some(user_id) = rule.user_id else {
                    return Err(AuthError::ResourceNotFound);
                };
                let user = users::Entity::find_by_id(user_id)
                    .one(&app_state.database)
                    .await
                    .map_err(|e| {
                        eprintln!("user lookup error: {e}");
                        AuthError::DbTimeout
                    })?
                    .ok_or(AuthError::ResourceNotFound)?;
                user.department_id
            }
            McpAccessType::Department => rule.department_id,
            McpAccessType::Role => None,
        };

        authz
            .ensure_permission(
                claims.user_id,
                PERMISSION_MCP_DELEGATE,
                target_department_id,
                PermissionScopeMode::RequireOrgWide,
                Some(server_id),
            )
            .await?;
    }

    mcp_access_policies::Entity::delete_by_id(rule_id)
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp rule delete error: {e}");
            AuthError::DbTimeout
        })?;

    if let Some(payload) = build_audit_payload(McpAccessRuleDeletedPayload { rule_id, server_id }) {
        let _ = record_auth_event(
            &app_state.database,
            "auth.mcp_access_rule_deleted",
            Some(claims.user_id),
            payload,
        )
        .await;
    }

    let affected_users = match rule.access_type {
        McpAccessType::User => rule.user_id.into_iter().collect(),
        McpAccessType::Department => {
            if let Some(dept_id) = rule.department_id {
                if rule.inherit_departments {
                    let department_path = departments::Entity::find_by_id(dept_id)
                        .select_only()
                        .column_as(Expr::cust("path::text"), "path")
                        .into_tuple::<String>()
                        .one(&app_state.database)
                        .await
                        .map_err(|e| {
                            eprintln!("department lookup error: {e}");
                            AuthError::DbTimeout
                        })?;
                    if let Some(department_path) = department_path {
                        users::Entity::find()
                            .select_only()
                            .column(users::Column::Id)
                            .join(JoinType::InnerJoin, users::Relation::Departments.def())
                            .filter(Expr::col(departments::Column::Path).binary(
                                BinOper::Custom("<@".into()),
                                Expr::val(department_path).cast_as(Alias::new("ltree")),
                            ))
                            .into_tuple::<Uuid>()
                            .all(&app_state.database)
                            .await
                            .map_err(|e| {
                                eprintln!("user lookup error: {e}");
                                AuthError::DbTimeout
                            })?
                    } else {
                        Vec::new()
                    }
                } else {
                    users::Entity::find()
                        .select_only()
                        .column(users::Column::Id)
                        .filter(users::Column::DepartmentId.eq(dept_id))
                        .into_tuple::<Uuid>()
                        .all(&app_state.database)
                        .await
                        .map_err(|e| {
                            eprintln!("user lookup error: {e}");
                            AuthError::DbTimeout
                        })?
                }
            } else {
                Vec::new()
            }
        }
        McpAccessType::Role => {
            let role_id = if let Some(role_id) = rule.role_id {
                Some(role_id)
            } else if let Some(role_name) = rule.role_name.clone() {
                let role = roles::Entity::find()
                    .filter(roles::Column::Name.eq(role_name))
                    .one(&app_state.database)
                    .await
                    .map_err(|e| {
                        eprintln!("role lookup error: {e}");
                        AuthError::DbTimeout
                    })?;
                role.map(|r| r.id)
            } else {
                None
            };

            if let Some(role_id) = role_id {
                let assignments = user_role_assignments::Entity::find()
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
                assignments
            } else {
                Vec::new()
            }
        }
    };

    let _ = authz
        .recompute_effective_permissions_for_users(&affected_users)
        .await;

    Ok(StatusCode::NO_CONTENT)
}

