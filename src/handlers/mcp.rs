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
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    error::AppError,
    auth::{claims::Claims, encryption::{decrypt_key, encrypt_key}},
    dto::mcp::{
        BulkToolAccessUpdate, BulkToolAccessUpdateResponse, McpAccessRule,
        McpAuthorizeResponse, McpServer, McpServerAccessList, McpServerAccessUpdate,
        McpServerCreate, McpServerTestResult, McpServerUpdate, McpSyncResult, McpTool,
        McpToolAccessList, McpToolAccessUpdate, McpToolExecution, McpToolsList, McpUserConnection,
        McpUserConnectionsList, PaginatedMcpServers, PaginatedMcpToolExecutions,
    },
    models::{
        mcp_access_policies,
        mcp_access_policies::McpAccessTarget,
        mcp_connections, mcp_executions, mcp_servers,
        mcp_servers::McpDefaultAccess,
        mcp_tools,
    },
    state::SharedState,
};

fn encrypt_db_url_in_config(
    app_key: &[u8; 32],
    connection_config: &mut serde_json::Value,
) -> Result<(), AppError> {
    let Some(obj) = connection_config.as_object_mut() else {
        return Ok(());
    };
    let Some(db_url_value) = obj.get("db_url").cloned() else {
        return Ok(());
    };
    let Some(db_url) = db_url_value.as_str() else {
        return Ok(());
    };
    if decrypt_key(app_key, db_url).is_ok() {
        return Ok(());
    }
    let encrypted = encrypt_key(app_key, db_url.as_bytes())
        .map_err(|_| AppError::ServiceTemporarilyUnavailable)?;
    obj.insert("db_url".to_string(), serde_json::Value::String(encrypted));
    Ok(())
}

#[derive(Deserialize)]
pub struct ListServersQuery {
    pub status: Option<String>,
    pub enabled: Option<bool>,
}

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
    Path(_server_id): Path<Uuid>,
) -> Result<Json<McpSyncResult>, AppError> {
    Ok(Json(McpSyncResult {
        success: true,
        tools_added: None,
        tools_updated: None,
        tools_removed: None,
        total_tools: None,
        synced_at: Some(Utc::now()),
        error: None,
    }))
}

#[derive(Deserialize)]
pub struct ListExecutionsQuery {
    pub limit: Option<u64>,
    pub offset: Option<u64>,
    pub tool_name: Option<String>,
    pub is_error: Option<bool>,
    pub user_id: Option<Uuid>,
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

#[derive(Deserialize)]
pub struct ListToolsQuery {
    pub server_id: Option<Uuid>,
    pub search: Option<String>,
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

// helpers

fn to_server_dto(model: mcp_servers::Model) -> McpServer {
    McpServer {
        id: model.id,
        name: model.name,
        description: model.description,
        transport_type: model.transport_type,
        connection_config: model.connection_config,
        client_id: model.client_id,
        client_secret_configured: model.client_secret.is_some(),
        url: model.url,
        enabled: model.enabled,
        status: model.status,
        status_message: model.status_message,
        tool_count: model.tool_count,
        default_access: model.default_access,
        last_connected_at: model.last_connected_at,
        last_synced_at: model.last_synced_at,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

fn to_tool_dto(model: &mcp_tools::Model) -> McpTool {
    McpTool {
        id: model.id,
        server_id: model.server_id,
        server_name: model.server_name.clone(),
        name: model.name.clone(),
        original_name: model.original_name.clone(),
        description: model.description.clone(),
        input_schema: model.input_schema.clone(),
        parameters: model.parameters.clone(),
        enabled: model.enabled,
        is_read_only: model.is_read_only,
        inherit_access_from_server: model.inherit_access_from_server,
        last_synced_at: model.last_synced_at,
    }
}

fn to_execution_dto(model: mcp_executions::Model) -> McpToolExecution {
    McpToolExecution {
        id: model.id,
        server_id: model.server_id,
        server_name: model.server_name,
        tool_name: model.tool_name,
        conversation_id: model.conversation_id,
        user_id: model.user_id,
        user_email: model.user_email,
        arguments: model.arguments,
        result: model.result,
        is_error: model.is_error,
        duration_ms: model.duration_ms,
        executed_at: model.executed_at,
    }
}

fn to_access_rule_dto(model: mcp_access_policies::Model) -> McpAccessRule {
    McpAccessRule {
        id: Some(model.id),
        access_type: model.access_type,
        role_name: model.role_name,
        department_id: model.department_id,
        user_id: model.user_id,
        permission: model.permission,
    }
}

async fn upsert_connection(
    state: &SharedState,
    user_id: Uuid,
    server_id: Uuid,
    connected: bool,
) -> Result<(), AppError> {
    if let Some(existing) = mcp_connections::Entity::find()
        .filter(mcp_connections::Column::UserId.eq(user_id))
        .filter(mcp_connections::Column::ServerId.eq(server_id))
        .one(&state.database)
        .await
        .map_err(|_| AppError::DbUnavailable)?
    {
        let mut active: mcp_connections::ActiveModel = existing.into();
        active.connected = Set(connected);
        active.updated_at = Set(Utc::now());
        active
            .update(&state.database)
            .await
            .map_err(|e|{
              eprintln!("Db update error {}",e);
              AppError::DbTimeout
           })?;
    } else {
        let model = mcp_connections::ActiveModel {
            id: Set(Uuid::new_v4()),
            user_id: Set(user_id),
            server_id: Set(server_id),
            server_name: Set("".into()),
            connected: Set(connected),
            connected_at: Set(if connected { Some(Utc::now()) } else { None }),
            expires_at: Set(None),
            scopes: Set(None),
            access_token: Set(None),
            refresh_token: Set(None),
            token_type: Set(None),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
        };
        model
            .insert(&state.database)
            .await
            .map_err(|e|{
              eprintln!("Db create error {}",e);
              AppError::DbTimeout
           })?;
    }
    Ok(())
}
