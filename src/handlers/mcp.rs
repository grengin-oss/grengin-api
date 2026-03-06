use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    Json,
};
use chrono::{Duration, Utc};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, Set,
};
use serde_json::json;
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    auth::{
        claims::Claims,
        encryption::encrypt_key,
        error::AuthError,
        permissions::{PERMISSION_MCP_ADMIN, PERMISSION_MCP_DELEGATE, PERMISSION_MCP_VIEW},
    },
    error::AppError,
    dto::mcp::{
        BulkToolAccessUpdate, BulkToolAccessUpdateResponse, ListExecutionsQuery, ListServersQuery,
        ListToolsQuery, McpAuthorizeQuery, McpAuthorizeResponse, McpOauthCallbackQuery, McpServer,
        McpServerAccessList,
        McpServerAccessUpdate, McpServerCreate, McpServerTestResult, McpServerUpdate, McpSyncResult,
        McpTool, McpToolAccessList, McpToolAccessUpdate, McpToolsList,
        McpUserConnection, McpUserConnectionsList, PaginatedMcpServers, PaginatedMcpToolExecutions,
    },
    models::{
        mcp_access_policies,
        mcp_access_policies::McpAccessTarget,
        mcp_connections, mcp_executions, mcp_oauth_states, mcp_servers,
        mcp_servers::McpDefaultAccess,
        mcp_tools,
    },
    llm::tooling::sanitize_tool_name,
    services::authorization::{AuthorizationService, PermissionScopeMode},
    services::mcp_client::{build_authorization_url, exchange_code},
    services::mcp_helpers::{
        build_oauth_config,
        encrypt_db_url_in_config,
        resolve_mcp_oauth_token,
        store_oauth_tokens,
        to_access_rule_dto,
        to_execution_dto,
        to_server_dto,
        to_tool_dto,
        upsert_connection,
    },
    state::SharedState,
};

#[utoipa::path(
    get,
    path = "/admin/mcp-servers",
    tag = "admin",
    params(
        ("status" = Option<String>, Query, description = "Filter servers by status"),
        ("enabled" = Option<bool>, Query, description = "Filter servers by enabled flag")
    ),
    responses(
        (status = 200, body = PaginatedMcpServers)
    )
)]
pub async fn list_mcp_servers(
    claims: Claims,
    State(state): State<SharedState>,
    Query(query): Query<ListServersQuery>,
) -> Result<Json<PaginatedMcpServers>, AuthError> {
    let authz = AuthorizationService::new(&state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_MCP_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;
    let mut finder = mcp_servers::Entity::find()
     .order_by_desc(mcp_servers::Column::CreatedAt);
    if let Some(enabled) = query.enabled {
        finder = finder.filter(mcp_servers::Column::Enabled.eq(enabled));
    }
    if let Some(status) = query.status {
        finder = finder.filter(mcp_servers::Column::Status.eq(status));
    }
    let servers = finder
        .all(&state.database)
        .await
        .map_err(|e|{
            eprintln!("Db get error {}",e);
            AuthError::DbTimeout
        })?;
    let dtos: Vec<_> = servers
        .into_iter()
        .map(|server| to_server_dto(&state, server))
        .collect();
    let total = dtos.len() as i64;
    Ok(Json(PaginatedMcpServers {
        servers: dtos,
        total,
    }))
}

#[utoipa::path(
    post,
    path = "/admin/mcp-servers",
    tag = "admin",
    request_body = McpServerCreate,
    responses(
        (status = 201, body = McpServer),
        (status = 500, description = "Database error")
    )
)]
pub async fn create_mcp_server(
    claims: Claims,
    State(state): State<SharedState>,
    Json(req): Json<McpServerCreate>,
) -> Result<(StatusCode, Json<McpServer>), AuthError> {
    let authz = AuthorizationService::new(&state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_MCP_ADMIN,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;
    let now = Utc::now();
    let server_name = req.name.clone();
    let mut connection_config = req.connection_config;
    encrypt_db_url_in_config(&state.settings.auth.app_key, &mut connection_config)
        .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    let client_secret = match req.client_secret {
        Some(ref secret) if !secret.is_empty() => Some(
            encrypt_key(&state.settings.auth.app_key, secret.as_bytes())
                .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?,
        ),
        _ => None,
    };
    let model = mcp_servers::ActiveModel {
        id: Set(Uuid::new_v4()),
        name: Set(req.name),
        description: Set(req.description),
        transport_type: Set(req.transport_type),
        connection_config: Set(connection_config),
        client_id: Set(req.client_id),
        client_secret: Set(client_secret),
        url: Set(req.url),
        enabled: Set(req.enabled),
        status: Set(Some("disconnected".into())),
        status_message: Set(None),
        tool_count: Set(0),
        access_default: Set(mcp_servers::McpAccessDefault::Deny),
        default_access: Set(req.default_access.unwrap_or(McpDefaultAccess::ExplicitOnly)),
        last_connected_at: Set(None),
        last_synced_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    };
    let saved = model
        .insert(&state.database)
        .await
        .map_err(|e| {
            let s = e.to_string();
            if s.contains("duplicate key value violates unique constraint")
                || s.contains("uq-mcp-servers-name")
            {
                AuthError::McpServerNameConflict {
                    name: Some(server_name.clone()),
                }
            } else {
                eprintln!("Db create error {}", e);
                AuthError::DbTimeout
            }
        })?;
    state.upsert_mcp_client(&saved).await;
    Ok((StatusCode::CREATED, Json(to_server_dto(&state, saved))))
}

#[utoipa::path(
    get,
    path = "/admin/mcp-servers/{server_id}",
    tag = "admin",
    params(
        ("server_id" = Uuid, Path, description = "MCP server id")
    ),
    responses(
        (status = 200, body = McpServer),
        (status = 404, description = "Server not found")
    )
)]
pub async fn get_mcp_server(
    claims: Claims,
    State(state): State<SharedState>,
    Path(server_id): Path<Uuid>,
) -> Result<Json<McpServer>, AuthError> {
    let authz = AuthorizationService::new(&state.database);
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
        .one(&state.database)
        .await
        .map_err(|e|{
            eprintln!("Db get one error {}",e);
            AuthError::DbTimeout
         })?
        .ok_or(AuthError::ResourceNotFound)?;
    Ok(Json(to_server_dto(&state, server)))
}

#[utoipa::path(
    put,
    path = "/admin/mcp-servers/{server_id}",
    tag = "admin",
    params(
        ("server_id" = Uuid, Path, description = "MCP server id")
    ),
    request_body = McpServerUpdate,
    responses(
        (status = 200, body = McpServer),
        (status = 404, description = "Server not found")
    )
)]
pub async fn update_mcp_server(
    claims: Claims,
    State(state): State<SharedState>,
    Path(server_id): Path<Uuid>,
    Json(req): Json<McpServerUpdate>,
) -> Result<Json<McpServer>, AuthError> {
    let authz = AuthorizationService::new(&state.database);
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
        .one(&state.database)
        .await
        .map_err(|_| AuthError::DbUnavailable)?
        .ok_or(AuthError::ResourceNotFound)?;
    let mut active: mcp_servers::ActiveModel = server.into();
    let requested_name = req.name.clone();
    if let Some(name) = req.name {
        active.name = Set(name);
    }
    if let Some(desc) = req.description {
        active.description = Set(Some(desc));
    }
    if let Some(cfg) = req.connection_config {
        let mut cfg = cfg;
        encrypt_db_url_in_config(&state.settings.auth.app_key, &mut cfg)
            .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
        active.connection_config = Set(cfg);
    }
    if let Some(client_id) = req.client_id {
        active.client_id = Set(Some(client_id));
    }
    if let Some(url) = req.url {
        active.url = Set(Some(url));
    }
    if let Some(client_secret) = req.client_secret {
        let encrypted = if client_secret.is_empty() {
            None
        } else {
            Some(
                encrypt_key(&state.settings.auth.app_key, client_secret.as_bytes())
                    .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?,
            )
        };
        active.client_secret = Set(encrypted);
    }
    if let Some(enabled) = req.enabled {
        active.enabled = Set(enabled);
    }
    if let Some(default_access) = req.default_access {
        active.default_access = Set(default_access);
    }
    active.updated_at = Set(Utc::now());
    let saved = active
        .update(&state.database)
        .await
        .map_err(|e| {
            let s = e.to_string();
            if s.contains("duplicate key value violates unique constraint")
                || s.contains("uq-mcp-servers-name")
            {
                AuthError::McpServerNameConflict {
                    name: requested_name.clone(),
                }
            } else {
                eprintln!("Db get error {}", e);
                AuthError::DbTimeout
            }
        })?;
    state.upsert_mcp_client(&saved).await;
    Ok(Json(to_server_dto(&state, saved)))
}

#[utoipa::path(
    delete,
    path = "/admin/mcp-servers/{server_id}",
    tag = "admin",
    params(
        ("server_id" = Uuid, Path, description = "MCP server id")
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Server not found")
    )
)]
pub async fn delete_mcp_server(
    claims: Claims,
    State(state): State<SharedState>,
    Path(server_id): Path<Uuid>,
) -> Result<StatusCode, AuthError> {
    let authz = AuthorizationService::new(&state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_MCP_ADMIN,
            None,
            PermissionScopeMode::RequireOrgWide,
            Some(server_id),
        )
        .await?;
    mcp_servers::Entity::delete_by_id(server_id)
        .exec(&state.database)
        .await
        .map_err(|e|{
            eprintln!("Db get error {}",e);
            AuthError::DbTimeout
        })?;
    state.remove_mcp_client(&server_id).await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/admin/mcp-servers/{server_id}/test",
    tag = "admin",
    params(
        ("server_id" = Uuid, Path, description = "MCP server id")
    ),
    responses(
        (status = 200, body = McpServerTestResult)
    )
)]
pub async fn test_mcp_server(
    claims: Claims,
    State(state): State<SharedState>,
    Path(_server_id): Path<Uuid>,
) -> Result<Json<McpServerTestResult>, AuthError> {
    let authz = AuthorizationService::new(&state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_MCP_ADMIN,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;
    Ok(Json(McpServerTestResult {
        success: true,
        message: Some("Connection test not implemented; stub response".into()),
        latency_ms: None,
        available_tools: None,
        error: None,
    }))
}

#[utoipa::path(
    post,
    path = "/admin/mcp-servers/{server_id}/sync-tools",
    tag = "admin",
    params(
        ("server_id" = Uuid, Path, description = "MCP server id")
    ),
    responses(
        (status = 200, body = McpSyncResult)
    )
)]
pub async fn sync_mcp_server_tools(
    claims: Claims,
    State(state): State<SharedState>,
    Path(server_id): Path<Uuid>,
) -> Result<Json<McpSyncResult>, AuthError> {
    let authz = AuthorizationService::new(&state.database);
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
        .one(&state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp server lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    let client = {
        let clients = state.mcp_clients.read().await;
        clients.get(&server_id).cloned()
    }
    .ok_or(AuthError::ResourceNotFound)?;

    let requires_oauth = server.transport_type == mcp_servers::McpTransportType::Http
        && server.connection_config.get("oauth").is_some();
    let oauth_token = if requires_oauth {
        match resolve_mcp_oauth_token(&state, claims.user_id, server_id).await {
            Ok(token) => token,
            Err(err) => {
                eprintln!("mcp oauth token lookup error: {:?}", err);
                return Err(AuthError::ServiceTemporarilyUnavailable);
            }
        }
    } else {
        None
    };

    if requires_oauth && oauth_token.is_none() {
        eprintln!("mcp oauth token missing for server {server_id}");
        return Err(AuthError::ServiceTemporarilyUnavailable);
    }

    let tools = client
        .list_tools_with_auth(oauth_token)
        .await
        .map_err(|e| {
            eprintln!("mcp list tools error: {e}");
            AuthError::ServiceTemporarilyUnavailable
        })?;

    let existing_tools = mcp_tools::Entity::find()
        .filter(mcp_tools::Column::ServerId.eq(server_id))
        .all(&state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp tools lookup error: {e}");
            AuthError::DbTimeout
        })?;

    let mut existing_by_name: HashMap<String, mcp_tools::Model> = existing_tools
        .into_iter()
        .map(|tool| (tool.original_name.clone(), tool))
        .collect();

    let mut tools_added = 0;
    let mut tools_updated = 0;
    let mut tools_removed = 0;

    for tool in tools {
        let original_name = tool.name.to_string();
        let sanitized_name = sanitize_tool_name(tool.name.as_ref());
        let input_schema = serde_json::to_value(&tool.input_schema).unwrap_or_else(|_| json!({}));
        let parameters = input_schema.clone();
        let is_read_only = tool
            .annotations
            .as_ref()
            .and_then(|a| a.read_only_hint)
            .unwrap_or(false);

        if let Some(existing) = existing_by_name.remove(&original_name) {
            let inherit_from_server = existing.inherit_access_from_server;
            let mut active: mcp_tools::ActiveModel = existing.into();
            active.name = Set(sanitized_name);
            active.description = Set(tool.description.as_ref().map(|d| d.to_string()));
            active.input_schema = Set(input_schema);
            active.parameters = Set(parameters);
            active.enabled = Set(true);
            active.is_read_only = Set(is_read_only);
            active.inherit_access_from_server = Set(inherit_from_server);
            active.last_synced_at = Set(Some(Utc::now()));
            active.updated_at = Set(Utc::now());
            if let Err(e) = active.update(&state.database).await {
                eprintln!("mcp tool update error: {e}");
            } else {
                tools_updated += 1;
            }
        } else {
            let model = mcp_tools::ActiveModel {
                id: Set(Uuid::new_v4()),
                server_id: Set(server_id),
                server_name: Set(server.name.clone()),
                name: Set(sanitized_name),
                original_name: Set(original_name),
                description: Set(tool.description.as_ref().map(|d| d.to_string())),
                input_schema: Set(input_schema),
                parameters: Set(parameters),
                enabled: Set(true),
                is_read_only: Set(is_read_only),
                inherit_access_from_server: Set(true),
                last_synced_at: Set(Some(Utc::now())),
                created_at: Set(Utc::now()),
                updated_at: Set(Utc::now()),
            };
            if let Err(e) = model.insert(&state.database).await {
                eprintln!("mcp tool insert error: {e}");
            } else {
                tools_added += 1;
            }
        }
    }

    for (_, tool) in existing_by_name.into_iter() {
        if tool.enabled {
            tools_removed += 1;
        }
        let mut active: mcp_tools::ActiveModel = tool.into();
        active.enabled = Set(false);
        active.last_synced_at = Set(Some(Utc::now()));
        active.updated_at = Set(Utc::now());
        if let Err(e) = active.update(&state.database).await {
            eprintln!("mcp tool disable error: {e}");
        }
    }

    let total_tools = mcp_tools::Entity::find()
        .filter(mcp_tools::Column::ServerId.eq(server_id))
        .filter(mcp_tools::Column::Enabled.eq(true))
        .count(&state.database)
        .await
        .unwrap_or(0) as i32;

    let mut server_active: mcp_servers::ActiveModel = server.into();
    server_active.tool_count = Set(total_tools);
    server_active.last_synced_at = Set(Some(Utc::now()));
    let _ = server_active.update(&state.database).await;

    Ok(Json(McpSyncResult {
        success: true,
        tools_added: Some(tools_added),
        tools_updated: Some(tools_updated),
        tools_removed: Some(tools_removed),
        total_tools: Some(total_tools),
        synced_at: Some(Utc::now()),
        error: None,
    }))
}

#[utoipa::path(
    get,
    path = "/admin/mcp-servers/{server_id}/executions",
    tag = "admin",
    params(
        ("server_id" = Uuid, Path, description = "MCP server id"),
        ("limit" = Option<u64>, Query, description = "Page size (max 100, default 50)"),
        ("offset" = Option<u64>, Query, description = "Offset for pagination"),
        ("tool_name" = Option<String>, Query, description = "Filter by tool name"),
        ("is_error" = Option<bool>, Query, description = "Filter by error executions"),
        ("user_id" = Option<Uuid>, Query, description = "Filter by user id")
    ),
    responses(
        (status = 200, body = PaginatedMcpToolExecutions)
    )
)]
pub async fn list_mcp_server_executions(
    claims: Claims,
    State(state): State<SharedState>,
    Path(server_id): Path<Uuid>,
    Query(query): Query<ListExecutionsQuery>,
) -> Result<Json<PaginatedMcpToolExecutions>, AuthError> {
    let authz = AuthorizationService::new(&state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_MCP_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            Some(server_id),
        )
        .await?;
    let mut finder =
        mcp_executions::Entity::find().filter(mcp_executions::Column::ServerId.eq(server_id));
    if let Some(tool) = query.tool_name {
        finder = finder.filter(mcp_executions::Column::ToolName.eq(tool));
    }
    if let Some(is_error) = query.is_error {
        finder = finder.filter(mcp_executions::Column::IsError.eq(is_error));
    }
    if let Some(user_id) = query.user_id {
        finder = finder.filter(mcp_executions::Column::UserId.eq(user_id));
    }
    let limit = query.limit.unwrap_or(50).min(100);
    let offset = query.offset.unwrap_or(0);
    let paginator = finder
        .order_by_desc(mcp_executions::Column::ExecutedAt)
        .paginate(&state.database, limit);
    let total = paginator
        .num_items()
        .await
                    .map_err(|e|{
              eprintln!("Db get error {}",e);
              AuthError::DbTimeout
           })?;
    let data = paginator
        .fetch_page(offset / limit)
        .await
                    .map_err(|e|{
              eprintln!("Db get error {}",e);
              AuthError::DbTimeout
           })?;
    let executions = data.into_iter().map(to_execution_dto).collect();
    Ok(Json(PaginatedMcpToolExecutions {
        executions,
        total: total as i64,
        limit: limit as i64,
        offset: offset as i64,
    }))
}

pub async fn get_mcp_server_access(
    claims: Claims,
    State(state): State<SharedState>,
    Path(server_id): Path<Uuid>,
) -> Result<Json<McpServerAccessList>, AuthError> {
    let authz = AuthorizationService::new(&state.database);
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
        .one(&state.database)
        .await
        .map_err(|_| AuthError::DbUnavailable)?
        .ok_or(AuthError::ResourceNotFound)?;
    let rules = mcp_access_policies::Entity::find()
        .filter(mcp_access_policies::Column::TargetType.eq(McpAccessTarget::Server))
        .filter(mcp_access_policies::Column::ServerId.eq(server_id))
        .all(&state.database)
        .await
        .map_err(|e|{
            eprintln!("Db get error {}",e);
            AuthError::DbTimeout
        })?;
    Ok(Json(McpServerAccessList {
        server_id,
        server_name: server.name,
        default_access: server.default_access,
        rules: rules.into_iter().map(to_access_rule_dto).collect(),
    }))
}

pub async fn update_mcp_server_access(
    claims: Claims,
    State(state): State<SharedState>,
    Path(server_id): Path<Uuid>,
    Json(req): Json<McpServerAccessUpdate>,
) -> Result<Json<McpServerAccessList>, AuthError> {
    let authz = AuthorizationService::new(&state.database);
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
            .one(&state.database)
            .await
            .map_err(|e|{
              eprintln!("Db get error {}",e);
              AuthError::DbTimeout
           })?
        {
            let mut active: mcp_servers::ActiveModel = server.into();
            active.default_access = Set(default_access);
            let _ = active.update(&state.database)
              .await
              .map_err(|e|{
                 eprintln!("Db get error {}",e);
                AuthError::DbTimeout
            })?;
        }
    }

    let _ = mcp_access_policies::Entity::delete_many()
        .filter(mcp_access_policies::Column::TargetType.eq(McpAccessTarget::Server))
        .filter(mcp_access_policies::Column::ServerId.eq(server_id))
        .exec(&state.database)
        .await
        .map_err(|e|{
            eprintln!("Db get error {}",e);
            AuthError::DbTimeout
        })?;

    if let Some(rules) = req.rules {
        for r in rules {
            let model = mcp_access_policies::ActiveModel {
                id: Set(Uuid::new_v4()),
                target_type: Set(McpAccessTarget::Server),
                server_id: Set(Some(server_id)),
                tool_id: Set(None),
                access_type: Set(r.access_type),
                permission: Set(r.permission),
                role_name: Set(r.role_name),
                department_id: Set(r.department_id),
                user_id: Set(r.user_id),
                inherit_from_server: Set(None),
                created_at: Set(Utc::now()),
                created_by: Set(None),
            };
            model
                .insert(&state.database)
                .await
                .map_err(|e|{
                   eprintln!("Db get error {}",e);
                   AuthError::DbTimeout
        })?;
        }
    }

    get_mcp_server_access(claims, State(state), Path(server_id)).await
}

pub async fn get_mcp_server_tools_access(
    claims: Claims,
    State(state): State<SharedState>,
    Path(server_id): Path<Uuid>,
) -> Result<Json<Vec<McpToolAccessList>>, AuthError> {
    let authz = AuthorizationService::new(&state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_MCP_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            Some(server_id),
        )
        .await?;
    let tools = mcp_tools::Entity::find()
        .filter(mcp_tools::Column::ServerId.eq(server_id))
        .all(&state.database)
        .await
        .map_err(|e|{
            eprintln!("Db get error {}",e);
            AuthError::DbTimeout
        })?;
    let mut result = Vec::new();
    for tool in tools {
        let rules = mcp_access_policies::Entity::find()
            .filter(mcp_access_policies::Column::TargetType.eq(McpAccessTarget::Tool))
            .filter(mcp_access_policies::Column::ToolId.eq(tool.id))
            .all(&state.database)
            .await
            .map_err(|e|{
               eprintln!("Db get error {}",e);
               AuthError::DbTimeout
        })?;
        result.push(McpToolAccessList {
            tool_id: tool.id,
            tool_name: tool.name.clone(),
            server_id: tool.server_id,
            inherit_from_server: tool.inherit_access_from_server,
            rules: rules.into_iter().map(to_access_rule_dto).collect(),
        });
    }
    Ok(Json(result))
}

pub async fn update_mcp_server_tools_access(
    claims: Claims,
    State(state): State<SharedState>,
    Path(server_id): Path<Uuid>,
    Json(req): Json<BulkToolAccessUpdate>,
) -> Result<Json<BulkToolAccessUpdateResponse>, AuthError> {
    let authz = AuthorizationService::new(&state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_MCP_DELEGATE,
            None,
            PermissionScopeMode::RequireOrgWide,
            Some(server_id),
        )
        .await?;
    let mut updated = Vec::new();
    for item in req.tools {
        if let Some(tool) = mcp_tools::Entity::find_by_id(item.tool_id)
            .one(&state.database)
            .await
            .map_err(|_| AuthError::DbUnavailable)?
        {
            let inherit_flag = item
                .inherit_from_server
                .unwrap_or(tool.inherit_access_from_server);
            let mut active: mcp_tools::ActiveModel = tool.clone().into();
            active.inherit_access_from_server = Set(inherit_flag);
            let _ = active.update(&state.database).await;

            let _ = mcp_access_policies::Entity::delete_many()
                .filter(mcp_access_policies::Column::TargetType.eq(McpAccessTarget::Tool))
                .filter(mcp_access_policies::Column::ToolId.eq(item.tool_id))
                .exec(&state.database)
                .await
                .map_err(|e|{
                   eprintln!("Db get error {}",e);
                   AuthError::DbTimeout
           })?;
            if let Some(rules) = item.rules {
                for r in rules {
                    let model = mcp_access_policies::ActiveModel {
                        id: Set(Uuid::new_v4()),
                        target_type: Set(McpAccessTarget::Tool),
                        server_id: Set(Some(server_id)),
                        tool_id: Set(Some(item.tool_id)),
                        access_type: Set(r.access_type),
                        permission: Set(r.permission),
                        role_name: Set(r.role_name),
                        department_id: Set(r.department_id),
                        user_id: Set(r.user_id),
                        inherit_from_server: Set(None),
                        created_at: Set(Utc::now()),
                        created_by: Set(None),
                    };
                    model
                        .insert(&state.database)
                        .await
                        .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
                }
            }

            updated.push(McpToolAccessList {
                tool_id: tool.id,
                tool_name: tool.name,
                server_id: tool.server_id,
                inherit_from_server: inherit_flag,
                rules: mcp_access_policies::Entity::find()
                    .filter(mcp_access_policies::Column::TargetType.eq(McpAccessTarget::Tool))
                    .filter(mcp_access_policies::Column::ToolId.eq(item.tool_id))
                    .all(&state.database)
                    .await
                    .map_err(|_| AuthError::DbUnavailable)?
                    .into_iter()
                    .map(to_access_rule_dto)
                    .collect(),
            });
        }
    }
    Ok(Json(BulkToolAccessUpdateResponse {
        updated_count: updated.len(),
        tools: updated,
    }))
}

pub async fn get_mcp_tool_access(
    claims: Claims,
    State(state): State<SharedState>,
    Path(tool_id): Path<Uuid>,
) -> Result<Json<McpToolAccessList>, AuthError> {
    let authz = AuthorizationService::new(&state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_MCP_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            Some(tool_id),
        )
        .await?;
    let tool = mcp_tools::Entity::find_by_id(tool_id)
        .one(&state.database)
        .await
        .map_err(|_| AuthError::DbUnavailable)?
        .ok_or(AuthError::ResourceNotFound)?;
    let rules = mcp_access_policies::Entity::find()
        .filter(mcp_access_policies::Column::TargetType.eq(McpAccessTarget::Tool))
        .filter(mcp_access_policies::Column::ToolId.eq(tool_id))
        .all(&state.database)
        .await
        .map_err(|e|{
              eprintln!("Db get error {}",e);
              AuthError::DbTimeout
           })?;
    Ok(Json(McpToolAccessList {
        tool_id,
        tool_name: tool.name,
        server_id: tool.server_id,
        inherit_from_server: tool.inherit_access_from_server,
        rules: rules.into_iter().map(to_access_rule_dto).collect(),
    }))
}

pub async fn update_mcp_tool_access(
    claims: Claims,
    State(state): State<SharedState>,
    Path(tool_id): Path<Uuid>,
    Json(req): Json<McpToolAccessUpdate>,
) -> Result<Json<McpToolAccessList>, AuthError> {
    let authz = AuthorizationService::new(&state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_MCP_DELEGATE,
            None,
            PermissionScopeMode::RequireOrgWide,
            Some(tool_id),
        )
        .await?;
    if let Some(tool) = mcp_tools::Entity::find_by_id(tool_id)
        .one(&state.database)
        .await
        .map_err(|_| AuthError::DbUnavailable)?
    {
        let mut active: mcp_tools::ActiveModel = tool.clone().into();
        if let Some(inherit) = req.inherit_from_server {
            active.inherit_access_from_server = Set(inherit);
        }
        let _ = active.update(&state.database).await;
    }

    let _ = mcp_access_policies::Entity::delete_many()
        .filter(mcp_access_policies::Column::TargetType.eq(McpAccessTarget::Tool))
        .filter(mcp_access_policies::Column::ToolId.eq(tool_id))
        .exec(&state.database)
        .await
        .map_err(|e|{
              eprintln!("Db delete error {}",e);
              AuthError::DbTimeout
           })?;
    if let Some(rules) = req.rules {
        for r in rules {
            let model = mcp_access_policies::ActiveModel {
                id: Set(Uuid::new_v4()),
                target_type: Set(McpAccessTarget::Tool),
                server_id: Set(None),
                tool_id: Set(Some(tool_id)),
                access_type: Set(r.access_type),
                permission: Set(r.permission),
                role_name: Set(r.role_name),
                department_id: Set(r.department_id),
                user_id: Set(r.user_id),
                inherit_from_server: Set(None),
                created_at: Set(Utc::now()),
                created_by: Set(None),
            };
            model
                .insert(&state.database)
                .await
                .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
        }
    }
    get_mcp_tool_access(claims, State(state), Path(tool_id)).await
}

pub async fn list_mcp_tools(
    State(state): State<SharedState>,
    Query(query): Query<ListToolsQuery>,
) -> Result<Json<McpToolsList>, AppError> {
    let mut finder = mcp_tools::Entity::find().filter(mcp_tools::Column::Enabled.eq(true));
    if let Some(server) = query.server_id {
        finder = finder.filter(mcp_tools::Column::ServerId.eq(server));
    }
    if let Some(search) = query.search {
        let pattern = format!("%{}%", search);
        finder = finder.filter(
            Expr::col(mcp_tools::Column::Name)
                .like(pattern.clone())
                .or(Expr::col(mcp_tools::Column::Description).like(pattern)),
        );
    }
    let tools = finder
        .all(&state.database)
        .await
        .map_err(|e|{
              eprintln!("Db get all error {}",e);
              AppError::DbTimeout
           })?;
    let dto_tools: Vec<McpTool> = tools.iter().map(to_tool_dto).collect();
    Ok(Json(McpToolsList {
        tools: dto_tools,
        total: tools.len() as i64,
    }))
}

pub async fn list_mcp_connections(
    claims: Claims,
    State(state): State<SharedState>,
) -> Result<Json<McpUserConnectionsList>, AppError> {
    let rows = mcp_connections::Entity::find()
        .filter(mcp_connections::Column::UserId.eq(claims.user_id))
        .all(&state.database)
        .await
        .map_err(|e|{
              eprintln!("Db get all error {}",e);
              AppError::DbTimeout
           })?;
    let connections = rows
        .into_iter()
        .map(|c| McpUserConnection {
            server_id: c.server_id,
            server_name: c.server_name,
            description: None,
            connected: c.connected,
            connected_at: c.connected_at,
            expires_at: c.expires_at,
            scopes: c
                .scopes
                .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok()),
        })
        .collect();
    Ok(Json(McpUserConnectionsList { connections }))
}

pub async fn authorize_mcp_connection(
    claims: Claims,
    State(state): State<SharedState>,
    Path(server_id): Path<Uuid>,
    Query(query): Query<McpAuthorizeQuery>,
) -> Result<Json<McpAuthorizeResponse>, AppError> {
    let server = mcp_servers::Entity::find_by_id(server_id)
        .one(&state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp server lookup error: {e}");
            AppError::DbTimeout
        })?
        .ok_or(AppError::McpServerNotFound)?;

    if !server.enabled {
        return Err(AppError::McpServerNotFound);
    }

    if server.transport_type != mcp_servers::McpTransportType::Http {
        return Err(AppError::ServiceTemporarilyUnavailable);
    }

    let oauth_config = build_oauth_config(&state, &server)?;
    let authorization = build_authorization_url(&oauth_config).map_err(|e| {
        eprintln!("mcp oauth authorize url error: {e}");
        AppError::ServiceTemporarilyUnavailable
    })?;

    let now = Utc::now();
    let expires_at = now + Duration::minutes(10);
    let model = mcp_oauth_states::ActiveModel {
        id: Set(Uuid::new_v4()),
        server_id: Set(server_id),
        user_id: Set(claims.user_id),
        state: Set(authorization.state.clone()),
        pkce_verifier: Set(authorization.pkce_verifier.clone()),
        redirect_uri: Set(query.redirect_uri.clone()),
        expires_at: Set(Some(expires_at)),
        created_at: Set(now),
    };
    model
        .insert(&state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp oauth state insert error: {e}");
            AppError::DbTimeout
        })?;

    Ok(Json(McpAuthorizeResponse {
        success: true,
        authorization_url: Some(authorization.authorization_url),
        message: Some("Authorize via provided URL".into()),
    }))
}

pub async fn mcp_oauth_callback(
    State(state): State<SharedState>,
    Query(query): Query<McpOauthCallbackQuery>,
) -> Result<Response, AppError> {
    let oauth_state = mcp_oauth_states::Entity::find()
        .filter(mcp_oauth_states::Column::State.eq(query.state.clone()))
        .one(&state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp oauth state lookup error: {e}");
            AppError::DbTimeout
        })?
        .ok_or(AppError::ResourceNotFound)?;

    if let Some(expires_at) = oauth_state.expires_at {
        if expires_at <= Utc::now() {
            let _ = mcp_oauth_states::Entity::delete_by_id(oauth_state.id)
                .exec(&state.database)
                .await;
            return Err(AppError::ResourceNotFound);
        }
    }

    if let Some(error) = query.error.clone() {
        let response = build_oauth_callback_response(
            oauth_state.redirect_uri.as_deref(),
            oauth_state.server_id,
            false,
        );
        eprintln!("mcp oauth error: {error} {:?}", query.error_description);
        let _ = mcp_oauth_states::Entity::delete_by_id(oauth_state.id)
            .exec(&state.database)
            .await;
        return Ok(response);
    }

    let code = query
        .code
        .ok_or(AppError::ValidationMissingField { field: "code" })?;

    let server = mcp_servers::Entity::find_by_id(oauth_state.server_id)
        .one(&state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp server lookup error: {e}");
            AppError::DbTimeout
        })?
        .ok_or(AppError::McpServerNotFound)?;

    let oauth_config = build_oauth_config(&state, &server)?;
    let tokens = exchange_code(
        &oauth_config,
        &code,
        &oauth_state.pkce_verifier,
        &state.req_client,
    )
    .await
    .map_err(|e| {
        eprintln!("mcp oauth token exchange error: {e}");
        AppError::ServiceTemporarilyUnavailable
    })?;

    store_oauth_tokens(&state, oauth_state.user_id, server.id, &server.name, tokens).await?;

    let _ = mcp_oauth_states::Entity::delete_by_id(oauth_state.id)
        .exec(&state.database)
        .await;

    Ok(build_oauth_callback_response(
        oauth_state.redirect_uri.as_deref(),
        oauth_state.server_id,
        true,
    ))
}

fn build_oauth_callback_response(
    redirect_uri: Option<&str>,
    server_id: Uuid,
    success: bool,
) -> Response {
    if let Some(redirect_uri) = redirect_uri {
        let separator = if redirect_uri.contains('?') { "&" } else { "?" };
        let status_value = if success { "success" } else { "error" };
        let url = format!(
            "{redirect_uri}{separator}mcp_server_id={server_id}&status={status_value}"
        );
        return Redirect::to(&url).into_response();
    }

    let body = json!({
        "success": success,
        "server_id": server_id,
        "status": if success { "success" } else { "error" }
    });
    (StatusCode::OK, Json(body)).into_response()
}

pub async fn disconnect_mcp_connection(
    claims: Claims,
    State(state): State<SharedState>,
    Path(server_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    upsert_connection(&state, claims.user_id, server_id, false).await?;
    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Disconnected"
    })))
}
