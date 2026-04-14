use serde::{Deserialize, Serialize};
use crate::{llm::prompt::Prompt, models::messages::ChatRole};

#[derive(Debug, Clone, Serialize)]
pub struct MistralChatCompletionRequest {
    pub model: String,
    pub messages: Vec<MistralMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<MistralTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MistralAgentCreateRequest {
    pub model: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub instructions: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<MistralTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_args: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MistralAgentCreateResponse {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MistralChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<MistralChatChoice>,
    #[serde(default)]
    pub usage: Option<MistralUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MistralChatChoice {
    pub index: u32,
    pub message: MistralMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MistralChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<MistralChatChunkChoice>,
    #[serde(default)]
    pub usage: Option<MistralUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MistralChatChunkChoice {
    pub index: u32,
    pub delta: MistralMessageDelta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MistralUsage {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MistralMessageDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<MistralToolCallDelta>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MistralMessage {
    pub role: ChatRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<MistralToolCall>>,
}

impl MistralMessage {
    pub fn from_prompts(prompts: Vec<Prompt>) -> Vec<Self> {
        prompts
            .into_iter()
            .map(|prompt| MistralMessage {
                role: prompt.role,
                content: Some(prompt.text),
                name: None,
                tool_call_id: None,
                tool_calls: None,
            })
            .collect()
    }

    pub fn assistant_with_tool_calls(content: Option<String>, tool_calls: Vec<MistralToolCall>) -> Self {
        MistralMessage {
            role: ChatRole::Assistant,
            content,
            name: None,
            tool_call_id: None,
            tool_calls: Some(tool_calls),
        }
    }

    pub fn tool_response(name: String, tool_call_id: String, content: String) -> Self {
        MistralMessage {
            role: ChatRole::Tool,
            content: Some(content),
            name: Some(name),
            tool_call_id: Some(tool_call_id),
            tool_calls: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MistralToolCallDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<MistralToolFunctionDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MistralToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: MistralToolFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MistralToolFunctionDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MistralToolFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MistralTool {
    #[serde(rename = "function")]
    Function { function: MistralToolDefinition },
    #[serde(rename = "web_search")]
    WebSearch,
    #[serde(rename = "web_search_premium")]
    WebSearchPremium,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MistralToolDefinition {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: serde_json::Value,
}
