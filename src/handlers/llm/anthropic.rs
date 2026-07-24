use std::collections::HashMap;
use std::sync::Mutex;
use anyhow::Error;
use reqwest::Client as ReqwestClient;
use reqwest_eventsource::EventSource;
use serde_json::Value;

use super::{
    StreamErrorKind, StreamParseResult, StreamParser, StreamWebSearchAction, StreamWebSearchResult,
    ToolInput, build_tool_call, build_tool_input_delta, parse_web_search_action,
    tool_name_is_web_search, CONV_SEARCH_TOOL_NAME,
};
use crate::{
    config::setting::AnthropicSettings,
    dto::llm::anthropic::{
        AnthropicContentBlock, AnthropicContentBlockResponse, AnthropicDelta, AnthropicMessage,
        AnthropicRole, AnthropicStreamEvent, AnthropicTool, AnthropicToolUnion,
        AnthropicWebSearchTool,
    },
    llm::{prompt::Prompt, provider::AnthropicApis},
    services::mcp_tools::McpToolDescriptor,
};

/// Anthropic stream parser
pub struct AnthropicStreamParser {
    tool_calls: Mutex<HashMap<u32, (String, Option<String>)>>,
    tool_input_buffers: Mutex<HashMap<u32, String>>,
}

impl AnthropicStreamParser {
    pub fn new() -> Self {
        Self {
            tool_calls: Mutex::new(HashMap::new()),
            tool_input_buffers: Mutex::new(HashMap::new()),
        }
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

                    let input_tokens = u64_to_u32(
                        v.pointer("/message/usage/input_tokens")
                            .and_then(|x| x.as_u64()),
                    );
                    let output_tokens = u64_to_u32(
                        v.pointer("/message/usage/output_tokens")
                            .and_then(|x| x.as_u64()),
                    );
                    let cache_creation_tokens = u64_to_u32(
                        v.pointer("/message/usage/cache_creation_input_tokens")
                            .and_then(|x| x.as_u64()),
                    );
                    let cached_input_tokens = u64_to_u32(
                        v.pointer("/message/usage/cache_read_input_tokens")
                            .and_then(|x| x.as_u64()),
                    );

                    return StreamParseResult::MessageStart {
                        request_id,
                        input_tokens,
                        output_tokens,
                        cached_input_tokens,
                        cache_creation_tokens,
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
                            cached_input_tokens: None,
                            cache_creation_tokens: None,
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
                    cached_input_tokens: None,
                    cache_creation_tokens: None,
                },

                AnthropicStreamEvent::ContentBlockStart {
                    index,
                    content_block,
                } => match content_block {
                    AnthropicContentBlockResponse::ToolUse { id, name, input } => {
                        if let Ok(mut calls) = self.tool_calls.lock() {
                            calls.insert(index, (name.clone(), Some(id.clone())));
                        }
                        let call = build_tool_call(
                            name,
                            Some(id),
                            Some(ToolInput::Json(input)),
                            Some(index),
                            None,
                        );
                        StreamParseResult::ToolCall(call)
                    }
                    AnthropicContentBlockResponse::ServerToolUse { id, name, input } => {
                        if let Ok(mut calls) = self.tool_calls.lock() {
                            calls.insert(index, (name.clone(), Some(id.clone())));
                        }
                        let call = build_tool_call(
                            name,
                            Some(id),
                            Some(ToolInput::Json(input)),
                            Some(index),
                            None,
                        );
                        StreamParseResult::ToolCall(call)
                    }
                    AnthropicContentBlockResponse::WebSearchToolResult {
                        tool_use_id,
                        content,
                    } => {
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
                        let (tool_name, tool_id) = self
                            .tool_calls
                            .lock()
                            .ok()
                            .and_then(|calls| calls.get(&index).cloned())
                            .unwrap_or((String::new(), None));

                        let mut web_search: Option<StreamWebSearchAction> = None;
                        if !tool_name.is_empty() && tool_name_is_web_search(&tool_name) {
                            if let Ok(mut buffers) = self.tool_input_buffers.lock() {
                                let buffer = buffers.entry(index).or_default();
                                buffer.push_str(&partial_json);
                                if let Ok(value) = serde_json::from_str::<serde_json::Value>(buffer)
                                {
                                    web_search = parse_web_search_action(&value);
                                }
                            }
                        }

                        let delta = build_tool_input_delta(
                            partial_json,
                            Some(index),
                            if tool_name.is_empty() {
                                None
                            } else {
                                Some(tool_name)
                            },
                            tool_id,
                            web_search,
                        );
                        StreamParseResult::ToolInput(delta)
                    }
                },
                AnthropicStreamEvent::ContentBlockStop { index } => {
                    if let Ok(mut buffers) = self.tool_input_buffers.lock() {
                        buffers.remove(&index);
                    }
                    StreamParseResult::None
                }

                AnthropicStreamEvent::Error { error } => StreamParseResult::Error {
                    kind: StreamErrorKind::from_provider_str(&error.error_type),
                    message: error.message,
                },

                AnthropicStreamEvent::MessageStop
                | AnthropicStreamEvent::MessageDelta { .. }
                | AnthropicStreamEvent::Ping => StreamParseResult::None,
            },
            Err(_) => StreamParseResult::None,
        }
    }
}

pub fn build_anthropic_tools(
    web_search: bool,
    mcp_tool_lookup: &HashMap<String, McpToolDescriptor>,
) -> Option<Vec<AnthropicToolUnion>> {
    let mut tools = Vec::new();
    if web_search {
        tools.push(AnthropicToolUnion::WebSearchTool(AnthropicWebSearchTool::new(Some(5))));
    }
    tools.push(AnthropicToolUnion::ClientTool(AnthropicTool {
        name: CONV_SEARCH_TOOL_NAME.to_string(),
        description: "Search the user's past conversations by keyword. Use when the user asks about previous chats or wants to find something from conversation history.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Keywords to search for in past conversations" }
            },
            "required": ["query"]
        }),
    }));
    for descriptor in mcp_tool_lookup.values() {
        let description = descriptor
            .description
            .clone()
            .unwrap_or_else(|| descriptor.original_name.clone());
        tools.push(AnthropicToolUnion::ClientTool(AnthropicTool {
            name: descriptor.openai_name.clone(),
            description,
            input_schema: descriptor.input_schema.clone(),
        }));
    }
    if tools.is_empty() { None } else { Some(tools) }
}

pub async fn continue_anthropic_stream(
    client: &ReqwestClient,
    settings: &AnthropicSettings,
    model_name: String,
    max_tokens: i32,
    temperature: Option<f32>,
    messages: Vec<AnthropicMessage>,
    system_prompt: Option<String>,
    tools: Option<Vec<AnthropicToolUnion>>,
) -> Result<EventSource, Error> {
    client
        .anthropic_chat_stream_with_messages(
            settings,
            model_name,
            max_tokens,
            temperature,
            messages,
            system_prompt,
            tools,
        )
        .await
}

pub fn make_anthropic_tool_blocks(
    call_id: String,
    tool_name: String,
    args: Value,
    output: &Value,
    is_error: bool,
) -> (AnthropicContentBlock, AnthropicContentBlock) {
    (
        AnthropicContentBlock::ToolUse {
            id: call_id.clone(),
            name: tool_name,
            input: args,
        },
        AnthropicContentBlock::ToolResult {
            tool_use_id: call_id,
            content: serde_json::to_string(output).unwrap_or_else(|_| "{}".to_string()),
            is_error: Some(is_error),
        },
    )
}

pub fn build_anthropic_continuation(
    existing: Option<Vec<AnthropicMessage>>,
    base_prompts: Vec<Prompt>,
    tool_use_blocks: Vec<AnthropicContentBlock>,
    tool_result_blocks: Vec<AnthropicContentBlock>,
) -> (Vec<AnthropicMessage>, Option<String>) {
    let (mut messages, system_prompt) = if let Some(m) = existing {
        (m, None)
    } else {
        AnthropicMessage::from_prompts(base_prompts)
    };
    if !tool_use_blocks.is_empty() {
        messages.push(AnthropicMessage::with_blocks(AnthropicRole::Assistant, tool_use_blocks));
    }
    messages.push(AnthropicMessage::with_blocks(AnthropicRole::User, tool_result_blocks));
    (messages, system_prompt)
}
