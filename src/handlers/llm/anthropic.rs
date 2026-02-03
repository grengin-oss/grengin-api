use crate::dto::llm::anthropic::{AnthropicStreamEvent, AnthropicDelta, AnthropicContentBlockResponse};
use super::{StreamParser, StreamParseResult, StreamWebSearchResult};

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
                Some("content_block_delta") => {
                    let delta_type = v.pointer("/delta/type").and_then(|x| x.as_str());
                    if delta_type == Some("thinking_delta") {
                        let message = v
                            .pointer("/delta/thinking")
                            .and_then(|x| x.as_str())
                            .map(|s| s.to_string());
                        return StreamParseResult::EventLog {
                            event_type: "thinking_delta".to_string(),
                            message,
                            data: Some(v.clone()),
                        };
                    }
                }
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

                AnthropicStreamEvent::ContentBlockStart { index, content_block } => match content_block {
                    AnthropicContentBlockResponse::ToolUse { id, name, input } => StreamParseResult::ToolCall {
                        tool_name: name,
                        tool_id: Some(id),
                        input: Some(input),
                        index: Some(index),
                        raw: None,
                    },
                    AnthropicContentBlockResponse::ServerToolUse { id, name, input } => StreamParseResult::ToolCall {
                        tool_name: name,
                        tool_id: Some(id),
                        input: Some(input),
                        index: Some(index),
                        raw: None,
                    },
                    AnthropicContentBlockResponse::WebSearchToolResult { tool_use_id, content } => {
                        let results = content
                            .into_iter()
                            .map(|item| StreamWebSearchResult {
                                title: item.title,
                                url: item.url,
                                source: None,
                                page_age: item.page_age,
                                snippet: None,
                            })
                            .collect::<Vec<StreamWebSearchResult>>();
                        StreamParseResult::WebSearchResult {
                            tool_name: "web_search_call".to_string(),
                            tool_id: Some(tool_use_id),
                            results,
                        }
                    }
                    _ => StreamParseResult::None,
                },

                AnthropicStreamEvent::ContentBlockDelta { index, delta } => match delta {
                    AnthropicDelta::TextDelta { text } => StreamParseResult::TextDelta {
                        text,
                        request_id: None,
                    },
                    AnthropicDelta::InputJsonDelta { partial_json } => {
                        StreamParseResult::ToolInput { partial_json, index: Some(index) }
                    }
                },

                AnthropicStreamEvent::Error { error } => StreamParseResult::Error {
                    error_type: error.error_type,
                    message: error.message,
                },

                AnthropicStreamEvent::MessageStop
                | AnthropicStreamEvent::ContentBlockStop { .. }
                | AnthropicStreamEvent::MessageDelta { .. }
                | AnthropicStreamEvent::Ping => StreamParseResult::None,
            },
            Err(_) => StreamParseResult::None,
        }
    }
}
