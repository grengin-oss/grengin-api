use chrono::{DateTime, Utc};
use sea_orm::{entity::prelude::*, JsonValue};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize, ToSchema)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum McpAccessDefault {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize, ToSchema)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum McpDefaultAccess {
    ExplicitOnly,
    AllowAll,
    DenyAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize, ToSchema)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum McpTransportType {
    Stdio,
    Http,
    Sse,
    Websocket,
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "mcp_servers", rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[sea_orm(primary_key, unique)]
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub transport_type: McpTransportType,
    pub connection_config: JsonValue,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub url: Option<String>,
    pub enabled: bool,
    pub status: Option<String>,
    pub status_message: Option<String>,
    pub tool_count: i32,
    /// Legacy access flag (pre-MCP revamp).
    pub access_default: McpAccessDefault,
    /// Revamped access flag used by new MCP access policies.
    pub default_access: McpDefaultAccess,
    pub last_connected_at: Option<DateTime<Utc>>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::mcp_server_access_rules::Entity")]
    AccessRules,
}

impl Related<super::mcp_server_access_rules::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::AccessRules.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
