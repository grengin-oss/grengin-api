use crate::dto::llm::anthropic::{AnthropicStreamEvent, AnthropicDelta};
use super::{StreamParser, StreamParseResult};

/// Anthropic stream parser
pub struct AnthropicStreamParser;

impl AnthropicStreamParser {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AnthropicStreamParser {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamParser for AnthropicStreamParser {
    fn parse_event(&self, data: &str) -> StreamParseResult {
        match serde_json::from_str::<AnthropicStreamEvent>(data) {
            Ok(stream_event) => match stream_event {
                AnthropicStreamEvent::MessageStart { message } => {
                    StreamParseResult::MessageStart {
                        request_id: message.id,
                    }
                }
                AnthropicStreamEvent::ContentBlockDelta { delta, .. } => match delta {
                    AnthropicDelta::TextDelta { text } => StreamParseResult::TextDelta {
                        text,
                        request_id: None,
                    },
                    AnthropicDelta::InputJsonDelta { partial_json } => {
                        StreamParseResult::ToolInput { partial_json }
                    }
                },
                AnthropicStreamEvent::Error { error } => StreamParseResult::Error {
                    error_type: error.error_type,
                    message: error.message,
                },
                AnthropicStreamEvent::MessageStop
                | AnthropicStreamEvent::ContentBlockStart { .. }
                | AnthropicStreamEvent::ContentBlockStop { .. }
                | AnthropicStreamEvent::MessageDelta { .. }
                | AnthropicStreamEvent::Ping => StreamParseResult::None,
            },
            Err(_) => StreamParseResult::None,
        }
    }
}
