use serde::{Deserialize, Serialize};
use crate::{llm::prompt::Prompt, models::messages::ChatRole};

pub const GROQ_DEFAULT_MAX_TOKENS: i32 = 8192;

#[derive(Serialize, Deserialize)]
pub struct GroqChatRequest {
    pub model: String,
    pub messages: Vec<GroqMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GroqChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<GroqChunkChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x_groq: Option<GroqMetadata>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GroqMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<GroqUsage>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GroqUsage {
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub total_tokens: i32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GroqChunkChoice {
    pub index: u32,
    pub delta: GroqMessageDelta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GroqChatCompletionResponse {
    pub id: String,
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub created: Option<i64>,
    #[serde(default)]
    pub model: Option<String>,
    pub choices: Vec<GroqChoiceResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<GroqUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x_groq: Option<GroqMetadata>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GroqChoiceResponse {
    pub index: u32,
    pub message: GroqResponseMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GroqResponseMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GroqMessageDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct GroqMessage {
    pub role: String,
    pub content: String,
}

impl GroqMessage {
    pub fn new(role: &str, content: String) -> Self {
        Self {
            role: role.to_string(),
            content,
        }
    }

    pub fn from_prompts(prompts: Vec<Prompt>) -> Vec<Self> {
        prompts
            .into_iter()
            .map(|prompt| {
                let role = match prompt.role {
                    ChatRole::User => "user",
                    ChatRole::Assistant => "assistant",
                    ChatRole::System => "system",
                    ChatRole::Tool => "tool",
                };
                Self::new(role, prompt.text)
            })
            .collect()
    }

    pub fn from_text(role: &str, text: String) -> Self {
        Self::new(role, text)
    }
}
