// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use axum::{
    Json,
    extract::{Path, Query, State},
};
use uuid::Uuid;

use crate::{
    auth::{claims::Claims, error::AuthError},
    dto::{connector_console::*, mcp::McpTool},
    services::connector_console_helpers::ConnectorConsoleService,
    state::SharedState,
};

#[utoipa::path(get, path = "/admin/mcp-servers/{server_id}/overview", tag = "admin",
    description = "Connector detail for the administration UI. Requires organization-wide mcp_servers:view. Includes safe authentication metadata, your stored connection state, active-account counts, tools, access policies and the last seven days of tool execution counts. It never returns account tokens. Counts are local stored state, not a live vendor health check; active_user_count includes all active accounts, not only eligible accounts. Write confirmation and reminders are not yet supported.",
    params(("server_id" = Uuid, Path, description = "Installed server UUID")),
    responses((status = 200, body = ConnectorConsole), (status = 401, description = "Sign-in required"), (status = 403, description = "Organization-wide MCP viewing permission required"), (status = 404, description = "Unknown server"), (status = 503, description = "Database unavailable")))]
pub async fn overview(
    claims: Claims,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ConnectorConsole>, AuthError> {
    let result = ConnectorConsoleService::new(&state)
        .overview(claims.user_id, id)
        .await?;
    Ok(Json(result))
}

#[utoipa::path(get, path = "/admin/mcp-servers/{server_id}/people", tag = "admin",
    description = "Paginated active workspace accounts and their stored connector status. Requires organization-wide mcp_servers:view. Email is admin-only; no credential or vendor response is returned. An expired access token with a refresh token remains connected until refresh fails. The endpoint does not call vendors or change sessions.",
    params(("server_id" = Uuid, Path, description = "Installed server UUID"), ConnectorPeopleQuery),
    responses((status = 200, body = ConnectorPeople), (status = 400, description = "limit must be 1-100; offset at most 1000000"), (status = 401, description = "Sign-in required"), (status = 403, description = "Organization-wide MCP viewing permission required"), (status = 404, description = "Unknown server")))]
pub async fn people(
    claims: Claims,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Query(query): Query<ConnectorPeopleQuery>,
) -> Result<Json<ConnectorPeople>, AuthError> {
    let result = ConnectorConsoleService::new(&state)
        .people(claims.user_id, id, query)
        .await?;
    Ok(Json(result))
}

#[utoipa::path(put, path = "/admin/mcp-servers/{server_id}/tools/{tool_id}", tag = "admin",
    description = "Enable or disable an installed tool without overwriting its access rules or credentials. Requires organization-wide mcp_servers:admin. The change and audit event are committed atomically. Enabling a tool does not override read-only permissions, and does not add missing write tools or provide action confirmation.",
    params(("server_id" = Uuid, Path, description = "Installed server UUID"), ("tool_id" = Uuid, Path, description = "Tool UUID belonging to that server")),
    request_body = ConnectorToolUpdate,
    responses((status = 200, body = McpTool), (status = 401, description = "Sign-in required"), (status = 403, description = "Organization-wide MCP administration permission required"), (status = 404, description = "Server or tool not found"), (status = 503, description = "Change or audit could not be saved")))]
pub async fn update_tool(
    claims: Claims,
    State(state): State<SharedState>,
    Path((server_id, tool_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<ConnectorToolUpdate>,
) -> Result<Json<McpTool>, AuthError> {
    Ok(Json(
        ConnectorConsoleService::new(&state)
            .update_tool(claims.user_id, server_id, tool_id, request)
            .await?,
    ))
}
