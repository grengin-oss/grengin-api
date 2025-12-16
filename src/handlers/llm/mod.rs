pub mod openai;
pub mod anthropic;
pub mod groq;

use crate::dto::chat_stream::ChatStream;
use uuid::Uuid;

/// Result of parsing a streaming event
pub enum StreamParseResult {
    /// Text content received
    TextDelta { text: String, request_id: Option<String> },
    /// Message started (with optional request id)
    MessageStart { request_id: String },
    /// Tool input streaming (for future support)
    ToolInput { partial_json: String },
    /// Error occurred
    Error { error_type: String, message: String },
    /// Nothing to yield (ping, ignored event, etc.)
    None,
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
            StreamParseResult::MessageStart { request_id } => Some(request_id.clone()),
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
