// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use axum::{Router, routing::get, routing::post};

use crate::{
    handlers::mcp::{
        authorize_mcp_connection, create_mcp_server, delete_mcp_server, disconnect_mcp_connection,
        get_mcp_effective_access, get_mcp_server, get_mcp_server_tools_access, get_mcp_tool_access,
        list_mcp_connections, list_mcp_server_executions, list_mcp_servers, list_mcp_tools,
        list_public_mcp_servers, mcp_oauth_callback, save_mcp_credential, sync_mcp_server_tools,
        test_mcp_server, update_mcp_server, update_mcp_server_tools_access, update_mcp_tool_access,
    },
    state::SharedState,
};

pub fn mcp_routes() -> Router<SharedState> {
    Router::new()
        .route(
            "/admin/mcp-servers/{server_id}/overview",
            get(crate::handlers::connector_console::overview),
        )
        .route(
            "/admin/mcp-servers/{server_id}/people",
            get(crate::handlers::connector_console::people),
        )
        .route(
            "/admin/mcp-servers/{server_id}/tools/{tool_id}",
            axum::routing::put(crate::handlers::connector_console::update_tool),
        )
        .route(
            "/admin/mcp-servers",
            get(list_mcp_servers).post(create_mcp_server),
        )
        .route(
            "/admin/mcp-servers/{server_id}",
            get(get_mcp_server)
                .put(update_mcp_server)
                .delete(delete_mcp_server),
        )
        .route("/admin/mcp-servers/{server_id}/test", post(test_mcp_server))
        .route(
            "/admin/mcp-servers/{server_id}/sync-tools",
            post(sync_mcp_server_tools),
        )
        .route(
            "/admin/mcp-servers/{server_id}/executions",
            get(list_mcp_server_executions),
        )
        .route(
            "/admin/mcp-servers/{server_id}/tools/access",
            get(get_mcp_server_tools_access).put(update_mcp_server_tools_access),
        )
        .route(
            "/admin/mcp-tools/{tool_id}/access",
            get(get_mcp_tool_access).put(update_mcp_tool_access),
        )
        .route("/mcp-servers", get(list_public_mcp_servers))
        .route("/mcp/effective-access", get(get_mcp_effective_access))
        .route("/mcp/tools", get(list_mcp_tools))
        .route("/mcp/connections", get(list_mcp_connections))
        .route(
            "/mcp/connections/{server_id}/credentials",
            post(save_mcp_credential),
        )
        .route(
            "/mcp/connections/{server_id}/authorize",
            post(authorize_mcp_connection),
        )
        .route(
            "/mcp/connections/{server_id}/disconnect",
            post(disconnect_mcp_connection),
        )
        .route("/mcp/oauth/callback", get(mcp_oauth_callback))
        .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("private, no-store"),
        ))
}
