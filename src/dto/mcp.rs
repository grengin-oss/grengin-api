use chrono::{DateTime, Utc};
use sea_orm::JsonValue;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::models::mcp_access_policies::{McpAccessType, McpPermission};
use crate::models::mcp_servers::{McpDefaultAccess, McpTransportType};

#[derive(Debug, Deserialize)]
pub struct ListServersQuery {
    pub status: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ListExecutionsQuery {
    pub limit: Option<u64>,
    pub offset: Option<u64>,
    pub tool_name: Option<String>,
    pub is_error: Option<bool>,
    pub user_id: Option<Uuid>,
}

#[derive(Deserialize)]
pub struct ListToolsQuery {
    pub server_id: Option<Uuid>,
    pub search: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct McpAuthorizeQuery {
    pub redirect_uri: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct McpOauthCallbackQuery {
    pub code: Option<String>,
    pub state: String,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct McpServer {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub transport_type: McpTransportType,
    #[schema(value_type = Object)]
    pub connection_config: JsonValue,
    pub client_id: Option<String>,
    pub client_secret_configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "client_secret_preview")]
    #[schema(value_type = String, rename = "client_secret_preview")]
    pub client_secret_preview: Option<String>,
    pub url: Option<String>,
    pub enabled: bool,
    pub status: Option<String>,
    pub status_message: Option<String>,
    pub tool_count: i32,
    pub default_access: McpDefaultAccess,
    pub last_connected_at: Option<DateTime<Utc>>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct McpServerCreate {
    pub name: String,
    pub description: Option<String>,
    pub transport_type: McpTransportType,
    #[schema(value_type = Object)]
    pub connection_config: JsonValue,
    pub client_id: Option<String>,
    /// Plaintext secret; encrypted before persisting.
    pub client_secret: Option<String>,
    pub url: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    pub default_access: Option<McpDefaultAccess>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct McpServerUpdate {
    pub name: Option<String>,
    pub transport_type: Option<McpTransportType>,
    pub description: Option<String>,
    #[schema(value_type = Object)]
    pub connection_config: Option<JsonValue>,
    pub client_id: Option<String>,
    /// Plaintext secret; encrypted before persisting.
    pub client_secret: Option<String>,
    pub url: Option<String>,
    pub enabled: Option<bool>,
    pub default_access: Option<McpDefaultAccess>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PaginatedMcpServers {
    pub servers: Vec<McpServer>,
    pub total: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct McpServerTestResult {
    pub success: bool,
    pub message: Option<String>,
    pub latency_ms: Option<i32>,
    pub available_tools: Option<i32>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct McpSyncResult {
    pub success: bool,
    pub tools_added: Option<i32>,
    pub tools_updated: Option<i32>,
    pub tools_removed: Option<i32>,
    pub total_tools: Option<i32>,
    pub synced_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct McpTool {
    pub id: Uuid,
    pub server_id: Uuid,
    pub server_name: String,
    pub name: String,
    pub original_name: String,
    pub description: Option<String>,
    #[schema(value_type = Object)]
    pub input_schema: JsonValue,
    #[schema(value_type = Object)]
    pub parameters: JsonValue,
    pub enabled: bool,
    pub is_read_only: bool,
    pub inherit_access_from_server: bool,
    pub last_synced_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct McpToolsList {
    pub tools: Vec<McpTool>,
    pub total: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct McpToolExecution {
    pub id: Uuid,
    pub server_id: Uuid,
    pub server_name: String,
    pub tool_name: String,
    pub conversation_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub user_email: Option<String>,
    #[schema(value_type = Object)]
    pub arguments: Option<JsonValue>,
    #[schema(value_type = Object)]
    pub result: Option<JsonValue>,
    pub is_error: bool,
    pub duration_ms: Option<i32>,
    pub executed_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PaginatedMcpToolExecutions {
    pub executions: Vec<McpToolExecution>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct McpAccessRule {
    pub id: Option<Uuid>,
    pub access_type: McpAccessType,
    pub role_name: Option<String>,
    pub department_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub permission: McpPermission,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct McpAccessRuleInput {
    pub access_type: McpAccessType,
    pub role_name: Option<String>,
    pub department_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub permission: McpPermission,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct McpServerAccessList {
    pub server_id: Uuid,
    pub server_name: String,
    pub default_access: McpDefaultAccess,
    pub rules: Vec<McpAccessRule>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct McpServerAccessUpdate {
    pub default_access: Option<McpDefaultAccess>,
    pub rules: Option<Vec<McpAccessRuleInput>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct McpToolAccessList {
    pub tool_id: Uuid,
    pub tool_name: String,
    pub server_id: Uuid,
    pub inherit_from_server: bool,
    pub rules: Vec<McpAccessRule>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct McpToolAccessUpdate {
    pub inherit_from_server: Option<bool>,
    pub rules: Option<Vec<McpAccessRuleInput>>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BulkToolAccessUpdate {
    pub tools: Vec<ToolAccessUpdateItem>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ToolAccessUpdateItem {
    pub tool_id: Uuid,
    pub inherit_from_server: Option<bool>,
    pub rules: Option<Vec<McpAccessRuleInput>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BulkToolAccessUpdateResponse {
    pub updated_count: usize,
    pub tools: Vec<McpToolAccessList>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct McpUserConnectionsList {
    pub connections: Vec<McpUserConnection>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct McpUserConnection {
    pub server_id: Uuid,
    pub server_name: String,
    pub description: Option<String>,
    pub connected: bool,
    pub connected_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub scopes: Option<Vec<String>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct McpAuthorizeResponse {
    pub success: bool,
    pub authorization_url: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct McpDisconnectResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct McpOauthCallbackResponse {
    pub success: bool,
    pub server_id: Uuid,
    pub status: String,
}
