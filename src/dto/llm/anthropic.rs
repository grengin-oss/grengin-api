use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::{llm::prompt::Prompt, models::messages::ChatRole};

// ============== Constants ==============

pub const ANTHROPIC_DEFAULT_MAX_TOKENS: i32 = 4096;

// ============== Request Structures ==============

#[derive(Serialize, Deserialize)]
pub struct AnthropicChatRequest {
    pub model: String,
    pub max_tokens: i32,
    pub messages: Vec<AnthropicMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<AnthropicToolUnion>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AnthropicMessage {
    pub role: AnthropicRole,
    pub content: AnthropicContent,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AnthropicRole {
    User,
    Assistant,
}

/// Content can be a simple string or an array of content blocks
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum AnthropicContent {
    Text(String),
    Blocks(Vec<AnthropicContentBlock>),
}

/// Content block types for requests
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: AnthropicImageSource },
    #[serde(rename = "document")]
    Document { source: AnthropicDocSource },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

/// Image source (base64 or URL)
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AnthropicImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Document source (base64 or URL) for PDF support
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AnthropicDocSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub media_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

// ============== Tool Definitions ==============

/// Union type for client tools and server tools
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum AnthropicToolUnion {
    ClientTool(AnthropicTool),
    WebSearchTool(AnthropicWebSearchTool),
}

/// Client-defined tools
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AnthropicTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Web search server tool
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AnthropicWebSearchTool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_uses: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_domains: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_domains: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_location: Option<AnthropicUserLocation>,
}

impl AnthropicWebSearchTool {
    pub fn new(max_uses: Option<i32>) -> Self {
        Self {
            tool_type: "web_search_20250305".to_string(),
            name: "web_search".to_string(),
            max_uses,
            allowed_domains: None,
            blocked_domains: None,
            user_location: None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AnthropicUserLocation {
    #[serde(rename = "type")]
    pub location_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

// ============== Streaming Event Structures ==============

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicStreamEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: AnthropicMessageResponse },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: u32,
        content_block: AnthropicContentBlockResponse,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: u32, delta: AnthropicDelta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: u32 },
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: AnthropicMessageDeltaInfo,
        usage: AnthropicUsage,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "error")]
    Error { error: AnthropicError },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Deserialize)]
pub struct AnthropicMessageDeltaInfo {
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AnthropicError {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

// ============== Response Structures ==============

#[derive(Debug, Deserialize)]
pub struct AnthropicMessageResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub message_type: String,
    pub role: String,
    pub content: Vec<AnthropicContentBlockResponse>,
    pub model: String,
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
    pub usage: AnthropicUsage,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum AnthropicContentBlockResponse {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "server_tool_use")]
    ServerToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "web_search_tool_result")]
    WebSearchToolResult {
        tool_use_id: String,
        content: Vec<WebSearchResult>,
    },
}

#[derive(Debug, Deserialize, Clone)]
pub struct WebSearchResult {
    #[serde(rename = "type")]
    pub result_type: String,
    pub url: String,
    pub title: String,
    pub encrypted_content: Option<String>,
    pub page_age: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AnthropicUsage {
    pub input_tokens: i32,
    pub output_tokens: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<i32>,
}

/// Non-streaming response (used for title generation, etc.)
#[derive(Debug, Deserialize)]
pub struct AnthropicChatResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub response_type: String,
    pub role: String,
    pub content: Vec<AnthropicContentBlockResponse>,
    pub model: String,
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
    pub usage: AnthropicUsage,
}

// ============== Conversion Methods ==============

impl AnthropicMessage {
    /// Convert from internal Prompt format to Anthropic message format
    ///
    /// `file_data_loader` is an optional function that loads file data given a file ID.
    /// This allows loading files from disk without coupling the DTO to file storage.
    pub fn from_prompts<F>(prompts: Vec<Prompt>, file_data_loader: Option<F>) -> (Vec<Self>, Option<String>)
    where
        F: Fn(&str) -> Option<String>,
    {
        let mut messages = Vec::new();
        let mut system_prompt = None;

        for prompt in prompts {
            // Handle system messages separately
            if prompt.role == ChatRole::System {
                system_prompt = Some(prompt.text.clone());
                continue;
            }

            let role = match prompt.role {
                ChatRole::User => AnthropicRole::User,
                ChatRole::Assistant => AnthropicRole::Assistant,
                ChatRole::Tool => AnthropicRole::User,
                ChatRole::System => continue, // Already handled above
            };

            // Build content blocks
            let mut blocks = Vec::new();

            // Add text content
            if !prompt.text.is_empty() {
                blocks.push(AnthropicContentBlock::Text {
                    text: prompt.text.clone(),
                });
            }

            // Add files as image or document blocks
            for file in &prompt.files {
                // Load file data using the provided loader if we have a file ID
                let data = file.id.as_ref().and_then(|id| {
                    file_data_loader.as_ref().and_then(|loader| loader(id))
                });

                if let Some(data) = data {
                    if file.content_type.starts_with("image/") {
                        blocks.push(AnthropicContentBlock::Image {
                            source: AnthropicImageSource::base64(&file.content_type, data),
                        });
                    } else if file.content_type == "application/pdf" {
                        blocks.push(AnthropicContentBlock::Document {
                            source: AnthropicDocSource::base64_pdf(data),
                        });
                    }
                }
            }

            let content = if blocks.len() == 1 {
                if let AnthropicContentBlock::Text { ref text } = blocks[0] {
                    AnthropicContent::Text(text.clone())
                } else {
                    AnthropicContent::Blocks(blocks)
                }
            } else if blocks.is_empty() {
                // Empty content, skip this message
                continue;
            } else {
                AnthropicContent::Blocks(blocks)
            };

            messages.push(AnthropicMessage { role, content });
        }

        (messages, system_prompt)
    }

    /// Create a simple text message
    pub fn from_text(role: AnthropicRole, text: String) -> Self {
        Self {
            role,
            content: AnthropicContent::Text(text),
        }
    }

    /// Create a message with content blocks
    pub fn with_blocks(role: AnthropicRole, blocks: Vec<AnthropicContentBlock>) -> Self {
        Self {
            role,
            content: AnthropicContent::Blocks(blocks),
        }
    }
}

impl AnthropicImageSource {
    /// Create a base64 image source
    pub fn base64(media_type: &str, data: String) -> Self {
        Self {
            source_type: "base64".to_string(),
            media_type: Some(media_type.to_string()),
            data: Some(data),
            url: None,
        }
    }

    /// Create a URL image source
    pub fn url(url: String) -> Self {
        Self {
            source_type: "url".to_string(),
            media_type: None,
            data: None,
            url: Some(url),
        }
    }
}

impl AnthropicDocSource {
    /// Create a base64 PDF source
    pub fn base64_pdf(data: String) -> Self {
        Self {
            source_type: "base64".to_string(),
            media_type: "application/pdf".to_string(),
            data: Some(data),
            url: None,
        }
    }

    /// Create a URL PDF source
    pub fn url_pdf(url: String) -> Self {
        Self {
            source_type: "url".to_string(),
            media_type: "application/pdf".to_string(),
            data: None,
            url: Some(url),
        }
    }
}
