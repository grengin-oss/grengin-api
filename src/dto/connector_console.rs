// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{
    dto::mcp::{McpServerAccessList, McpTool},
    models::mcp_servers::McpDefaultAccess,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorAuthenticationMethod {
    Oauth2,
    ApiToken,
    GrenginBroker,
    ServerManaged,
    Unavailable,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ConnectorIntegration {
    pub suggested_prompts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_version: Option<String>,
    pub authentication: ConnectorAuthenticationMethod,
    pub supported_authentication: Vec<ConnectorAuthenticationMethod>,
    pub per_user_credentials: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorConnectionState {
    Connected,
    NotConnected,
    NeedsReconnect,
    Unavailable,
}

#[derive(Serialize, ToSchema)]
pub struct ConnectorPerson {
    pub user_id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub picture: Option<String>,
    pub connection_state: ConnectorConnectionState,
    pub connected_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, ToSchema)]
pub struct ConnectorPeople {
    pub people: Vec<ConnectorPerson>,
    pub total: u64,
    pub connected: u64,
    pub limit: u64,
    pub offset: u64,
}

#[derive(Default, Deserialize, IntoParams)]
pub struct ConnectorPeopleQuery {
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

#[derive(Serialize, ToSchema)]
pub struct ConnectorUsage {
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub tool_calls: u64,
    pub successful_tool_calls: u64,
    pub failed_tool_calls: u64,
}

#[derive(Serialize, ToSchema)]
pub struct ConnectorConsole {
    pub server_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub enabled: bool,
    pub integration: ConnectorIntegration,
    pub my_connection_state: ConnectorConnectionState,
    pub active_user_count: u64,
    pub connected_user_count: u64,
    pub default_access: McpDefaultAccess,
    pub access: McpServerAccessList,
    pub tools: Vec<McpTool>,
    pub usage: ConnectorUsage,
    pub reminders_supported: bool,
    pub write_confirmation_supported: bool,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectorToolUpdate {
    pub enabled: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct McpToolResultDetails {
    pub execution_id: Uuid,
    pub server_id: Uuid,
    pub server_name: String,
    pub tool_name: String,
    pub duration_ms: i32,
    pub executed_at: DateTime<Utc>,
    pub preview: McpResultPreview,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct McpResultPreview {
    pub text: String,
    pub truncated: bool,
    pub empty: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_count: Option<usize>,
}
