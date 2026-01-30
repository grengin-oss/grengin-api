use crate::dto::llm::openai::{OpenaiResponseStreamEvent, OpenaiChatCompletionChunk};
use super::{StreamParser, StreamParseResult, StreamWebSearchResult};
use serde_json::Value;

/// OpenAI stream parser
pub struct OpenaiStreamParser;

impl OpenaiStreamParser {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OpenaiStreamParser {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamParser for OpenaiStreamParser {
    fn parse_event(&self, data: &str) -> StreamParseResult {
        // 1) Prefer typed Responses-API events (response.output_text.delta etc.)
        
        if let Ok(stream_event) = serde_json::from_str::<OpenaiResponseStreamEvent>(data) {
            match stream_event {
                OpenaiResponseStreamEvent::OutputTextDelta(delta) => {
                    return StreamParseResult::TextDelta {
                        text: delta.delta,
                        request_id: Some(delta.item_id),
                    };
                }

                // If your DTO includes ResponseCompleted with usage, emit usage here.
                // If not, we still handle it via the raw JSON fallback below.
                OpenaiResponseStreamEvent::ResponseCompleted(ev) => {
                    if let Some(usage) = ev.response.usage.clone() {
                        return StreamParseResult::TokenUsage {
                            request_id: Some(ev.response.id),
                            input_tokens: Some(usage.input_tokens),
                            output_tokens: Some(usage.output_tokens),
                            total_tokens: Some(usage.total_tokens),
                        };
                    }
                },

                OpenaiResponseStreamEvent::ResponseCreated(ev) => {
                        return StreamParseResult::MessageStart {
                            request_id:ev.response.id,
                            input_tokens:ev.response.usage.as_ref().map(|usage| usage.input_tokens),
                            output_tokens:ev.response.usage.as_ref().map(|usage| usage.output_tokens),
                        };
                },

                OpenaiResponseStreamEvent::Error(ev) => {
                     return StreamParseResult::Error {
                        error_type:ev.error.error_type.unwrap_or("openai_error".into()),
                        message:ev.error.message.unwrap_or("openai.error.message".into()),
                    };
                }

                _ => {}
            }
        }
        // 3) Chat Completions streaming: parse chunk (usage shows up on the final chunk if enabled)
        if let Ok(chunk) = serde_json::from_str::<OpenaiChatCompletionChunk>(data) {
            // usage chunk
            if let Some(usage) = chunk.usage {
                return StreamParseResult::TokenUsage {
                    request_id: Some(chunk.id),
                    input_tokens: Some(usage.prompt_tokens),
                    output_tokens: Some(usage.completion_tokens),
                    total_tokens: Some(usage.total_tokens),
                };
            }

            // delta text (for chat.completion chunks)
            if let Some(choice) = chunk.choices.get(0) {
                if let Some(text) = choice.delta.content.clone() {
                    return StreamParseResult::TextDelta {
                        text,
                        request_id: Some(chunk.id),
                    };
                }
            }
        }

        if let Ok(v) = serde_json::from_str::<Value>(data) {
            if let Some(event) = extract_openai_event_log(&v) {
                return event;
            }
            if let Some(event) = extract_openai_tool_event(&v) {
                return event;
            }
        }

        StreamParseResult::None
    }
}

fn extract_openai_event_log(v: &Value) -> Option<StreamParseResult> {
    let event_type = v.get("type").and_then(|t| t.as_str())?;
    let is_reasoning = event_type.contains("reasoning")
        || event_type.contains("thinking")
        || event_type.contains("thought");
    if !is_reasoning {
        return None;
    }
    let message = v.get("delta")
        .and_then(|d| d.as_str())
        .or_else(|| v.get("text").and_then(|d| d.as_str()))
        .or_else(|| v.get("thinking").and_then(|d| d.as_str()))
        .map(|s| s.to_string());
    let data = if message.is_some() { None } else { Some(v.clone()) };
    Some(StreamParseResult::EventLog {
        event_type: event_type.to_string(),
        message,
        data,
    })
}

fn extract_openai_tool_event(v: &Value) -> Option<StreamParseResult> {
    let event_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let is_toolish = event_type.contains("tool")
        || event_type.contains("web_search")
        || event_type.contains("function");

    if let Some(item) = v.get("item") {
        if let Some(event) = parse_openai_tool_item(item, Some(event_type), v) {
            return Some(event);
        }
    }

    if event_type == "response.output_text.annotation.added" {
        if let Some(annotation) = v.get("annotation") {
            if let Some(result) = parse_openai_url_citation(annotation) {
                return Some(StreamParseResult::WebSearchResult {
                    tool_name: "web_search_call".to_string(),
                    tool_id: None,
                    results: vec![result],
                });
            }
        }
    }

    if event_type == "response.completed" {
        if let Some(output) = v.pointer("/response/output").and_then(|o| o.as_array()) {
            for item in output {
                if let Some(event) = parse_openai_tool_item(item, Some(event_type), v) {
                    return Some(event);
                }
            }
        }
    }

    if is_toolish {
        if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
            let tool_name = v.get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("tool_call")
                .to_string();
            return Some(StreamParseResult::ToolCall {
                tool_name,
                tool_id: v.get("id").and_then(|id| id.as_str()).map(|s| s.to_string()),
                input: Some(Value::String(delta.to_string())),
                index: None,
                raw: Some(v.clone()),
            });
        }
    }

    None
}

fn parse_openai_tool_item(
    item: &Value,
    event_type: Option<&str>,
    raw: &Value,
) -> Option<StreamParseResult> {
    let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let is_toolish = item_type.contains("tool")
        || item_type.contains("web_search")
        || item_type.contains("function")
        || item_type.contains("call");
    if !is_toolish && event_type.map(|t| t.contains("tool")).unwrap_or(false) == false {
        return None;
    }

    let tool_name = item
        .get("name")
        .and_then(|n| n.as_str())
        .or_else(|| item.get("tool_name").and_then(|n| n.as_str()))
        .or_else(|| if item_type.is_empty() { None } else { Some(item_type) })
        .unwrap_or("tool_call")
        .to_string();
    let tool_id = item.get("id").and_then(|id| id.as_str()).map(|s| s.to_string());
    let input = item.get("input")
        .cloned()
        .or_else(|| item.get("arguments").cloned())
        .or_else(|| item.get("call").cloned());
    let output = item.get("output")
        .cloned()
        .or_else(|| item.get("result").cloned())
        .or_else(|| item.get("response").cloned())
        .or_else(|| item.get("results").cloned())
        .or_else(|| item.get("content").cloned())
        .or_else(|| item.get("output").and_then(|o| o.get("results")).cloned());
    let action = item.get("action").cloned();
    let status = item.get("status").and_then(|s| s.as_str());

    if output.is_some() {
        return Some(StreamParseResult::ToolResult {
            tool_name: Some(tool_name),
            tool_id,
            output,
            index: None,
            raw: Some(raw.clone()),
        });
    }

    if action.is_some() && tool_name.contains("web_search") {
        let query = action
            .as_ref()
            .and_then(|a| a.get("query"))
            .and_then(|q| q.as_str())
            .map(|s| s.to_string());
        let queries = action
            .as_ref()
            .and_then(|a| a.get("queries"))
            .and_then(|q| q.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<String>>()
            });
        return Some(StreamParseResult::WebSearchAction {
            tool_name,
            tool_id,
            query,
            queries,
        });
    }

    if action.is_some() && status.is_some() {
        let mut payload = serde_json::Map::new();
        if let Some(action) = action {
            payload.insert("action".to_string(), action);
        }
        if let Some(status) = item.get("status") {
            payload.insert("status".to_string(), status.clone());
        }
        return Some(StreamParseResult::ToolResult {
            tool_name: Some(tool_name),
            tool_id,
            output: Some(serde_json::Value::Object(payload)),
            index: None,
            raw: Some(raw.clone()),
        });
    }

    Some(StreamParseResult::ToolCall {
        tool_name,
        tool_id,
        input,
        index: None,
        raw: Some(raw.clone()),
    })
}

fn parse_openai_url_citation(annotation: &Value) -> Option<StreamWebSearchResult> {
    let annotation_type = annotation.get("type").and_then(|t| t.as_str())?;
    if annotation_type != "url_citation" {
        return None;
    }
    let title = annotation.get("title").and_then(|t| t.as_str())?.to_string();
    let url = annotation.get("url").and_then(|u| u.as_str())?.to_string();
    Some(StreamWebSearchResult {
        title,
        url,
        source: None,
        page_age: None,
        snippet: None,
    })
}
