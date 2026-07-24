use crate::{dto::files::File, models::messages::ChatRole};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct ArchiveChatRequest {
    pub title: String,
    pub archived: bool,
}

#[derive(Serialize, ToSchema, IntoParams)]
pub struct ConversationResponse {
    pub id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub web_search_enabled: bool,
    pub archived: bool,
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
    pub search_score: Option<f32>,
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
}
