use crate::{dto::files::File, models::messages::ChatRole};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActiveSkillInfo {
    pub id: Uuid,
    pub identifier: String,
    pub name: String,
    pub avatar: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillsActivePayload {
    pub skills: Vec<ActiveSkillInfo>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BudgetWarningPayload {
    pub department_id: Uuid,
    pub budget_available: String,
    pub action: &'static str,
    pub message: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatStreamEvents {
    Conversation,
    Skills,
    MessageStart,
    Delta,
    MessageEnd,
    Event,
    ToolCall,
    ToolResult,
    Cancelled,
    Done,
    #[serde(rename = "artifact_start")]
    ArtifactStart,
    #[serde(rename = "artifact_delta")]
    ArtifactDelta,
    #[serde(rename = "artifact_end")]
    ArtifactEnd,
    #[serde(rename = "artifact_saved")]
    ArtifactSaved,
    #[serde(rename = "budget_warning")]
    DepartmentBudgetWarning,
    #[serde(rename = "llm_error")]
    LlmError,
}

impl ChatStreamEvents {
    pub fn to_string(&self) -> String {
        serde_json::to_string(self).unwrap()
    }
}

#[derive(Serialize, ToSchema, IntoParams)]
pub struct ChatStream {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Uuid>,
    #[serde(rename = "text", skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_new: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<ChatStreamEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<ChatStreamToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<ChatStreamToolResult>,
}

impl ChatStream {
    pub fn to_string(&self) -> String {
        serde_json::to_string(self).unwrap()
    }
}

#[derive(Serialize, ToSchema, IntoParams)]
pub struct ChatStreamEvent {
    pub event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<ChatStreamPayload>,
}

#[derive(Serialize, ToSchema, IntoParams)]
pub struct ChatStreamToolCall {
    pub tool_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<ChatStreamToolInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ChatToolKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_search: Option<ChatStreamWebSearchAction>,
}

#[derive(Serialize, ToSchema, IntoParams)]
pub struct ChatStreamToolResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ChatToolKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object)]
    pub output: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_search: Option<ChatStreamWebSearchResult>,
}

#[derive(Serialize, ToSchema, IntoParams)]
pub struct ChatStreamPayload {
    #[schema(value_type = Object)]
    pub value: serde_json::Value,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatToolKind {
    WebSearch,
    Other,
}

#[derive(Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatStreamToolInput {
    Text {
        text: String,
    },
    Json {
        #[schema(value_type = Object)]
        value: serde_json::Value,
    },
}

#[derive(Clone, Serialize, ToSchema, IntoParams)]
pub struct ChatStreamWebSearchAction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queries: Option<Vec<String>>,
}

#[derive(Serialize, ToSchema, IntoParams, Clone)]
pub struct ChatStreamWebSearchResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queries: Option<Vec<String>>,
    pub results: Vec<ChatStreamWebSearchResultItem>,
}

#[derive(Serialize, ToSchema, IntoParams, Clone)]
pub struct ChatStreamWebSearchResultItem {
    pub title: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_age: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct ChatInput {
    pub provider: Option<String>,
    pub model_name: Option<String>,
    pub config: Option<serde_json::Value>,
    #[serde(default)]
    pub web_search: bool,
    pub selected_tools: Option<Vec<String>>,
    pub selected_mcp_servers: Option<Vec<Uuid>>,
    pub selected_skills: Option<Vec<Uuid>>,
    pub conversation_id: Option<Uuid>,
    pub messages: Vec<MessageRequest>,
    pub temperature: Option<f32>,
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct MessageRequest {
    pub role: ChatRole,
    pub content: String,
    pub files: Vec<File>,
}
