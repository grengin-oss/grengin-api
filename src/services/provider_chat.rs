// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{collections::HashMap, pin::Pin};

use futures_util::{Stream, StreamExt};
use grengin_provider::{
    ChatMessage, ChatRequest, ContentPart, ModelId, ProviderError, ProviderEvent,
    ProviderEventErrorKind, ProviderEventStream, TokenUsage, ToolCallId, ToolChoice,
    ToolDefinition,
};
use reqwest::{Response, StatusCode};
use reqwest_eventsource::{Event as ReqwestEvent, EventSource};
use serde_json::Value;

use crate::{
    handlers::llm::{
        StreamErrorKind, StreamParseResult, StreamParser, StreamWebSearchResult, ToolCall,
        ToolInput, build_tool_input_delta,
    },
    llm::prompt::Prompt,
    models::messages::ChatRole,
    services::mcp_tools::McpToolDescriptor,
};

pub enum LlmStreamEvent {
    Open,
    Message { event: String, data: String },
}

pub enum LlmStreamError {
    Ended,
    InvalidStatus(StatusCode, Response),
    Provider(ProviderError),
    Connection,
}

pub type LlmEventStream =
    Pin<Box<dyn Stream<Item = Result<LlmStreamEvent, LlmStreamError>> + Send>>;

pub fn native_event_stream(mut source: EventSource) -> LlmEventStream {
    Box::pin(async_stream::stream! {
        while let Some(event) = source.next().await {
            match event {
                Ok(ReqwestEvent::Open) => yield Ok(LlmStreamEvent::Open),
                Ok(ReqwestEvent::Message(message)) => yield Ok(LlmStreamEvent::Message {
                    event: message.event,
                    data: message.data,
                }),
                Err(reqwest_eventsource::Error::StreamEnded) => {
                    yield Err(LlmStreamError::Ended);
                    return;
                }
                Err(reqwest_eventsource::Error::InvalidStatusCode(status, response)) => {
                    yield Err(LlmStreamError::InvalidStatus(status, response));
                    return;
                }
                Err(_) => {
                    yield Err(LlmStreamError::Connection);
                    return;
                }
            }
        }
        yield Err(LlmStreamError::Ended);
    })
}

pub fn plugin_event_stream(mut source: ProviderEventStream) -> LlmEventStream {
    Box::pin(async_stream::stream! {
        yield Ok(LlmStreamEvent::Open);
        while let Some(event) = source.next().await {
            match event {
                Ok(event) => match serde_json::to_string(&event) {
                    Ok(data) => yield Ok(LlmStreamEvent::Message {
                        event: "provider_event".to_string(),
                        data,
                    }),
                    Err(error) => {
                        yield Err(LlmStreamError::Provider(ProviderError::ResponseMapping(
                            error.to_string(),
                        )));
                        return;
                    }
                },
                Err(error) => {
                    yield Err(LlmStreamError::Provider(error));
                    return;
                }
            }
        }
        yield Err(LlmStreamError::Ended);
    })
}

pub fn provider_error_class(error: &ProviderError) -> &'static str {
    match error {
        ProviderError::InvalidManifest(_) => "invalid_manifest",
        ProviderError::Configuration(_) => "configuration",
        ProviderError::MissingCredential(_) => "missing_credential",
        ProviderError::UnsupportedCapability(_) => "unsupported_capability",
        ProviderError::PayloadMapping(_) => "payload_mapping",
        ProviderError::ResponseMapping(_) => "response_mapping",
        ProviderError::UrlNotAllowed(_) => "url_not_allowed",
        ProviderError::HeaderNotAllowed(_) => "header_not_allowed",
        ProviderError::Transport(_) => "transport",
        ProviderError::HttpStatus { .. } => "http_status",
        ProviderError::QuotaExhausted => "quota_exhausted",
        ProviderError::PaymentRequired => "payment_required",
        ProviderError::StreamEnded => "stream_ended",
        ProviderError::Cancelled => "cancelled",
        ProviderError::ResponseTooLarge => "response_too_large",
    }
}

pub fn build_plugin_chat_request(
    model: impl Into<String>,
    prompts: Vec<Prompt>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    tools: &HashMap<String, McpToolDescriptor>,
    options: Value,
) -> ChatRequest {
    let mut tool_definitions = tools
        .values()
        .map(|tool| ToolDefinition {
            name: tool.openai_name.clone(),
            description: tool.description.clone(),
            parameters: tool.input_schema.clone(),
        })
        .collect::<Vec<_>>();
    tool_definitions.sort_by(|left, right| left.name.cmp(&right.name));

    ChatRequest {
        model: ModelId::new(model),
        messages: prompts.into_iter().map(prompt_to_message).collect(),
        temperature,
        max_tokens,
        tool_choice: (!tool_definitions.is_empty()).then_some(ToolChoice::Auto),
        tools: tool_definitions,
        options,
    }
}

fn prompt_to_message(prompt: Prompt) -> ChatMessage {
    let mut content = Vec::with_capacity(prompt.files.len() + 1);
    if !prompt.text.is_empty() {
        content.push(ContentPart::Text { text: prompt.text });
    }
    for file in prompt.files {
        let Some(data) = file.base64 else {
            continue;
        };
        if file.content_type.starts_with("image/") {
            content.push(ContentPart::ImageBase64 {
                data,
                media_type: file.content_type,
            });
        } else {
            content.push(ContentPart::File {
                name: file.name,
                data,
                media_type: file.content_type,
            });
        }
    }
    ChatMessage {
        role: match prompt.role {
            ChatRole::System => grengin_provider::ChatRole::System,
            ChatRole::User => grengin_provider::ChatRole::User,
            ChatRole::Assistant => grengin_provider::ChatRole::Assistant,
            ChatRole::Tool => grengin_provider::ChatRole::Tool,
        },
        content,
        tool_calls: Vec::new(),
        tool_result: None,
    }
}

#[derive(Default)]
pub struct PluginStreamParser {
    tools: std::sync::Mutex<HashMap<ToolCallId, PendingToolCall>>,
}

struct PendingToolCall {
    name: String,
    index: u32,
    arguments: String,
}

impl PluginStreamParser {
    pub fn new() -> Self {
        Self::default()
    }

    fn provider_event(&self, event: ProviderEvent) -> StreamParseResult {
        match event {
            ProviderEvent::MessageStart { request_id } => request_id
                .map(|request_id| request_id.to_string())
                .map(|request_id| StreamParseResult::MessageStart {
                    request_id,
                    input_tokens: None,
                    output_tokens: None,
                    cached_input_tokens: None,
                    cache_creation_tokens: None,
                })
                .unwrap_or(StreamParseResult::None),
            ProviderEvent::TextDelta { text } => StreamParseResult::TextDelta {
                text,
                request_id: None,
            },
            ProviderEvent::ReasoningDelta { text } => StreamParseResult::EventLog {
                event_type: "thinking_delta".to_string(),
                message: Some(text),
                data: None,
            },
            ProviderEvent::ToolCallStart { id, name, index } => {
                let Ok(mut tools) = self.tools.lock() else {
                    return provider_parse_error("plugin tool state is unavailable");
                };
                tools.insert(
                    id,
                    PendingToolCall {
                        name,
                        index,
                        arguments: String::new(),
                    },
                );
                StreamParseResult::None
            }
            ProviderEvent::ToolArgumentsDelta { id, fragment } => {
                let Ok(mut tools) = self.tools.lock() else {
                    return provider_parse_error("plugin tool state is unavailable");
                };
                let Some(tool) = tools.get_mut(&id) else {
                    return provider_parse_error("tool arguments arrived before tool start");
                };
                tool.arguments.push_str(&fragment);
                StreamParseResult::ToolInput(build_tool_input_delta(
                    fragment,
                    Some(tool.index),
                    Some(tool.name.clone()),
                    Some(id.to_string()),
                    None,
                ))
            }
            ProviderEvent::ToolCallEnd { id } => {
                let Ok(mut tools) = self.tools.lock() else {
                    return provider_parse_error("plugin tool state is unavailable");
                };
                let Some(tool) = tools.remove(&id) else {
                    return provider_parse_error("tool end arrived before tool start");
                };
                let input = if tool.arguments.trim().is_empty() {
                    Value::Object(Default::default())
                } else {
                    match serde_json::from_str(&tool.arguments) {
                        Ok(value) => value,
                        Err(_) => return provider_parse_error("tool arguments are not valid JSON"),
                    }
                };
                StreamParseResult::ToolCall(ToolCall {
                    tool_name: tool.name,
                    tool_id: Some(id.to_string()),
                    input: Some(ToolInput::Json(input)),
                    index: Some(tool.index),
                    raw: None,
                    web_search: None,
                })
            }
            ProviderEvent::ServerToolStart {
                id,
                name,
                query,
                queries,
            } => StreamParseResult::WebSearchAction {
                tool_name: name,
                tool_id: id.map(|id| id.to_string()),
                query,
                queries: (!queries.is_empty()).then_some(queries),
            },
            // The provider streams its search query in fragments; forward them the same way client
            // tool input is forwarded so the UI can show the query as it forms.
            ProviderEvent::ServerToolQueryDelta { id, name, fragment } => {
                StreamParseResult::ToolInput(build_tool_input_delta(
                    fragment,
                    None,
                    Some(name),
                    id.map(|id| id.to_string()),
                    None,
                ))
            }
            ProviderEvent::ServerToolResult { id, name, results } => {
                StreamParseResult::WebSearchResult {
                    tool_name: name,
                    tool_id: id.map(|id| id.to_string()),
                    results: results
                        .into_iter()
                        .map(|result| StreamWebSearchResult {
                            title: result.title,
                            url: result.url,
                            source: result.source,
                            page_age: result.page_age,
                            snippet: result.snippet,
                        })
                        .collect(),
                }
            }
            ProviderEvent::Usage { usage } => usage_result(usage),
            ProviderEvent::ProviderEvent { kind, data } => StreamParseResult::EventLog {
                event_type: kind,
                message: None,
                data: Some(data),
            },
            ProviderEvent::Error { kind, message } => StreamParseResult::Error {
                kind: match kind {
                    ProviderEventErrorKind::QuotaExhausted => StreamErrorKind::QuotaExhausted,
                    ProviderEventErrorKind::Provider => StreamErrorKind::ProviderError,
                },
                message,
            },
            ProviderEvent::Completed { .. } => StreamParseResult::None,
        }
    }
}

impl StreamParser for PluginStreamParser {
    fn parse_event(&self, data: &str) -> StreamParseResult {
        if data.is_empty() {
            return StreamParseResult::None;
        }
        match serde_json::from_str::<ProviderEvent>(data) {
            Ok(event) => self.provider_event(event),
            Err(_) => provider_parse_error("provider plugin emitted an invalid event"),
        }
    }
}

fn usage_result(usage: TokenUsage) -> StreamParseResult {
    StreamParseResult::TokenUsage {
        request_id: None,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        total_tokens: usage.total_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        cache_creation_tokens: usage.cache_creation_tokens,
    }
}

fn provider_parse_error(message: &str) -> StreamParseResult {
    StreamParseResult::Error {
        kind: StreamErrorKind::ProviderError,
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use grengin_provider::{ProviderEvent, ToolCallId};

    use super::*;

    #[test]
    fn plugin_tool_deltas_become_one_typed_call() {
        let parser = PluginStreamParser::new();
        let id = ToolCallId::new("call-1");
        assert!(matches!(
            parser.provider_event(ProviderEvent::ToolCallStart {
                id: id.clone(),
                name: "weather".to_string(),
                index: 0,
            }),
            StreamParseResult::None
        ));
        assert!(matches!(
            parser.provider_event(ProviderEvent::ToolArgumentsDelta {
                id: id.clone(),
                fragment: "{\"city\":".to_string(),
            }),
            StreamParseResult::ToolInput(_)
        ));
        let _ = parser.provider_event(ProviderEvent::ToolArgumentsDelta {
            id: id.clone(),
            fragment: "\"Paris\"}".to_string(),
        });
        let StreamParseResult::ToolCall(call) =
            parser.provider_event(ProviderEvent::ToolCallEnd { id })
        else {
            panic!("expected a completed tool call");
        };
        assert_eq!(call.tool_name, "weather");
        assert_eq!(
            call.input.and_then(|input| input.as_json().cloned()),
            Some(serde_json::json!({"city": "Paris"}))
        );
    }

    #[test]
    fn malformed_tool_arguments_are_rejected() {
        let parser = PluginStreamParser::new();
        let id = ToolCallId::new("call-1");
        let _ = parser.provider_event(ProviderEvent::ToolCallStart {
            id: id.clone(),
            name: "weather".to_string(),
            index: 0,
        });
        let _ = parser.provider_event(ProviderEvent::ToolArgumentsDelta {
            id: id.clone(),
            fragment: "{".to_string(),
        });
        assert!(matches!(
            parser.provider_event(ProviderEvent::ToolCallEnd { id }),
            StreamParseResult::Error { .. }
        ));
    }
}
