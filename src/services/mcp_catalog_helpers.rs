// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;

use chrono::Utc;
use sea_orm::{ColumnTrait, DbErr, EntityTrait, QueryFilter, QueryOrder};
use uuid::Uuid;

use crate::{
    dto::mcp::{
        ListPublicMcpServersQuery, McpServerCatalogEntry, McpServerCatalogResponse, McpToolSummary,
    },
    error::AppError,
    models::{mcp_access_policies::McpPermission, mcp_connections, mcp_servers, mcp_tools},
    services::{
        connector_console_helpers::{catalog_connection_state, integration_metadata},
        connectors_helpers::cached_mcp_statuses,
        mcp_tools::resolve_listing_access,
    },
    state::SharedState,
};

fn database_error(error: DbErr) -> AppError {
    eprintln!("mcp catalog database error: {error}");
    AppError::DbTimeout
}

pub async fn list_catalog(
    state: &SharedState,
    user_id: Uuid,
    query: ListPublicMcpServersQuery,
) -> Result<McpServerCatalogResponse, AppError> {
    let mut finder = mcp_servers::Entity::find().filter(mcp_servers::Column::Enabled.eq(true));
    if let Some(transport_type) = query.transport_type {
        finder = finder.filter(mcp_servers::Column::TransportType.eq(transport_type));
    }
    let servers = finder
        .order_by_asc(mcp_servers::Column::Name)
        .order_by_asc(mcp_servers::Column::Id)
        .all(&state.database)
        .await
        .map_err(database_error)?;
    if servers.is_empty() {
        return Ok(McpServerCatalogResponse {
            servers: Vec::new(),
        });
    }
    let server_ids: Vec<_> = servers.iter().map(|server| server.id).collect();
    let tools = mcp_tools::Entity::find()
        .filter(mcp_tools::Column::ServerId.is_in(server_ids.clone()))
        .filter(mcp_tools::Column::Enabled.eq(true))
        .all(&state.database)
        .await
        .map_err(database_error)?;
    let access = resolve_listing_access(&state.database, user_id, &servers, tools).await?;
    let mut tools_by_server: HashMap<Uuid, Vec<McpToolSummary>> = HashMap::new();
    for tool in access.tools {
        tools_by_server
            .entry(tool.server_id)
            .or_default()
            .push(McpToolSummary {
                name: tool.name,
                description: tool.description,
            });
    }
    let mut allowed_servers = Vec::new();
    for server in servers {
        // Explicit per-tool grants remain usable even when the server default denies access.
        if access
            .server_access
            .get(&server.id)
            .is_some_and(|resolved| resolved.permission != McpPermission::Denied)
            || tools_by_server.contains_key(&server.id)
        {
            allowed_servers.push(server);
        }
    }
    if allowed_servers.is_empty() {
        return Ok(McpServerCatalogResponse {
            servers: Vec::new(),
        });
    }
    let connections = mcp_connections::Entity::find()
        .filter(mcp_connections::Column::UserId.eq(user_id))
        .filter(mcp_connections::Column::ServerId.is_in(allowed_servers.iter().map(|s| s.id)))
        .all(&state.database)
        .await
        .map_err(database_error)?;
    let mut connections_by_server: HashMap<_, _> =
        connections.into_iter().map(|c| (c.server_id, c)).collect();
    let broker_statuses = cached_mcp_statuses(state, user_id).await?;
    let now = Utc::now();
    let mut response = Vec::with_capacity(allowed_servers.len());
    for server in allowed_servers {
        let integration = integration_metadata(&server);
        let connection_state = catalog_connection_state(
            &server,
            &integration,
            connections_by_server.remove(&server.id).as_ref(),
            broker_statuses.get(&server.id).copied(),
            now,
        );
        let connected =
            connection_state == crate::dto::connector_console::ConnectorConnectionState::Connected;
        if query.connected.is_some_and(|filter| filter != connected) {
            continue;
        }
        let mut tools = tools_by_server.remove(&server.id).unwrap_or_default();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        response.push(McpServerCatalogEntry {
            id: server.id,
            name: server.name,
            description: server.description,
            transport_type: server.transport_type,
            icon: server.icon,
            tools,
            connected,
            integration,
            connection_state,
        });
    }
    Ok(McpServerCatalogResponse { servers: response })
}
