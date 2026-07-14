use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// Typed tools configuration stored in a skill's toolsConfig JSON column.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillToolsConfig {
    pub web_search: bool,
    #[serde(default)]
    pub mcp_server_ids: Vec<Uuid>,
}

impl Default for SkillToolsConfig {
    fn default() -> Self {
        Self { web_search: false, mcp_server_ids: Vec::new() }
    }
}

impl SkillToolsConfig {
    pub fn from_json(value: &serde_json::Value) -> Self {
        serde_json::from_value(value.clone()).unwrap_or_default()
    }
}

// ── Request types ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillCreateRequest {
    pub identifier: String,
    pub name: String,
    pub description: Option<String>,
    pub avatar: Option<String>,
    pub system_role: Option<String>,
    pub tools_config: Option<SkillToolsConfig>,
    pub department_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillUpdateRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub avatar: Option<String>,
    pub system_role: Option<String>,
    pub tools_config: Option<SkillToolsConfig>,
    pub is_active: Option<bool>,
    pub department_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillListQuery {
    pub limit: Option<u64>,
    pub offset: Option<u64>,
    pub department_id: Option<Uuid>,
    pub is_active: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LinkSkillRequest {
    pub skill_id: Uuid,
}

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillResponse {
    pub id: Uuid,
    pub identifier: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_role: Option<String>,
    pub tools_config: SkillToolsConfig,
    pub is_builtin: bool,
    pub is_active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub department_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillListResponse {
    pub skills: Vec<SkillResponse>,
    pub total: u64,
    pub limit: u64,
    pub offset: u64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSkillResponse {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub skill: SkillResponse,
    pub created_at: DateTime<Utc>,
}
