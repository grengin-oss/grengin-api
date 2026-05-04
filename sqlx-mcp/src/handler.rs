use crate::db_manager::DatabaseManager;
use crate::read_only::is_read_only_sql;
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::*,
    schemars,
    service::{RequestContext, RoleServer},
    tool, tool_handler, tool_router,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct SqlxDatabaseHandler {
    db_manager: Arc<DatabaseManager>,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SqlQueryParams {
    /// Read-only PostgreSQL SQL query statement to execute
    sql: String,
    /// SQL parameter array, optional
    #[serde(default)]
    params: Vec<serde_json::Value>,
}

#[tool_router]
impl SqlxDatabaseHandler {
    pub fn new(db_manager: Arc<DatabaseManager>) -> Self {
        Self {
            db_manager,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Execute a single read-only PostgreSQL SQL query and return results as JSON rows. Use PostgreSQL syntax (for table listing, query information_schema.tables).",
        annotations(read_only_hint = true)
    )]
    async fn sql_query(
        &self,
        _context: RequestContext<RoleServer>,
        Parameters(params): Parameters<SqlQueryParams>,
    ) -> Result<CallToolResult, McpError> {
        if !is_read_only_sql(&params.sql) {
            return Err(McpError::invalid_params(
                "sql_query only accepts single read-only SQL statements".to_string(),
                None,
            ));
        }

        match self
            .db_manager
            .execute_query(&params.sql, params.params)
            .await
        {
            Ok(results) => {
                let content = Content::json(results).map_err(|e| {
                    McpError::internal_error(format!("Result serialization failed: {e}"), None)
                })?;
                Ok(CallToolResult::success(vec![content]))
            }
            Err(err) => Err(McpError::internal_error(
                format!("SQL query failed: {err:#}"),
                None,
            )),
        }
    }

    #[tool(
        description = "Get SQLx MCP database status, including database_type and read_only mode.",
        annotations(read_only_hint = true)
    )]
    async fn db_status(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let status = self.db_manager.get_pool_state().await;
        let content = Content::json(status).map_err(|e| {
            McpError::internal_error(format!("Status serialization failed: {e}"), None)
        })?;
        Ok(CallToolResult::success(vec![content]))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SqlxDatabaseHandler {
    fn get_info(&self) -> ServerInfo {
        let mut server_info = Implementation::from_build_env();
        server_info.name = "sqlx-mcp".to_string();
        server_info.title = Some("SQLx MCP Server".to_string());
        server_info.version = env!("CARGO_PKG_VERSION").to_string();

        ServerInfo {
            instructions: Some("SQLx PostgreSQL MCP server. Exposes read-only tools only. Use PostgreSQL syntax (not SQLite/MySQL metadata commands). For listing tables, query information_schema.tables.".to_string()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info,
            ..Default::default()
        }
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        Ok(self.get_info())
    }
}
