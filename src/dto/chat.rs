// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    dto::files::File,
    models::messages::{ChatRole, StopReason},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Serialize, ToSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    Lexical,
    Semantic,
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct ArchiveChatRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub archived: Option<bool>,
    #[serde(default)]
    pub pinned: Option<bool>,
}

#[derive(Serialize, ToSchema, IntoParams)]
pub struct ConversationResponse {
    pub id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub web_search_enabled: bool,
    pub archived: bool,
    pub pinned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<DateTime<Utc>>,
    pub model: String,
    pub total_tokens: i64,
    pub total_cost: f32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<MessageResponse>>,
    pub message_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_mode: Option<SearchMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_score: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_snippet: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct PaginatedConversations {
    pub total: u64,
    pub limit: u64,
    pub offset: u64,
    pub conversations: Vec<ConversationResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_results: Option<HashMap<Uuid, SemanticResult>>,
}

#[derive(Serialize, ToSchema)]
pub struct SemanticResult {
    pub message_id: Uuid,
    pub snippet: String,
    pub distance: f64,
}

#[derive(Serialize, ToSchema, IntoParams)]
pub struct MessageResponse {
    pub id: Uuid,
    pub role: ChatRole,
    pub cost: f32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub request_id: Option<String>,
    pub model: String,
    pub model_params: Option<serde_json::Value>,
    pub tool_calls: Vec<serde_json::Value>,
    pub tools_results: Vec<serde_json::Value>,
    pub parts: MessageParts,
    pub usage: TokenUsage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub versions: Option<MessageVersions>,
}

#[derive(Serialize, ToSchema, Clone, Debug, PartialEq, Eq)]
#[schema(
    description = "Present on a message that has other versions, which edits create; render as \"< current / total >\". current is this message's 1-based position among its versions, total is how many there are, and message_ids lists every version oldest first. Send one of those ids to PUT /chat/{chat_id}/branch to show that version.",
    example = json!({
        "current": 2,
        "total": 2,
        "message_ids": ["5d0f3c1e-8a2b-4b6e-9f3a-1c2d3e4f5a60", "9b2f6c1e-4d7a-4f3b-8c55-2a1e0f9d7b10"]
    })
)]
pub struct MessageVersions {
    pub current: u32,
    pub total: u32,
    pub message_ids: Vec<Uuid>,
}

#[derive(Deserialize, ToSchema)]
#[schema(
    description = "message_id is the message to show: usually one of a message's versions.message_ids from GET /chat/{chat_id}, but any message of the conversation is accepted.",
    example = json!({ "message_id": "5d0f3c1e-8a2b-4b6e-9f3a-1c2d3e4f5a60" })
)]
pub struct SelectBranchRequest {
    pub message_id: Uuid,
}

#[derive(Serialize, Deserialize, ToSchema, IntoParams)]
pub struct ArtifactMeta {
    pub id: Uuid,
    pub file_id: Uuid,
    pub title: String,
    pub content_type: String,
}

#[derive(Serialize, ToSchema, IntoParams)]
pub struct MessageParts {
    pub text: String,
    pub files: Option<Vec<File>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Vec<ArtifactMeta>>,
}

#[derive(Serialize, ToSchema, IntoParams)]
pub struct TokenUsage {
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub total_tokens: i32,
    pub cached_input_tokens: i32,
    pub cache_creation_tokens: i32,
}
