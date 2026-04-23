use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;

use super::{
    build_tool_call, build_tool_input_delta, parse_web_search_action, StreamParseResult,
    StreamParser, StreamWebSearchResult, ToolInput,
};

#[derive(Debug, Clone)]
struct ToolBuffer {
    name: Option<String>,
    buffer: String,
}

pub struct MistralConversationStreamParser {
    tool_buffers: Mutex<HashMap<String, ToolBuffer>>,
}

impl MistralConversationStreamParser {
    pub fn new() -> Self {
        Self {
            tool_buffers: Mutex::new(HashMap::new()),
        }
    }

    fn parse_event_wrapper(&self, value: &Value) -> Option<(String, Value)> {
        if let Some(event) = value.get("event").and_then(|v| v.as_str()) {
            let payload = value.get("data").cloned().unwrap_or(Value::Null);
            return Some((event.to_string(), payload));
        }
        if let Some(event) = value.get("type").and_then(|v| v.as_str()) {
            return Some((event.to_string(), value.clone()));
        }
        None
    }

    fn parse_message_output_delta(
        &self,
        payload: &Value,
        request_id: Option<String>,
    ) -> Option<StreamParseResult> {
        let content = payload.get("content")?;
        let mut collected_text = String::new();
        let mut references: Vec<StreamWebSearchResult> = Vec::new();

        let mut parse_chunk = |item: &Value| {
            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                if !text.is_empty() {
                    collected_text.push_str(text);
                }
            }
            if item.get("type").and_then(|v| v.as_str()) == Some("tool_reference") {
                let title = item
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let url = item
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let source = item
                    .get("tool")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let snippet = item
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                if !title.is_empty() || !url.is_empty() {
                    references.push(StreamWebSearchResult {
                        title,
                        url,
                        source,
                        page_age: None,
                        snippet,
                    });
                }
            }
        };

        if let Some(text) = content.as_str() {
            if !text.is_empty() {
                collected_text.push_str(text);
            }
        } else if let Some(items) = content.as_array() {
            for item in items {
                parse_chunk(item);
            }
        } else if content.is_object() {
            parse_chunk(content);
        }

        if !references.is_empty() {
            return Some(StreamParseResult::WebSearchResult {
                tool_name: "web_search_call".to_string(),
                tool_id: None,
                results: references,
            });
        }
        if !collected_text.is_empty() {
            return Some(StreamParseResult::TextDelta {
                text: collected_text,
                request_id,
            });
        }
        None
    }

    fn parse_tool_execution(&self, payload: &Value, is_done: bool) -> Option<StreamParseResult> {
        let name = payload
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("tool_call")
            .to_string();
        let tool_id = payload
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let arguments = payload
            .get("arguments")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if is_done {
            let mut output = payload.get("info").cloned().unwrap_or(Value::Null);
            if output.is_null() || output == Value::Object(Default::default()) {
                if let Some(value) = payload.get("output") {
                    output = value.clone();
                } else if let Some(value) = payload.get("result") {
                    output = value.clone();
                }
            }
            if output.is_null() || output == Value::Object(Default::default()) {
                output = payload.clone();
            }
            if name.contains("web_search") {
                let results = output
                    .get("results")
                    .and_then(|v| v.as_array())
                    .or_else(|| payload.get("results").and_then(|v| v.as_array()))
                    .or_else(|| {
                        payload
                            .get("result")
                            .and_then(|v| v.get("results"))
                            .and_then(|v| v.as_array())
                    })
                    .or_else(|| {
                        payload
                            .get("output")
                            .and_then(|v| v.get("results"))
                            .and_then(|v| v.as_array())
                    });
                if let Some(results) = results {
                    let mapped = results
                        .iter()
                        .filter_map(|item| {
                            let title = item
                                .get("title")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let url = item
                                .get("url")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let source = item
                                .get("source")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            let snippet = item
                                .get("snippet")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            if title.is_empty() && url.is_empty() {
                                None
                            } else {
                                Some(StreamWebSearchResult {
                                    title,
                                    url,
                                    source,
                                    page_age: None,
                                    snippet,
                                })
                            }
                        })
                        .collect::<Vec<_>>();
                    if !mapped.is_empty() {
                        return Some(StreamParseResult::WebSearchResult {
                            tool_name: "web_search_call".to_string(),
                            tool_id: tool_id.clone(),
                            results: mapped,
                        });
                    }
                }
            }
            let result = super::ToolResult {
                tool_name: Some(name),
                tool_id,
                output: Some(output),
                index: None,
                raw: Some(payload.clone()),
            };
            return Some(StreamParseResult::ToolResult(result));
        }
        if !arguments.is_empty() {
            let web_search = serde_json::from_str::<Value>(&arguments)
                .ok()
                .and_then(|value| parse_web_search_action(&value));
            let delta = build_tool_input_delta(arguments, None, Some(name), tool_id, web_search);
            return Some(StreamParseResult::ToolInput(delta));
        }
        None
    }

    fn parse_function_call_delta(&self, payload: &Value) -> Option<StreamParseResult> {
        let tool_call_id = payload
            .get("tool_call_id")
            .and_then(|v| v.as_str())?
            .to_string();
        let name = payload
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let arguments = payload
            .get("arguments")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if let Ok(mut buffers) = self.tool_buffers.lock() {
            let entry = buffers
                .entry(tool_call_id.clone())
                .or_insert_with(|| ToolBuffer {
                    name: name.clone(),
                    buffer: String::new(),
                });
            if entry.name.is_none() && name.is_some() {
                entry.name = name.clone();
            }
            if !arguments.is_empty() {
                entry.buffer.push_str(&arguments);
            }
            let candidate = entry.buffer.clone();
            if let Ok(json) = serde_json::from_str::<Value>(&candidate) {
                let tool_name = entry
                    .name
                    .clone()
                    .unwrap_or_else(|| "tool_call".to_string());
                let call = build_tool_call(
                    tool_name,
                    Some(tool_call_id),
                    Some(ToolInput::Json(json)),
                    None,
                    Some(payload.clone()),
                );
                return Some(StreamParseResult::ToolCall(call));
            }
            if !arguments.is_empty() {
                let delta = build_tool_input_delta(
                    arguments,
                    None,
                    entry.name.clone(),
                    Some(tool_call_id),
                    None,
                );
                return Some(StreamParseResult::ToolInput(delta));
            }
        }
        None
    }
}

impl Default for MistralConversationStreamParser {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamParser for MistralConversationStreamParser {
    fn parse_event(&self, data: &str) -> StreamParseResult {
        let value = match serde_json::from_str::<Value>(data) {
            Ok(v) => v,
            Err(_) => return StreamParseResult::None,
        };
        let Some((event_type, payload)) = self.parse_event_wrapper(&value) else {
            return StreamParseResult::None;
        };

        match event_type.as_str() {
            "message.output.delta" => {
                return self
                    .parse_message_output_delta(&payload, None)
                    .unwrap_or(StreamParseResult::None);
            }
            "tool.execution.started" | "tool.execution.delta" => {
                return self
                    .parse_tool_execution(&payload, false)
                    .unwrap_or(StreamParseResult::None);
            }
            "tool.execution.done" => {
                return self
                    .parse_tool_execution(&payload, true)
                    .unwrap_or(StreamParseResult::None);
            }
            "function.call.delta" => {
                return self
                    .parse_function_call_delta(&payload)
                    .unwrap_or(StreamParseResult::None);
            }
            "conversation.response.done" => {
                if let Some(usage) = payload.get("usage") {
                    let input_tokens = usage
                        .get("prompt_tokens")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32);
                    let output_tokens = usage
                        .get("completion_tokens")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32);
                    let total_tokens = usage
                        .get("total_tokens")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32);
                    return StreamParseResult::TokenUsage {
                        request_id: None,
                        input_tokens,
                        output_tokens,
                        total_tokens,
                    };
                }
            }
            "conversation.response.error" => {
                let message = payload
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("mistral error")
                    .to_string();
                let code = payload
                    .get("code")
                    .and_then(|v| v.as_i64())
                    .map(|v| v.to_string())
                    .unwrap_or("mistral_error".to_string());
                return StreamParseResult::Error {
                    error_type: code,
                    message,
                };
            }
            _ => {}
        }

        StreamParseResult::None
    }
}
