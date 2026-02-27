use axum::{extract::{Path, State}, Json};
use chrono::Utc;
use reqwest::StatusCode;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, JoinType, QueryFilter, QuerySelect, RelationTrait, Set};
use sea_orm::sea_query::{Alias, BinOper, Expr};
use uuid::Uuid;

use crate::{
    auth::{claims::Claims, error::{AuthError, AuthErrorResponse}, permissions::{PERMISSION_MCP_ADMIN, PERMISSION_MCP_DELEGATE, PERMISSION_MCP_VIEW}},
    dto::admin_mcp::{
        McpAccessDefaultChangedPayload, McpAccessDefaultRequest, McpAccessRuleCreatedPayload,
        McpAccessRuleDeletedPayload, McpAccessRuleDto, McpAccessRuleRequest,
        McpServerAccessResponse,
    },
    models::{
        departments,
        mcp_server_access_rules::{self, McpSubjectType},
        mcp_servers,
        roles,
        user_role_assignments,
        users,
    },
    services::{
        authorization::{AuthorizationService, PermissionScopeMode},
        auth_audit::{build_audit_payload, record_auth_event},
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
        (status = 200, body = McpServerAccessResponse),
        (status = 401, content_type = "application/json", body = AuthErrorResponse),
        (status = 403, content_type = "application/json", body = AuthErrorResponse),
        (status = 404, content_type = "application/json", body = AuthErrorResponse),
        (status = 503, content_type = "application/json", body = AuthErrorResponse)
    )
)]
pub async fn get_mcp_server_access(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(server_id): Path<Uuid>,
) -> Result<(StatusCode, Json<McpServerAccessResponse>), AuthError> {
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

    let rules = mcp_server_access_rules::Entity::find()
        .filter(mcp_server_access_rules::Column::ServerId.eq(server_id))
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp rules lookup error: {e}");
            AuthError::DbTimeout
        })?;

    let rule_dtos = rules
        .into_iter()
        .map(|rule| McpAccessRuleDto {
            id: rule.id,
            subject_type: rule.subject_type,
            subject_id: rule.subject_id,
            rule_type: rule.rule_type,
            created_by: rule.created_by,
            created_at: rule.created_at,
        })
        .collect();

    Ok((
        StatusCode::OK,
        Json(McpServerAccessResponse {
            server_id: server.id,
            access_default: server.access_default,
            rules: rule_dtos,
        }),
    ))
}

#[utoipa::path(
    put,
    path = "/admin/mcp-servers/{server_id}/access/default",
    tag = "admin",
    request_body = McpAccessDefaultRequest,
    params(
        ("server_id" = Uuid, Path, description = "MCP server id")
    ),
    responses(
        (status = 200, body = McpServerAccessResponse),
        (status = 401, content_type = "application/json", body = AuthErrorResponse),
        (status = 403, content_type = "application/json", body = AuthErrorResponse),
        (status = 404, content_type = "application/json", body = AuthErrorResponse),
        (status = 503, content_type = "application/json", body = AuthErrorResponse)
    )
)]
pub async fn update_mcp_server_default(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(server_id): Path<Uuid>,
    Json(req): Json<McpAccessDefaultRequest>,
) -> Result<(StatusCode, Json<McpServerAccessResponse>), AuthError> {
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
    server_active.access_default = Set(req.access_default);
    server_active.updated_at = Set(Utc::now());
    server_active
        .update(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp server update error: {e}");
            AuthError::DbTimeout
        })?;

    let rules = mcp_server_access_rules::Entity::find()
        .filter(mcp_server_access_rules::Column::ServerId.eq(server_id))
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp rules lookup error: {e}");
            AuthError::DbTimeout
        })?;

    let rule_dtos = rules
        .into_iter()
        .map(|rule| McpAccessRuleDto {
            id: rule.id,
            subject_type: rule.subject_type,
            subject_id: rule.subject_id,
            rule_type: rule.rule_type,
            created_by: rule.created_by,
            created_at: rule.created_at,
        })
        .collect::<Vec<_>>();

    if let Some(payload) = build_audit_payload(McpAccessDefaultChangedPayload {
        server_id,
        access_default: req.access_default,
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
        Json(McpServerAccessResponse {
            server_id: server_id,
            access_default: req.access_default,
            rules: rule_dtos,
        }),
    ))
}

#[utoipa::path(
    post,
    path = "/admin/mcp-servers/{server_id}/access/rules",
    tag = "admin",
    request_body = McpAccessRuleRequest,
    params(
        ("server_id" = Uuid, Path, description = "MCP server id")
    ),
    responses(
        (status = 201, body = McpAccessRuleDto),
        (status = 401, content_type = "application/json", body = AuthErrorResponse),
        (status = 403, content_type = "application/json", body = AuthErrorResponse),
        (status = 404, content_type = "application/json", body = AuthErrorResponse),
        (status = 409, content_type = "application/json", body = AuthErrorResponse),
        (status = 503, content_type = "application/json", body = AuthErrorResponse)
    )
)]
pub async fn create_mcp_access_rule(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(server_id): Path<Uuid>,
    Json(req): Json<McpAccessRuleRequest>,
) -> Result<(StatusCode, Json<McpAccessRuleDto>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);

    if req.subject_type == McpSubjectType::Role {
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
        let target_department_id = match req.subject_type {
            McpSubjectType::User => {
                let user = users::Entity::find_by_id(req.subject_id)
                    .one(&app_state.database)
                    .await
                    .map_err(|e| {
                        eprintln!("user lookup error: {e}");
                        AuthError::DbTimeout
                    })?
                    .ok_or(AuthError::ResourceNotFound)?;
                user.department_id
            }
            McpSubjectType::Department => Some(req.subject_id),
            McpSubjectType::Role => None,
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

    let _ = mcp_servers::Entity::find_by_id(server_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp server lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    if req.subject_type == McpSubjectType::Department {
        let exists = departments::Entity::find_by_id(req.subject_id)
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

    if req.subject_type == McpSubjectType::Role {
        let exists = roles::Entity::find_by_id(req.subject_id)
            .one(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("role lookup error: {e}");
                AuthError::DbTimeout
            })?;
        if exists.is_none() {
            return Err(AuthError::ResourceNotFound);
        }
    }

    let rule_id = Uuid::new_v4();
    let now = Utc::now();
    let rule = mcp_server_access_rules::ActiveModel {
        id: Set(rule_id),
        server_id: Set(server_id),
        subject_type: Set(req.subject_type),
        subject_id: Set(req.subject_id),
        rule_type: Set(req.rule_type),
        created_by: Set(claims.user_id),
        created_at: Set(now),
    };

    rule.insert(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp rule insert error: {e}");
            AuthError::DbTimeout
        })?;

    if let Some(payload) = build_audit_payload(McpAccessRuleCreatedPayload {
        rule_id,
        server_id,
        subject_type: req.subject_type,
        subject_id: req.subject_id,
        rule_type: req.rule_type,
    }) {
        let _ = record_auth_event(
            &app_state.database,
            "auth.mcp_access_rule_created",
            Some(claims.user_id),
            payload,
        )
        .await;
    }

    let affected_users = match req.subject_type {
        McpSubjectType::User => vec![req.subject_id],
        McpSubjectType::Department => {
            let department_path = departments::Entity::find_by_id(req.subject_id)
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
                .filter(
                    Expr::col(departments::Column::Path).binary(
                        BinOper::Custom("<@".into()),
                        Expr::val(department_path).cast_as(Alias::new("ltree")),
                    ),
                )
                .into_tuple::<Uuid>()
                .all(&app_state.database)
                .await
                .map_err(|e| {
                    eprintln!("user lookup error: {e}");
                    AuthError::DbTimeout
                })?
        }
        McpSubjectType::Role => {
            let assignments = user_role_assignments::Entity::find()
                .select_only()
                .column(user_role_assignments::Column::UserId)
                .filter(user_role_assignments::Column::RoleId.eq(req.subject_id))
                .into_tuple::<Uuid>()
                .all(&app_state.database)
                .await
                .map_err(|e| {
                    eprintln!("role assignment lookup error: {e}");
                    AuthError::DbTimeout
                })?;
            assignments
        }
    };

    let _ = authz.recompute_effective_permissions_for_users(&affected_users).await;

    Ok((
        StatusCode::CREATED,
        Json(McpAccessRuleDto {
            id: rule_id,
            subject_type: req.subject_type,
            subject_id: req.subject_id,
            rule_type: req.rule_type,
            created_by: claims.user_id,
            created_at: now,
        }),
    ))
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
        (status = 401, content_type = "application/json", body = AuthErrorResponse),
        (status = 403, content_type = "application/json", body = AuthErrorResponse),
        (status = 404, content_type = "application/json", body = AuthErrorResponse),
        (status = 503, content_type = "application/json", body = AuthErrorResponse)
    )
)]
pub async fn delete_mcp_access_rule(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path((server_id, rule_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AuthError> {
    let authz = AuthorizationService::new(&app_state.database);

    let rule = mcp_server_access_rules::Entity::find_by_id(rule_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp rule lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    if rule.server_id != server_id {
        return Err(AuthError::ResourceNotFound);
    }

    if rule.subject_type == McpSubjectType::Role {
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
        let target_department_id = match rule.subject_type {
            McpSubjectType::User => {
                let user = users::Entity::find_by_id(rule.subject_id)
                    .one(&app_state.database)
                    .await
                    .map_err(|e| {
                        eprintln!("user lookup error: {e}");
                        AuthError::DbTimeout
                    })?
                    .ok_or(AuthError::ResourceNotFound)?;
                user.department_id
            }
            McpSubjectType::Department => Some(rule.subject_id),
            McpSubjectType::Role => None,
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

    mcp_server_access_rules::Entity::delete_by_id(rule_id)
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp rule delete error: {e}");
            AuthError::DbTimeout
        })?;

    if let Some(payload) = build_audit_payload(McpAccessRuleDeletedPayload {
        rule_id,
        server_id,
    }) {
        let _ = record_auth_event(
            &app_state.database,
            "auth.mcp_access_rule_deleted",
            Some(claims.user_id),
            payload,
        )
        .await;
    }

    let affected_users = match rule.subject_type {
        McpSubjectType::User => vec![rule.subject_id],
        McpSubjectType::Department => {
            let department_path = departments::Entity::find_by_id(rule.subject_id)
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
                    .filter(
                        Expr::col(departments::Column::Path).binary(
                            BinOper::Custom("<@".into()),
                            Expr::val(department_path).cast_as(Alias::new("ltree")),
                        ),
                    )
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
        McpSubjectType::Role => {
            let assignments = user_role_assignments::Entity::find()
                .select_only()
                .column(user_role_assignments::Column::UserId)
                .filter(user_role_assignments::Column::RoleId.eq(rule.subject_id))
                .into_tuple::<Uuid>()
                .all(&app_state.database)
                .await
                .map_err(|e| {
                    eprintln!("role assignment lookup error: {e}");
                    AuthError::DbTimeout
                })?;
            assignments
        }
    };

    let _ = authz.recompute_effective_permissions_for_users(&affected_users).await;

    Ok(StatusCode::NO_CONTENT)
}
