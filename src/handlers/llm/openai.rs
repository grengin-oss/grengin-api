use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::Error;
use reqwest::Client as ReqwestClient;
use reqwest_eventsource::EventSource;
use serde_json::Value;
use uuid::Uuid;

use super::{
    StreamErrorKind, StreamParseResult, StreamParser, StreamWebSearchResult, ToolInput, ToolResult,
    build_tool_call, build_tool_input_delta, parse_web_search_action, tool_name_is_web_search,
};
use crate::{
    config::setting::OpenaiSettings,
    dto::llm::openai::{
        OpenaiChatCompletionChunk, OpenaiFunctionCallItem, OpenaiFunctionCallOutput,
        OpenaiInputItem, OpenaiResponseOutputItem, OpenaiResponseStreamEvent, OpenaiTool,
        OpenaiWebSearchAction,
    },
    llm::provider::OpenaiApis,
    services::artifacts::{ARTIFACT_TOOL_DESC, ARTIFACT_TOOL_NAME},
};

#[derive(Debug, Clone)]
struct OpenaiToolCallMeta {
    name: String,
    call_id: Option<String>,
}

/// OpenAI stream parser
pub struct OpenaiStreamParser {
    tool_calls: Mutex<HashMap<String, OpenaiToolCallMeta>>, // item_id -> meta
}

impl OpenaiStreamParser {
    pub fn new() -> Self {
        Self {
            tool_calls: Mutex::new(HashMap::new()),
        }
    }

    fn record_function_call_meta(&self, item: &OpenaiFunctionCallItem) {
        if let Ok(mut calls) = self.tool_calls.lock() {
            calls.insert(
                item.id.clone(),
                OpenaiToolCallMeta {
                    name: item.name.clone(),
                    call_id: item.call_id.clone(),
                },
            );
        }
    }

    fn get_tool_meta(&self, item_id: &str) -> (Option<String>, Option<String>) {
        self.tool_calls
            .lock()
            .ok()
            .and_then(|calls| calls.get(item_id).cloned())
            .map(|meta| (Some(meta.name), meta.call_id))
            .unwrap_or((None, None))
    }

    fn take_tool_meta(&self, item_id: &str) -> (Option<String>, Option<String>) {
        self.tool_calls
            .lock()
            .ok()
            .and_then(|mut calls| calls.remove(item_id))
            .map(|meta| (Some(meta.name), meta.call_id))
            .unwrap_or((None, None))
    }

    fn handle_response_stream_event(
        &self,
        event: OpenaiResponseStreamEvent,
        raw: &Value,
    ) -> Option<StreamParseResult> {
        match event {
            OpenaiResponseStreamEvent::OutputItemAdded(ev) => {
                if let OpenaiResponseOutputItem::FunctionCall(item) = ev.item {
                    self.record_function_call_meta(&item);
                }
                Some(StreamParseResult::None)
            }
            OpenaiResponseStreamEvent::FunctionCallArgumentsDelta(ev) => {
                let (tool_name, tool_id) = self.get_tool_meta(&ev.item_id);
                let delta = build_tool_input_delta(ev.delta, None, tool_name, tool_id, None);
                Some(StreamParseResult::ToolInput(delta))
            }
            OpenaiResponseStreamEvent::FunctionCallArgumentsDone(ev) => {
                let (tool_name, tool_id) = self.take_tool_meta(&ev.item_id);
                if let Some(tool_name) = tool_name {
                    let input = parse_tool_input(ev.arguments);
                    let call = build_tool_call(tool_name, tool_id, input, None, Some(raw.clone()));
                    return Some(StreamParseResult::ToolCall(call));
                }
                None
            }
            OpenaiResponseStreamEvent::OutputTextDelta(delta) => {
                Some(StreamParseResult::TextDelta {
                    text: delta.delta,
                    request_id: Some(delta.item_id),
                })
            }
            OpenaiResponseStreamEvent::OutputTextAnnotationAdded(ev) => {
                parse_openai_url_citation(&ev.annotation).map(|result| {
                    StreamParseResult::WebSearchResult {
                        tool_name: "web_search_call".to_string(),
                        tool_id: None,
                        results: vec![result],
                    }
                })
            }
            OpenaiResponseStreamEvent::ResponseCompleted(ev) => {
                if let Some(event) = extract_openai_tool_event(raw) {
                    return Some(event);
                }
                ev.response
                    .usage
                    .clone()
                    .map(|usage| StreamParseResult::TokenUsage {
                        request_id: Some(ev.response.id),
                        input_tokens: Some(usage.input_tokens),
                        output_tokens: Some(usage.output_tokens),
                        total_tokens: Some(usage.total_tokens),
                    })
            }
            OpenaiResponseStreamEvent::ResponseCreated(ev) => {
                Some(StreamParseResult::MessageStart {
                    request_id: ev.response.id,
                    input_tokens: ev.response.usage.as_ref().map(|usage| usage.input_tokens),
                    output_tokens: ev.response.usage.as_ref().map(|usage| usage.output_tokens),
                })
            }
            OpenaiResponseStreamEvent::Error(ev) => {
                let raw_type = ev.error.error_type.as_deref().unwrap_or("openai_error");
                Some(StreamParseResult::Error {
                    kind: StreamErrorKind::from_provider_str(raw_type),
                    message: ev.error.message.unwrap_or_else(|| "OpenAI stream error".into()),
                })
            }
            _ => None,
        }
    }
}

impl Default for OpenaiStreamParser {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamParser for OpenaiStreamParser {
    fn parse_event(&self, data: &str) -> StreamParseResult {
        let value = match serde_json::from_str::<Value>(data) {
            Ok(v) => v,
            Err(_) => return StreamParseResult::None,
        };

        // 1) Prefer typed Responses-API events (response.output_text.delta etc.)
        if let Ok(stream_event) = serde_json::from_value::<OpenaiResponseStreamEvent>(value.clone())
        {
            if let Some(result) = self.handle_response_stream_event(stream_event, &value) {
                return result;
            }
        }

        // 2) Chat Completions streaming: parse chunk (usage shows up on the final chunk if enabled)
        if let Ok(chunk) = serde_json::from_value::<OpenaiChatCompletionChunk>(value.clone()) {
            if let Some(usage) = chunk.usage {
                return StreamParseResult::TokenUsage {
                    request_id: Some(chunk.id),
                    input_tokens: Some(usage.prompt_tokens),
                    output_tokens: Some(usage.completion_tokens),
                    total_tokens: Some(usage.total_tokens),
                };
            }

            if let Some(choice) = chunk.choices.get(0) {
                if let Some(text) = choice.delta.content.clone() {
                    return StreamParseResult::TextDelta {
                        text,
                        request_id: Some(chunk.id),
                    };
                }
            }
        }

        if let Some(event) = extract_openai_event_log(&value) {
            return event;
        }
        if let Some(event) = extract_openai_tool_event(&value) {
            return event;
        }

        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("OpenAI stream error")
                .to_string();
            let raw_type = error
                .get("type")
                .or_else(|| error.get("code"))
                .and_then(|v| v.as_str())
                .unwrap_or("openai_error");
            return StreamParseResult::Error {
                kind: StreamErrorKind::from_provider_str(raw_type),
                message,
            };
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
    let message = v
        .get("delta")
        .and_then(|d| d.as_str())
        .or_else(|| v.get("text").and_then(|d| d.as_str()))
        .or_else(|| v.get("thinking").and_then(|d| d.as_str()))
        .map(|s| s.to_string());
    let data = if message.is_some() {
        None
    } else {
        Some(v.clone())
    };
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
            let tool_name = v
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("tool_call")
                .to_string();
            let tool_id = v
                .get("call_id")
                .or_else(|| v.get("id"))
                .and_then(|id| id.as_str())
                .map(|s| s.to_string());
            let web_search = if tool_name_is_web_search(&tool_name) {
                serde_json::from_str::<Value>(delta)
                    .ok()
                    .and_then(|value| parse_web_search_action(&value))
            } else {
                None
            };
            let delta = build_tool_input_delta(
                delta.to_string(),
                None,
                Some(tool_name),
                tool_id,
                web_search,
            );
            return Some(StreamParseResult::ToolInput(delta));
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
    let event_toolish = event_type
        .map(|t| t.contains("tool") || t.contains("web_search") || t.contains("function"))
        .unwrap_or(false);
    if !is_toolish && !event_toolish {
        return None;
    }

    let output = extract_tool_output(item);
    if output.is_some() {
        let tool_name = resolve_tool_name(item, item_type);
        let tool_id = resolve_tool_id(item);
        return Some(StreamParseResult::ToolResult(ToolResult {
            tool_name: Some(tool_name),
            tool_id,
            output,
            index: None,
            raw: Some(raw.clone()),
        }));
    }

    if let Ok(typed_item) = serde_json::from_value::<OpenaiResponseOutputItem>(item.clone()) {
        if let Some(event) = parse_openai_typed_tool_item(&typed_item, raw) {
            return Some(event);
        }
    }

    let tool_name = resolve_tool_name(item, item_type);
    let tool_id = resolve_tool_id(item);
    let input = item
        .get("input")
        .cloned()
        .or_else(|| item.get("arguments").cloned())
        .or_else(|| item.get("call").cloned())
        .and_then(parse_tool_input);
    let action = item.get("action").cloned();
    let status = item.get("status").and_then(|s| s.as_str());

    if status == Some("in_progress") && input.is_none() {
        return None;
    }

    if action.is_some() && tool_name_is_web_search(&tool_name) {
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
        return Some(StreamParseResult::ToolResult(ToolResult {
            tool_name: Some(tool_name),
            tool_id,
            output: Some(serde_json::Value::Object(payload)),
            index: None,
            raw: Some(raw.clone()),
        }));
    }

    let call = build_tool_call(tool_name, tool_id, input, None, Some(raw.clone()));
    Some(StreamParseResult::ToolCall(call))
}

fn parse_openai_typed_tool_item(
    item: &OpenaiResponseOutputItem,
    raw: &Value,
) -> Option<StreamParseResult> {
    match item {
        OpenaiResponseOutputItem::FunctionCall(call) => {
            let tool_id = call.call_id.clone().or_else(|| Some(call.id.clone()));
            let input = call.arguments.clone().and_then(parse_tool_input);
            if call.status.as_deref() == Some("in_progress") && input.is_none() {
                return None;
            }
            let call = build_tool_call(call.name.clone(), tool_id, input, None, Some(raw.clone()));
            Some(StreamParseResult::ToolCall(call))
        }
        OpenaiResponseOutputItem::WebSearchCall(call) => {
            let (query, queries) = call
                .action
                .as_ref()
                .map(openai_web_search_query_and_queries)
                .unwrap_or((None, None));
            if query.is_some() || queries.is_some() {
                return Some(StreamParseResult::WebSearchAction {
                    tool_name: "web_search_call".to_string(),
                    tool_id: call.call_id.clone().or_else(|| Some(call.id.clone())),
                    query,
                    queries,
                });
            }
            None
        }
        OpenaiResponseOutputItem::Other => None,
    }
}

fn openai_web_search_query_and_queries(
    action: &OpenaiWebSearchAction,
) -> (Option<String>, Option<Vec<String>>) {
    let query = action.query.clone();
    let mut queries = action.queries.clone();
    if queries.is_none() {
        if let Some(query_value) = query.as_ref() {
            queries = Some(vec![query_value.clone()]);
        }
    }
    (query, queries)
}

fn resolve_tool_name(item: &Value, item_type: &str) -> String {
    item.get("name")
        .and_then(|n| n.as_str())
        .or_else(|| item.get("tool_name").and_then(|n| n.as_str()))
        .or_else(|| {
            if item_type.is_empty() {
                None
            } else {
                Some(item_type)
            }
        })
        .unwrap_or("tool_call")
        .to_string()
}

fn resolve_tool_id(item: &Value) -> Option<String> {
    item.get("call_id")
        .or_else(|| item.get("id"))
        .and_then(|id| id.as_str())
        .map(|s| s.to_string())
}

fn extract_tool_output(item: &Value) -> Option<Value> {
    item.get("output")
        .cloned()
        .or_else(|| item.get("result").cloned())
        .or_else(|| item.get("response").cloned())
        .or_else(|| item.get("results").cloned())
        .or_else(|| item.get("content").cloned())
        .or_else(|| item.get("output").and_then(|o| o.get("results")).cloned())
}

fn parse_openai_url_citation(annotation: &Value) -> Option<StreamWebSearchResult> {
    let annotation_type = annotation.get("type").and_then(|t| t.as_str())?;
    if annotation_type != "url_citation" {
        return None;
    }
    let title = annotation
        .get("title")
        .and_then(|t| t.as_str())?
        .to_string();
    let url = annotation.get("url").and_then(|u| u.as_str())?.to_string();
    Some(StreamWebSearchResult {
        title,
        url,
        source: None,
        page_age: None,
        snippet: None,
    })
}

fn parse_tool_input(value: Value) -> Option<ToolInput> {
    if value.is_null() {
        return None;
    }
    match value {
        Value::String(raw) => {
            if raw.trim().is_empty() {
                return None;
            }
            let trimmed = raw.trim_start();
            if trimmed.starts_with('{') || trimmed.starts_with('[') {
                if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                    return Some(ToolInput::Json(parsed));
                }
            }
            Some(ToolInput::Text(raw))
        }
        other => Some(ToolInput::Json(other)),
    }
}

pub fn build_openai_tools(
    web_search: bool,
    mcp_openai_tools: Vec<OpenaiTool>,
    artifact_schema: Value,
) -> Option<Vec<OpenaiTool>> {
    let mut tools = Vec::new();
    if web_search {
        tools.push(OpenaiTool::web_search());
    }
    tools.extend(mcp_openai_tools);
    tools.push(OpenaiTool::Function {
        name: ARTIFACT_TOOL_NAME.to_string(),
        description: Some(ARTIFACT_TOOL_DESC.to_string()),
        parameters: artifact_schema,
        strict: None,
    });
    if tools.is_empty() { None } else { Some(tools) }
}

pub async fn continue_openai_stream(
    client: &ReqwestClient,
    settings: &OpenaiSettings,
    model_name: String,
    temperature: Option<f32>,
    user_id: &Uuid,
    tools: Option<Vec<OpenaiTool>>,
    prev_response_id: Option<String>,
    next_input: Vec<OpenaiInputItem>,
) -> Result<EventSource, Error> {
    client
        .openai_chat_stream(
            settings,
            model_name,
            temperature,
            Vec::new(),
            user_id,
            tools,
            None,
            prev_response_id,
            Some(next_input),
        )
        .await
}

pub fn make_openai_function_output(call_id: String, output: &Value) -> OpenaiInputItem {
    OpenaiInputItem::FunctionCallOutput(OpenaiFunctionCallOutput {
        item_type: "function_call_output".to_string(),
        call_id,
        output: serde_json::to_string(output).unwrap_or_else(|_| "{}".to_string()),
    })
}
