use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::Error;
use reqwest::Client as ReqwestClient;
use reqwest_eventsource::EventSource;
use serde_json::{Value, json};

use crate::{
    config::setting::MistralSettings,
    dto::llm::mistral::{
        MistralChatCompletionChunk, MistralConversationFunctionResult, MistralMessage, MistralTool,
        MistralToolCall, MistralToolCallDelta, MistralToolDefinition, MistralToolFunction,
    },
    llm::{prompt::Prompt, provider::MistralApis},
    models::messages::ChatRole,
    services::mcp_tools::McpToolDescriptor,
};

use super::{
    StreamErrorKind, StreamParseResult, StreamParser, ToolInput, build_tool_call,
    build_tool_input_delta, parse_web_search_action,
};

#[derive(Debug, Clone)]
struct MistralToolCallMeta {
    name: Option<String>,
    buffer: String,
    index: Option<u32>,
}

pub struct MistralStreamParser {
    tool_calls: Mutex<HashMap<String, MistralToolCallMeta>>,
    index_to_id: Mutex<HashMap<u32, String>>,
}

impl MistralStreamParser {
    pub fn new() -> Self {
        Self {
            tool_calls: Mutex::new(HashMap::new()),
            index_to_id: Mutex::new(HashMap::new()),
        }
    }

    fn resolve_tool_id(&self, call: &MistralToolCallDelta) -> String {
        if let Some(id) = call.id.clone() {
            if let Some(index) = call.index {
                if let Ok(mut map) = self.index_to_id.lock() {
                    map.insert(index, id.clone());
                }
            }
            return id;
        }
        if let Some(index) = call.index {
            if let Ok(map) = self.index_to_id.lock() {
                if let Some(existing) = map.get(&index) {
                    return existing.clone();
                }
            }
            let fallback = format!("mistral_tool_{}", index);
            if let Ok(mut map) = self.index_to_id.lock() {
                map.insert(index, fallback.clone());
            }
            return fallback;
        }
        "mistral_tool_0".to_string()
    }

    fn upsert_tool_meta(
        &self,
        tool_id: String,
        name: Option<String>,
        index: Option<u32>,
        arguments_delta: Option<&str>,
    ) -> (Option<String>, String) {
        if let Ok(mut calls) = self.tool_calls.lock() {
            let entry = calls
                .entry(tool_id.clone())
                .or_insert_with(|| MistralToolCallMeta {
                    name: name.clone(),
                    buffer: String::new(),
                    index,
                });
            if entry.name.is_none() && name.is_some() {
                entry.name = name.clone();
            }
            if entry.index.is_none() && index.is_some() {
                entry.index = index;
            }
            if let Some(delta) = arguments_delta {
                entry.buffer.push_str(delta);
            }
            return (entry.name.clone(), entry.buffer.clone());
        }
        (name, arguments_delta.unwrap_or_default().to_string())
    }

    fn handle_tool_call_delta(
        &self,
        call: &MistralToolCallDelta,
        finish_reason: Option<&str>,
        raw: &Value,
    ) -> Option<StreamParseResult> {
        let tool_id = self.resolve_tool_id(call);
        let index = call.index;
        let name = call.function.as_ref().and_then(|f| f.name.clone());
        let arguments = call.function.as_ref().and_then(|f| f.arguments.clone());

        let (resolved_name, buffer) =
            self.upsert_tool_meta(tool_id.clone(), name.clone(), index, arguments.as_deref());

        let arguments_text = arguments.unwrap_or_else(|| buffer.clone());
        if arguments_text.is_empty() {
            return None;
        }

        if let Ok(json) = serde_json::from_str::<Value>(&arguments_text) {
            if finish_reason == Some("tool_calls") || finish_reason == Some("stop") {
                let tool_name = resolved_name.unwrap_or_else(|| "tool_call".to_string());
                let input = Some(ToolInput::Json(json));
                let call =
                    build_tool_call(tool_name, Some(tool_id), input, index, Some(raw.clone()));
                return Some(StreamParseResult::ToolCall(call));
            }
        }

        let web_search = serde_json::from_str::<Value>(&arguments_text)
            .ok()
            .and_then(|value| parse_web_search_action(&value));
        let delta = build_tool_input_delta(
            arguments_text,
            index,
            resolved_name,
            Some(tool_id),
            web_search,
        );
        Some(StreamParseResult::ToolInput(delta))
    }

    fn pop_buffered_tool_call(&self, raw: &Value) -> Option<StreamParseResult> {
        let mut entry = None;
        if let Ok(mut calls) = self.tool_calls.lock() {
            if let Some((id, meta)) = calls.iter().next().map(|(k, v)| (k.clone(), v.clone())) {
                entry = Some((id.clone(), meta));
                calls.remove(&id);
            }
        }
        if let Some((tool_id, meta)) = entry {
            if meta.buffer.is_empty() {
                return None;
            }
            if let Ok(value) = serde_json::from_str::<Value>(&meta.buffer) {
                let tool_name = meta.name.unwrap_or_else(|| "tool_call".to_string());
                let call = build_tool_call(
                    tool_name,
                    Some(tool_id),
                    Some(ToolInput::Json(value)),
                    meta.index,
                    Some(raw.clone()),
                );
                return Some(StreamParseResult::ToolCall(call));
            }
        }
        None
    }
}

impl Default for MistralStreamParser {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamParser for MistralStreamParser {
    fn parse_event(&self, data: &str) -> StreamParseResult {
        let trimmed = data.trim();
        if trimmed == "[DONE]" || trimmed == "null" {
            return StreamParseResult::None;
        }

        let value = match serde_json::from_str::<Value>(data) {
            Ok(v) => v,
            Err(_) => return StreamParseResult::None,
        };

        if value.get("object").and_then(|v| v.as_str()) == Some("error") {
            let message = value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Mistral stream error")
                .to_string();
            let raw_type = value
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("mistral_error");
            return StreamParseResult::Error {
                kind: StreamErrorKind::from_provider_str(raw_type),
                message,
            };
        }
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Mistral stream error")
                .to_string();
            let raw_type = error
                .get("type")
                .or_else(|| error.get("code"))
                .and_then(|v| v.as_str())
                .unwrap_or("mistral_error");
            return StreamParseResult::Error {
                kind: StreamErrorKind::from_provider_str(raw_type),
                message,
            };
        }

        if let Ok(chunk) = serde_json::from_value::<MistralChatCompletionChunk>(value.clone()) {
            // Process choices (text / tool calls) BEFORE checking usage so that a chunk
            // carrying both finish_reason:"tool_calls" and usage doesn't lose the tool call.
            if let Some(choice) = chunk.choices.first() {
                let finish_reason = choice.finish_reason.as_deref();
                if let Some(text) = choice.delta.content.clone() {
                    return StreamParseResult::TextDelta {
                        text,
                        request_id: Some(chunk.id),
                    };
                }
                if let Some(tool_calls) = choice.delta.tool_calls.as_ref() {
                    for call in tool_calls {
                        if let Some(result) =
                            self.handle_tool_call_delta(call, finish_reason, &value)
                        {
                            return result;
                        }
                    }
                }
                if finish_reason == Some("tool_calls") {
                    if let Some(result) = self.pop_buffered_tool_call(&value) {
                        return result;
                    }
                }
            }

            if let Some(usage) = chunk.usage {
                return StreamParseResult::TokenUsage {
                    request_id: Some(chunk.id),
                    input_tokens: Some(usage.prompt_tokens),
                    output_tokens: Some(usage.completion_tokens),
                    total_tokens: Some(usage.total_tokens),
                };
            }
        }

        if let Some(choices) = value.get("choices").and_then(|v| v.as_array()) {
            if let Some(choice) = choices.first() {
                let finish_reason = choice.get("finish_reason").and_then(|v| v.as_str());
                if let Some(message) = choice.get("message") {
                    if let Some(tool_calls) = message.get("tool_calls").and_then(|v| v.as_array()) {
                        for call in tool_calls {
                            if let Ok(delta) =
                                serde_json::from_value::<MistralToolCallDelta>(call.clone())
                            {
                                if let Some(result) =
                                    self.handle_tool_call_delta(&delta, finish_reason, &value)
                                {
                                    return result;
                                }
                            }
                        }
                    }
                }
            }
        }

        StreamParseResult::None
    }
}

pub fn build_mistral_tools(
    use_conversations: bool,
    mcp_tool_lookup: &HashMap<String, McpToolDescriptor>,
) -> Option<Vec<MistralTool>> {
    let mut tools = Vec::new();
    if !use_conversations {
        for descriptor in mcp_tool_lookup.values() {
            let description = descriptor
                .description
                .clone()
                .unwrap_or_else(|| descriptor.original_name.clone());
            tools.push(MistralTool::Function {
                function: MistralToolDefinition {
                    name: descriptor.openai_name.clone(),
                    description: Some(description),
                    parameters: descriptor.input_schema.clone(),
                },
            });
        }
    }
    if tools.is_empty() { None } else { Some(tools) }
}

pub fn build_mistral_tool_choice(
    selected_tools: &[String],
    mcp_tool_lookup: &HashMap<String, McpToolDescriptor>,
    has_tools: bool,
) -> Option<Value> {
    if !has_tools {
        return None;
    }
    if !selected_tools.is_empty() && mcp_tool_lookup.len() == 1 {
        let tool_name = mcp_tool_lookup.keys().next().cloned().unwrap_or_default();
        Some(json!({"type":"function","function":{"name": tool_name}}))
    } else {
        Some(json!("auto"))
    }
}

pub async fn continue_mistral_stream(
    client: &ReqwestClient,
    settings: &MistralSettings,
    model_name: String,
    temperature: Option<f32>,
    messages: Vec<MistralMessage>,
    tools: Option<Vec<MistralTool>>,
    tool_choice: Option<Value>,
) -> Result<EventSource, Error> {
    client
        .mistral_chat_stream_with_messages(settings, model_name, temperature, messages, tools, tool_choice)
        .await
}

pub fn build_mistral_conversation_tools(
    web_search: bool,
    supports_mcp_tools: bool,
    mcp_tool_lookup: &HashMap<String, McpToolDescriptor>,
) -> Option<Vec<MistralTool>> {
    let mut tools = Vec::new();
    if web_search {
        tools.push(MistralTool::WebSearch);
    }
    if supports_mcp_tools {
        for descriptor in mcp_tool_lookup.values() {
            let description = descriptor
                .description
                .clone()
                .unwrap_or_else(|| descriptor.original_name.clone());
            tools.push(MistralTool::Function {
                function: MistralToolDefinition {
                    name: descriptor.openai_name.clone(),
                    description: Some(description),
                    parameters: descriptor.input_schema.clone(),
                },
            });
        }
    }
    if tools.is_empty() { None } else { Some(tools) }
}

pub fn build_mistral_agent_inputs(previous_prompts: &[Prompt]) -> (String, Value) {
    let mut instructions = Vec::new();
    let entries = previous_prompts
        .iter()
        .filter_map(|prompt| match prompt.role {
            ChatRole::System => {
                if !prompt.text.trim().is_empty() {
                    instructions.push(prompt.text.clone());
                }
                None
            }
            ChatRole::User | ChatRole::Assistant => Some(json!({
                "object": "entry",
                "type": "message.input",
                "role": prompt.role,
                "content": prompt.text,
            })),
            _ => None,
        })
        .collect::<Vec<Value>>();
    (instructions.join("\n\n"), Value::Array(entries))
}

pub fn build_mistral_completion_args(
    temperature: Option<f32>,
    selected_tools: &[String],
    mcp_tool_lookup: &HashMap<String, McpToolDescriptor>,
    use_conversations: bool,
) -> Option<Value> {
    let mut map = serde_json::Map::new();
    if let Some(t) = temperature {
        map.insert("temperature".to_string(), json!(t));
    }
    if !use_conversations && !selected_tools.is_empty() && mcp_tool_lookup.len() == 1 {
        if let Some(tool_name) = mcp_tool_lookup.keys().next() {
            map.insert(
                "tool_choice".to_string(),
                json!({"type": "function", "function": {"name": tool_name}}),
            );
        }
    }
    if map.is_empty() { None } else { Some(Value::Object(map)) }
}

pub fn make_mistral_conversation_result(call_id: String, output: &Value) -> MistralConversationFunctionResult {
    MistralConversationFunctionResult {
        object: "entry".to_string(),
        result_type: "function.result".to_string(),
        tool_call_id: call_id,
        result: serde_json::to_string(output).unwrap_or_else(|_| "{}".to_string()),
    }
}

pub fn make_mistral_tool_result(
    call_id: String,
    tool_name: String,
    args: Value,
    output: &Value,
) -> (MistralToolCall, MistralMessage) {
    let tool_call = MistralToolCall {
        id: call_id.clone(),
        call_type: "function".to_string(),
        function: MistralToolFunction {
            name: tool_name.clone(),
            arguments: serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string()),
        },
    };
    let message = MistralMessage::tool_response(
        tool_name,
        call_id,
        serde_json::to_string(output).unwrap_or_else(|_| "{}".to_string()),
    );
    (tool_call, message)
}

pub fn build_mistral_continuation(
    existing: Option<Vec<MistralMessage>>,
    base_prompts: Vec<Prompt>,
    stream_content: String,
    tool_calls: Vec<MistralToolCall>,
    tool_messages: Vec<MistralMessage>,
) -> Vec<MistralMessage> {
    let mut messages = existing.unwrap_or_else(|| MistralMessage::from_prompts(base_prompts));
    if !tool_calls.is_empty() {
        let content = if stream_content.trim().is_empty() { None } else { Some(stream_content) };
        messages.push(MistralMessage::assistant_with_tool_calls(content, tool_calls));
    }
    messages.extend(tool_messages);
    messages
}
