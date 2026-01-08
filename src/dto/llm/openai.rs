use serde::{Deserialize, Serialize};
use crate::{dto::files::File, llm::prompt::Prompt, models::messages::ChatRole};

//
// ---------------------------
// Requests
// ---------------------------
//

// Responses API request (/v1/responses) style
#[derive(Serialize, Deserialize)]
pub struct OpenaiChatRequest {
    pub model: String,

    // Responses API accepts string or array; you're using the array form.
    pub input: Vec<OpenaiMessage>,

    #[serde(default)]
    pub stream: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    // NEW: enable built-in tools like web_search
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<OpenaiTool>>,

    // NEW: control tool selection ("auto"/"none"/{...})
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<OpenaiToolChoice>,

    // NEW: request extra output fields (e.g., sources from web search)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
}

// Chat Completions request (/v1/chat/completions)
#[derive(Serialize, Deserialize)]
pub struct OpenaiChatCompletionRequest {
    pub model: String,
    pub messages: Vec<OpenaiMessage>,

    #[serde(default)]
    pub stream: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenaiChatStreamOptions {
    #[serde(default)]
    pub include_usage: bool,
}

//
// ---------------------------
// Built-in tools (web_search)
// ---------------------------
//

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OpenaiTool {
    #[serde(rename = "web_search")]
    WebSearch {
        #[serde(skip_serializing_if = "Option::is_none")]
        filters: Option<OpenaiWebSearchFilters>,
    },

    // Optional: keep forward-compatible if you ever use preview tool
    #[serde(rename = "web_search_preview")]
    WebSearchPreview {
        #[serde(skip_serializing_if = "Option::is_none")]
        filters: Option<OpenaiWebSearchFilters>,
    },

    // Forward compatibility
    #[serde(other)]
    Other,
}

impl OpenaiTool {
    pub fn web_search() -> Self {
        Self::WebSearch { filters: None }
    }

    pub fn web_search_allowed_domains<I, S>(domains: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::WebSearch {
            filters: Some(OpenaiWebSearchFilters {
                allowed_domains: domains.into_iter().map(Into::into).collect(),
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenaiWebSearchFilters {
    pub allowed_domains: Vec<String>,
}

// tool_choice can be "auto" | "none" | { "type": "web_search" } etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OpenaiToolChoice {
    String(String),
    Object(OpenaiToolChoiceObject),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenaiToolChoiceObject {
    #[serde(rename = "type")]
    pub tool_type: String,
}

//
// ---------------------------
// Responses API streaming events
// ---------------------------
//

// NOTE: usage comes on response.completed (and in response objects in general)
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum OpenaiResponseStreamEvent {
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta(OpenaiOutputTextDelta),

    // NEW: capture response lifecycle events so you can read usage on completion
    #[serde(rename = "response.created")]
    ResponseCreated(OpenaiResponseEvent),

    #[serde(rename = "response.in_progress")]
    ResponseInProgress(OpenaiResponseEvent),

    #[serde(rename = "response.completed")]
    ResponseCompleted(OpenaiResponseEvent),

    // Optional: error event
    #[serde(rename = "error")]
    Error(OpenaiStreamErrorEvent),

    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub struct OpenaiOutputTextDelta {
    pub item_id: String,
    pub delta: String,
}

#[derive(Debug, Deserialize)]
pub struct OpenaiResponseEvent {
    pub response: OpenaiResponseObject,
    pub sequence_number: i64,
}

// Minimal response object (add more fields if you need them)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenaiResponseObject {
    pub id: String,

    #[serde(default)]
    pub status: Option<String>,

    // NEW: token usage (present on completed responses; null earlier)
    #[serde(default)]
    pub usage: Option<OpenaiResponsesUsage>,
}

#[derive(Debug, Deserialize)]
pub struct OpenaiStreamErrorEvent {
    pub error: OpenaiErrorObject,
    pub sequence_number: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct OpenaiErrorObject {
    pub message: Option<String>,
    pub r#type: Option<String>,
    pub code: Option<String>,
}

//
// ---------------------------
// Token usage structs
// ---------------------------
//

// Responses API usage: input/output/total (+ details)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenaiResponsesUsage {
    pub input_tokens: u32,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens_details: Option<OpenaiInputTokensDetails>,

    pub output_tokens: u32,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens_details: Option<OpenaiOutputTokensDetails>,

    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenaiInputTokensDetails {
    #[serde(default)]
    pub cached_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenaiOutputTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: u32,
}

// Chat Completions usage: prompt/completion/total (+ details)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenaiChatUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_details: Option<OpenaiPromptTokensDetails>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens_details: Option<OpenaiCompletionTokensDetails>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenaiPromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: u32,
    #[serde(default)]
    pub audio_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenaiCompletionTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: u32,
    #[serde(default)]
    pub audio_tokens: u32,
    #[serde(default)]
    pub accepted_prediction_tokens: u32,
    #[serde(default)]
    pub rejected_prediction_tokens: u32,
}

//
// ---------------------------
// Chat Completions responses (patched to include usage)
// ---------------------------
//

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenaiChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<OpenaiChatChunkChoice>,

    // NEW: present on final chunk when stream_options.include_usage = true
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<OpenaiChatUsage>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenaiChatChunkChoice {
    pub index: u32,
    pub delta: OpenaiMessageDelta,
    pub finish_reason: Option<String>,
}

// PATCH: include usage for non-stream responses too
#[derive(Debug, Serialize, Deserialize)]
pub struct OpenaiChatCompletionResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<i64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    pub choices: Vec<OpenaiChatChoiceResponse>,

    // NEW
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<OpenaiChatUsage>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenaiChatChoiceResponse {
    pub index: u32,
    pub message: OpenaiMessageDelta,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenaiMessageDelta {
    pub role: Option<String>,
    pub content: Option<String>,
}

//
// ---------------------------
// Your existing message/content DTOs (unchanged)
// ---------------------------
//

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenaiContentType {
    #[serde(rename = "input_text")]
    InputText,
    #[serde(rename = "output_text")]
    OutputText,
    Text,
    #[serde(rename = "input_file")]
    InputFile,
    #[serde(rename = "input_image")]
    InputImage,
}

#[derive(Serialize, Deserialize)]
pub struct OpenaiMessage {
    pub role: ChatRole,
    pub content: Vec<OpenaiContent>,
}

impl OpenaiMessage {
    pub fn from_text_and_files_input(prompts: Vec<String>, files: Vec<File>) -> Self {
        let mut content = vec![];
        for file in files {
            content.push(OpenaiContent {
                content_type: OpenaiContentType::InputFile,
                text: None,
                file_id: file.openai_id,
            });
        }
        for prompt in prompts {
            content.push(OpenaiContent {
                content_type: OpenaiContentType::InputText,
                text: Some(prompt),
                file_id: None,
            })
        }
        OpenaiMessage {
            role: ChatRole::User,
            content,
        }
    }

    pub fn from_text(prompts: Vec<String>) -> Self {
        let mut content = vec![];
        for prompt in prompts {
            content.push(OpenaiContent {
                content_type: OpenaiContentType::Text,
                text: Some(prompt),
                file_id: None,
            })
        }
        OpenaiMessage {
            role: ChatRole::User,
            content,
        }
    }

    pub fn from_prompts(prompts: Vec<Prompt>) -> Vec<Self> {
        let mut messages = vec![];
        for prompt in prompts {
            let mut content = vec![];
            let content_type = if prompt.role == ChatRole::Assistant {
                OpenaiContentType::OutputText
            } else {
                OpenaiContentType::InputText
            };
            content.push(OpenaiContent {
                content_type,
                text: Some(prompt.text),
                file_id: None,
            });
            for file in prompt.files {
                let content_type = if file.content_type.contains("image") {
                    OpenaiContentType::InputImage
                } else {
                    OpenaiContentType::InputFile
                };
                content.push(OpenaiContent {
                    content_type,
                    file_id: file.openai_id,
                    text: None,
                })
            }
            messages.push(Self {
                role: prompt.role,
                content,
            });
        }
        messages
    }
}

#[derive(Serialize, Deserialize)]
pub struct OpenaiContent {
    #[serde(rename = "type")]
    pub content_type: OpenaiContentType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
}

#[derive(Deserialize)]
pub struct FileUploadResponse {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenaiModel {
    pub id: String,
    pub object: String,
    pub created: Option<i64>,
    pub owned_by: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenaiListModelsResponse {
    pub object: String,
    pub data: Vec<OpenaiModel>,
}
