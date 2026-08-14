// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// Typed tools configuration stored in a skill's toolsConfig JSON column.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SkillToolsConfig {
    pub web_search: bool,
    #[serde(default)]
    pub mcp_server_ids: Vec<Uuid>,
}

impl Default for SkillToolsConfig {
    fn default() -> Self {
        Self {
            web_search: false,
            mcp_server_ids: Vec::new(),
        }
    }
}

impl SkillToolsConfig {
    pub fn from_json(value: &serde_json::Value) -> Self {
        serde_json::from_value(value.clone()).unwrap_or_default()
    }
}

/// A file attachment uploaded with a skill create/update request.
/// `content_type` must be `text/markdown` (single .md) or `application/zip` (multiple .md files).
/// `data` is the base64-encoded file bytes.
#[derive(Debug, Deserialize, ToSchema)]
pub struct KnowledgeAttachment {
    pub file_name: String,
    pub content_type: String,
    pub data: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SkillKnowledgeInfo {
    pub id: Uuid,
    pub file_name: String,
    pub char_count: i32,
    pub storage_mode: String,
    pub created_at: DateTime<Utc>,
}

// ── Request types ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct UserSkillCreateRequest {
    pub name: String,
    pub description: Option<String>,
    pub avatar: Option<String>,
    pub instructions: Option<String>,
    pub tools_config: Option<SkillToolsConfig>,
    pub knowledge_attachment: Option<KnowledgeAttachment>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UserSkillUpdateRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub avatar: Option<String>,
    pub instructions: Option<String>,
    pub tools_config: Option<SkillToolsConfig>,
    pub is_active: Option<bool>,
    pub knowledge_attachment: Option<KnowledgeAttachment>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UserSkillListQuery {
    pub limit: Option<u64>,
    pub offset: Option<u64>,
    pub is_active: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SkillCreateRequest {
    pub identifier: String,
    pub name: String,
    pub description: Option<String>,
    pub avatar: Option<String>,
    pub instructions: Option<String>,
    pub tools_config: Option<SkillToolsConfig>,
    pub department_id: Option<Uuid>,
    pub knowledge_attachment: Option<KnowledgeAttachment>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SkillUpdateRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub avatar: Option<String>,
    pub instructions: Option<String>,
    pub tools_config: Option<SkillToolsConfig>,
    pub is_active: Option<bool>,
    pub department_id: Option<Uuid>,
    pub knowledge_attachment: Option<KnowledgeAttachment>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SkillListQuery {
    pub limit: Option<u64>,
    pub offset: Option<u64>,
    pub department_id: Option<Uuid>,
    pub is_active: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LinkSkillRequest {
    pub skill_id: Uuid,
}

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct SkillResponse {
    pub id: Uuid,
    pub identifier: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    pub tools_config: SkillToolsConfig,
    pub is_builtin: bool,
    pub is_active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub department_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub knowledge_files: Vec<SkillKnowledgeInfo>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SkillListResponse {
    pub skills: Vec<SkillResponse>,
    pub total: u64,
    pub limit: u64,
    pub offset: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ConversationSkillResponse {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub skill: SkillResponse,
    pub created_at: DateTime<Utc>,
}
