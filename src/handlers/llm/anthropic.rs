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

fn u64_to_u32(v: Option<u64>) -> Option<u32> {
    v.and_then(|x| u32::try_from(x).ok())
}

impl StreamParser for AnthropicStreamParser {
    fn parse_event(&self, data: &str) -> StreamParseResult {
        // 1) Raw JSON extraction for token usage (robust even if DTO is incomplete)
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
            match v.get("type").and_then(|t| t.as_str()) {
                Some("message_start") => {
                    let request_id = v
                        .pointer("/message/id")
                        .and_then(|x| x.as_str())
                        .unwrap_or_default()
                        .to_string();

                    let input_tokens =
                        u64_to_u32(v.pointer("/message/usage/input_tokens").and_then(|x| x.as_u64()));
                    let output_tokens =
                        u64_to_u32(v.pointer("/message/usage/output_tokens").and_then(|x| x.as_u64()));

                    // PATCH: MessageStart now carries tokens
                    return StreamParseResult::MessageStart {
                        request_id,
                        input_tokens,
                        output_tokens,
                    };
                }

                Some("message_delta") => {
                    // docs: usage.output_tokens is cumulative in message_delta
                    let output_tokens =
                        u64_to_u32(v.pointer("/usage/output_tokens").and_then(|x| x.as_u64()));

                    if output_tokens.is_some() {
                        return StreamParseResult::TokenUsage {
                            request_id: None,
                            input_tokens: None,
                            output_tokens,
                            total_tokens: None,
                        };
                    }
                }

                _ => {}
            }
        }

        // 2) Your existing typed parsing for text/tool/error
        match serde_json::from_str::<AnthropicStreamEvent>(data) {
            Ok(stream_event) => match stream_event {
                // If you still want request_id events for non-usage flows,
                // you can keep this, but note message_start is already handled above.
                AnthropicStreamEvent::MessageStart { message } => StreamParseResult::MessageStart {
                    request_id: message.id,
                    input_tokens: None,
                    output_tokens: None,
                },

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
