use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, Set,
};
use serde_json::json;
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    error::AppError,
    auth::{claims::Claims, encryption::encrypt_key},
    dto::mcp::{
        BulkToolAccessUpdate, BulkToolAccessUpdateResponse, ListExecutionsQuery, ListServersQuery,
        ListToolsQuery, McpAuthorizeResponse, McpServer, McpServerAccessList,
        McpServerAccessUpdate, McpServerCreate, McpServerTestResult, McpServerUpdate, McpSyncResult,
        McpTool, McpToolAccessList, McpToolAccessUpdate, McpToolsList,
        McpUserConnection, McpUserConnectionsList, PaginatedMcpServers, PaginatedMcpToolExecutions,
    },
    models::{
        mcp_access_policies,
        mcp_access_policies::McpAccessTarget,
        mcp_connections, mcp_executions, mcp_servers,
        mcp_servers::McpDefaultAccess,
        mcp_tools,
    },
    llm::tooling::sanitize_tool_name,
    services::mcp_helpers::{
        encrypt_db_url_in_config,
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
    State(state): State<SharedState>,
    Query(query): Query<ListServersQuery>,
) -> Result<Json<PaginatedMcpServers>, AppError> {
    let mut finder = mcp_servers::Entity::find();
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
            AppError::DbTimeout
        })?;
    let dtos: Vec<_> = servers.into_iter().map(to_server_dto).collect();
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
    State(state): State<SharedState>,
    Json(req): Json<McpServerCreate>,
) -> Result<(StatusCode, Json<McpServer>), AppError> {
    let now = Utc::now();
    let mut connection_config = req.connection_config;
    encrypt_db_url_in_config(&state.settings.auth.app_key, &mut connection_config)?;
    let client_secret = match req.client_secret {
        Some(ref secret) if !secret.is_empty() => Some(
            encrypt_key(&state.settings.auth.app_key, secret.as_bytes())
                .map_err(|_| AppError::ServiceTemporarilyUnavailable)?,
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
        .map_err(|e|{
            eprintln!("Db create error {}",e);
            AppError::DbTimeout
        })?;
    state.upsert_mcp_client(&saved).await;
    Ok((StatusCode::CREATED, Json(to_server_dto(saved))))
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
    State(state): State<SharedState>,
    Path(server_id): Path<Uuid>,
) -> Result<Json<McpServer>, AppError> {
    let server = mcp_servers::Entity::find_by_id(server_id)
        .one(&state.database)
        .await
        .map_err(|e|{
            eprintln!("Db get one error {}",e);
            AppError::DbTimeout
         })?
        .ok_or(AppError::McpServerNotFound)?;
    Ok(Json(to_server_dto(server)))
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
    State(state): State<SharedState>,
    Path(server_id): Path<Uuid>,
    Json(req): Json<McpServerUpdate>,
) -> Result<Json<McpServer>, AppError> {
    let server = mcp_servers::Entity::find_by_id(server_id)
        .one(&state.database)
        .await
        .map_err(|_| AppError::DbUnavailable)?
        .ok_or(AppError::McpServerNotFound)?;
    let mut active: mcp_servers::ActiveModel = server.into();
    if let Some(name) = req.name {
        active.name = Set(name);
    }
    if let Some(desc) = req.description {
        active.description = Set(Some(desc));
    }
    if let Some(cfg) = req.connection_config {
        let mut cfg = cfg;
        encrypt_db_url_in_config(&state.settings.auth.app_key, &mut cfg)?;
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
                    .map_err(|_| AppError::ServiceTemporarilyUnavailable)?,
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
        .map_err(|e|{
            eprintln!("Db get error {}",e);
            AppError::DbTimeout
        })?;
    state.upsert_mcp_client(&saved).await;
    Ok(Json(to_server_dto(saved)))
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
    State(state): State<SharedState>,
    Path(server_id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    mcp_servers::Entity::delete_by_id(server_id)
        .exec(&state.database)
        .await
        .map_err(|e|{
            eprintln!("Db get error {}",e);
            AppError::DbTimeout
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
    Path(_server_id): Path<Uuid>,
) -> Result<Json<McpServerTestResult>, AppError> {
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
    State(state): State<SharedState>,
    Path(server_id): Path<Uuid>,
) -> Result<Json<McpSyncResult>, AppError> {
    let server = mcp_servers::Entity::find_by_id(server_id)
        .one(&state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp server lookup error: {e}");
            AppError::DbTimeout
        })?
        .ok_or(AppError::McpServerNotFound)?;

    let client = {
        let clients = state.mcp_clients.read().await;
        clients.get(&server_id).cloned()
    }
    .ok_or(AppError::McpServerNotFound)?;

    let tools = client.list_tools().await.map_err(|e| {
        eprintln!("mcp list tools error: {e}");
        AppError::ServiceTemporarilyUnavailable
    })?;

    let existing_tools = mcp_tools::Entity::find()
        .filter(mcp_tools::Column::ServerId.eq(server_id))
        .all(&state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp tools lookup error: {e}");
            AppError::DbTimeout
        })?;

    let mut existing_by_name: HashMap<String, mcp_tools::Model> = existing_tools
        .into_iter()
        .map(|tool| (tool.original_name.clone(), tool))
        .collect();

    let mut tools_added = 0;
    let mut tools_updated = 0;
    let mut tools_removed = 0;

    for tool in tools {
        let original_name = tool.name.clone();
        let sanitized_name = sanitize_tool_name(&tool.name);
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
            active.description = Set(tool.description.clone());
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
                description: Set(tool.description.clone()),
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
    State(state): State<SharedState>,
    Path(server_id): Path<Uuid>,
    Query(query): Query<ListExecutionsQuery>,
) -> Result<Json<PaginatedMcpToolExecutions>, AppError> {
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
              AppError::DbTimeout
           })?;
    let data = paginator
        .fetch_page(offset / limit)
        .await
                    .map_err(|e|{
              eprintln!("Db get error {}",e);
              AppError::DbTimeout
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
    State(state): State<SharedState>,
    Path(server_id): Path<Uuid>,
) -> Result<Json<McpServerAccessList>, AppError> {
    let server = mcp_servers::Entity::find_by_id(server_id)
        .one(&state.database)
        .await
        .map_err(|_| AppError::DbUnavailable)?
        .ok_or(AppError::McpServerNotFound)?;
    let rules = mcp_access_policies::Entity::find()
        .filter(mcp_access_policies::Column::TargetType.eq(McpAccessTarget::Server))
        .filter(mcp_access_policies::Column::ServerId.eq(server_id))
        .all(&state.database)
        .await
        .map_err(|e|{
            eprintln!("Db get error {}",e);
            AppError::DbTimeout
        })?;
    Ok(Json(McpServerAccessList {
        server_id,
        server_name: server.name,
        default_access: server.default_access,
        rules: rules.into_iter().map(to_access_rule_dto).collect(),
    }))
}

pub async fn update_mcp_server_access(
    State(state): State<SharedState>,
    Path(server_id): Path<Uuid>,
    Json(req): Json<McpServerAccessUpdate>,
) -> Result<Json<McpServerAccessList>, AppError> {
    if let Some(default_access) = req.default_access {
        if let Some(server) = mcp_servers::Entity::find_by_id(server_id)
            .one(&state.database)
            .await
            .map_err(|e|{
              eprintln!("Db get error {}",e);
              AppError::DbTimeout
           })?
        {
            let mut active: mcp_servers::ActiveModel = server.into();
            active.default_access = Set(default_access);
            let _ = active.update(&state.database)
              .await
              .map_err(|e|{
                 eprintln!("Db get error {}",e);
                AppError::DbTimeout
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
            AppError::DbTimeout
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
                   AppError::DbTimeout
        })?;
        }
    }

    get_mcp_server_access(State(state), Path(server_id)).await
}

pub async fn get_mcp_server_tools_access(
    State(state): State<SharedState>,
    Path(server_id): Path<Uuid>,
) -> Result<Json<Vec<McpToolAccessList>>, AppError> {
    let tools = mcp_tools::Entity::find()
        .filter(mcp_tools::Column::ServerId.eq(server_id))
        .all(&state.database)
        .await
        .map_err(|e|{
            eprintln!("Db get error {}",e);
            AppError::DbTimeout
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
               AppError::DbTimeout
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
    State(state): State<SharedState>,
    Path(server_id): Path<Uuid>,
    Json(req): Json<BulkToolAccessUpdate>,
) -> Result<Json<BulkToolAccessUpdateResponse>, AppError> {
    let mut updated = Vec::new();
    for item in req.tools {
        if let Some(tool) = mcp_tools::Entity::find_by_id(item.tool_id)
            .one(&state.database)
            .await
            .map_err(|_| AppError::DbUnavailable)?
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
                   AppError::DbTimeout
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
                        .map_err(|_| AppError::McpAccessUpdateFailed)?;
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
                    .map_err(|_| AppError::DbUnavailable)?
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
    State(state): State<SharedState>,
    Path(tool_id): Path<Uuid>,
) -> Result<Json<McpToolAccessList>, AppError> {
    let tool = mcp_tools::Entity::find_by_id(tool_id)
        .one(&state.database)
        .await
        .map_err(|_| AppError::DbUnavailable)?
        .ok_or(AppError::ResourceNotFound)?;
    let rules = mcp_access_policies::Entity::find()
        .filter(mcp_access_policies::Column::TargetType.eq(McpAccessTarget::Tool))
        .filter(mcp_access_policies::Column::ToolId.eq(tool_id))
        .all(&state.database)
        .await
        .map_err(|e|{
              eprintln!("Db get error {}",e);
              AppError::DbTimeout
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
    State(state): State<SharedState>,
    Path(tool_id): Path<Uuid>,
    Json(req): Json<McpToolAccessUpdate>,
) -> Result<Json<McpToolAccessList>, AppError> {
    if let Some(tool) = mcp_tools::Entity::find_by_id(tool_id)
        .one(&state.database)
        .await
        .map_err(|_| AppError::DbUnavailable)?
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
              AppError::DbTimeout
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
                .map_err(|_| AppError::McpAccessUpdateFailed)?;
        }
    }
    get_mcp_tool_access(State(state), Path(tool_id)).await
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
) -> Result<Json<McpAuthorizeResponse>, AppError> {
    upsert_connection(&state, claims.user_id, server_id, true).await?;
    Ok(Json(McpAuthorizeResponse {
        success: true,
        authorization_url: Some(format!("https://auth.example.com/mcp/{server_id}")),
        message: Some("Authorize via provided URL".into()),
    }))
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
