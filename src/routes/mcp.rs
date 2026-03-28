use axum::{Router, routing::get, routing::post};

use crate::{
    handlers::mcp::{
        authorize_mcp_connection, create_mcp_server, delete_mcp_server, disconnect_mcp_connection,
        get_mcp_server, get_mcp_server_tools_access, get_mcp_tool_access, list_mcp_connections,
        list_mcp_server_executions, list_mcp_servers, list_mcp_tools, list_public_mcp_servers,
        get_mcp_effective_access,
        mcp_oauth_callback, sync_mcp_server_tools, test_mcp_server, update_mcp_server,
        update_mcp_server_tools_access, update_mcp_tool_access,
    },
    state::SharedState,
};

pub fn mcp_routes() -> Router<SharedState> {
    Router::new()
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
            "/mcp/connections/{server_id}/authorize",
            post(authorize_mcp_connection),
        )
        .route(
            "/mcp/connections/{server_id}/disconnect",
            post(disconnect_mcp_connection),
        )
        .route("/mcp/oauth/callback", get(mcp_oauth_callback))
}
