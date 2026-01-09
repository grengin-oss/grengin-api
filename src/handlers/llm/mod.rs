pub mod openai;
pub mod anthropic;

use crate::dto::chat_stream::ChatStream;
use uuid::Uuid;

/// Result of parsing a streaming event
#[derive(Debug, Clone)]
pub enum StreamParseResult {
    None,

    // PATCH: include optional usage on start (useful for Anthropic message_start)
    MessageStart {
        request_id: String,
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
    },

    TextDelta {
        text: String,
        request_id: Option<String>,
    },

    ToolInput {
        partial_json: String,
    },

    // NEW: token usage updates mid/final stream
    TokenUsage {
        request_id: Option<String>,
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
        total_tokens: Option<u32>,
    },

    Error {
        error_type: String,
        message: String,
    },
}

/// Trait for parsing provider-specific streaming events
pub trait StreamParser: Send + Sync {
    /// Parse a raw SSE message data string into a StreamParseResult
    fn parse_event(&self, data: &str) -> StreamParseResult;
}

impl StreamParseResult {
    /// Convert to a ChatStream event if this result contains content
    pub fn to_chat_stream(&self, conversation_id: Uuid) -> Option<ChatStream> {
        match self {
            StreamParseResult::TextDelta { text, .. } => Some(ChatStream {
                id: conversation_id,
                role: None,
                content: Some(text.clone()),
            }),
            _ => None,
        }
    }

    /// Extract request_id if available
    pub fn request_id(&self) -> Option<String> {
        match self {
            StreamParseResult::TextDelta { request_id, .. } => request_id.clone(),
            StreamParseResult::MessageStart { request_id,input_tokens:_,output_tokens:_ } => Some(request_id.clone()),
            _ => None,
        }
    }

    /// Extract text content if available
    pub fn text(&self) -> Option<&str> {
        match self {
            StreamParseResult::TextDelta { text, .. } => Some(text),
            _ => None,
        }
    }
}
