use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use anyhow::Error;
use reqwest::Client as ReqwestClient;
use reqwest_eventsource::EventSource;
use serde_json::{Value, json};

use crate::{
    config::setting::GeminiSettings,
    dto::llm::gemini::{
        GeminiContent, GeminiFunctionCall, GeminiFunctionResponse, GeminiPart,
        normalize_gemini_parameters,
    },
    llm::provider::GeminiApis,
    services::mcp_tools::McpToolDescriptor,
};

use crate::handlers::llm::{
    StreamErrorKind, StreamParseResult, StreamParser, StreamWebSearchResult, ToolInput,
    build_tool_call,
};

#[derive(Default)]
pub struct GeminiStreamParser {
    pending: Mutex<VecDeque<StreamParseResult>>,
}

impl GeminiStreamParser {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(VecDeque::new()),
        }
    }
}

impl StreamParser for GeminiStreamParser {
    fn parse_event(&self, data: &str) -> StreamParseResult {
        if let Ok(mut pending) = self.pending.lock() {
            if let Some(next) = pending.pop_front() {
                return next;
            }
        }

        let trimmed = data.trim();
        if trimmed.is_empty() || trimmed == "[DONE]" {
            return StreamParseResult::None;
        }

        let value: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(err) => {
                return StreamParseResult::Error {
                    kind: StreamErrorKind::ProviderError,
                    message: err.to_string(),
                };
            }
        };
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Gemini stream error")
                .to_string();
            let raw_type = error
                .get("status")
                .or_else(|| error.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("gemini_error");
            return StreamParseResult::Error {
                kind: StreamErrorKind::from_provider_str(raw_type),
                message,
            };
        }

        let responses = value
            .as_array()
            .cloned()
            .unwrap_or_else(|| vec![value.clone()]);
        let mut parsed_results: Vec<StreamParseResult> = Vec::new();

        for response in responses {
            let candidates = response
                .get("candidates")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            for candidate in candidates {
                if let Some(content) = candidate.get("content") {
                    if let Some(parts) = content.get("parts").and_then(|v| v.as_array()) {
                        for part in parts {
                            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                                if !text.is_empty() {
                                    parsed_results.push(StreamParseResult::TextDelta {
                                        text: text.to_string(),
                                        request_id: None,
                                    });
                                }
                            }

                            if let Some(fc) = part.get("functionCall") {
                                let name = fc
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let tool_id =
                                    fc.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
                                let args = fc
                                    .get("args")
                                    .cloned()
                                    .unwrap_or_else(|| Value::Object(Default::default()));
                                if !name.is_empty() {
                                    let call = build_tool_call(
                                        name,
                                        tool_id,
                                        Some(ToolInput::Json(args)),
                                        None,
                                        Some(part.clone()),
                                    );
                                    parsed_results.push(StreamParseResult::ToolCall(call));
                                }
                            }
                        }
                    }
                }

                // Web grounding metadata (google_search)
                if let Some(meta) = candidate
                    .get("groundingMetadata")
                    .or_else(|| candidate.get("grounding_metadata"))
                {
                    if let Some(queries) = meta
                        .get("webSearchQueries")
                        .or_else(|| meta.get("web_search_queries"))
                        .and_then(|v| v.as_array())
                    {
                        let queries = queries
                            .iter()
                            .filter_map(|q| q.as_str().map(|s| s.to_string()))
                            .collect::<Vec<String>>();
                        if !queries.is_empty() {
                            parsed_results.push(StreamParseResult::WebSearchAction {
                                tool_name: "web_search_call".to_string(),
                                tool_id: Some("gemini_web_search".to_string()),
                                query: queries.first().cloned(),
                                queries: Some(queries),
                            });
                        }
                    }

                    if let Some(chunks) = meta
                        .get("groundingChunks")
                        .or_else(|| meta.get("grounding_chunks"))
                        .and_then(|v| v.as_array())
                    {
                        let mut results: Vec<StreamWebSearchResult> = Vec::new();
                        for chunk in chunks {
                            let web = chunk.get("web").unwrap_or(chunk);
                            let url = web
                                .get("uri")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let title = web
                                .get("title")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if url.is_empty() && title.is_empty() {
                                continue;
                            }
                            results.push(StreamWebSearchResult {
                                title,
                                url,
                                source: None,
                                page_age: None,
                                snippet: None,
                            });
                        }
                        if !results.is_empty() {
                            parsed_results.push(StreamParseResult::WebSearchResult {
                                tool_name: "web_search_call".to_string(),
                                tool_id: Some("gemini_web_search".to_string()),
                                results,
                            });
                        }
                    }
                }
            }

            if let Some(usage) = response.get("usageMetadata") {
                let input_tokens = usage
                    .get("promptTokenCount")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32);
                let output_tokens = usage
                    .get("candidatesTokenCount")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32);
                let total_tokens = usage
                    .get("totalTokenCount")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32);
                if input_tokens.is_some() || output_tokens.is_some() || total_tokens.is_some() {
                    parsed_results.push(StreamParseResult::TokenUsage {
                        request_id: None,
                        input_tokens,
                        output_tokens,
                        total_tokens,
                    });
                }
            }
        }

        if parsed_results.is_empty() {
            StreamParseResult::None
        } else {
            let mut queue = VecDeque::from(parsed_results);
            let first = queue.pop_front().unwrap_or(StreamParseResult::None);
            if let Ok(mut pending) = self.pending.lock() {
                pending.extend(queue);
            }
            first
        }
    }
}

pub fn build_gemini_tools(
    web_search: bool,
    mcp_tool_lookup: &HashMap<String, McpToolDescriptor>,
) -> Option<Value> {
    let mut tools: Vec<Value> = Vec::new();
    if web_search {
        tools.push(json!({ "google_search": {} }));
    }
    let mut function_declarations: Vec<Value> = Vec::new();
    for descriptor in mcp_tool_lookup.values() {
        let description = descriptor
            .description
            .clone()
            .unwrap_or_else(|| descriptor.original_name.clone());
        function_declarations.push(json!({
            "name": descriptor.openai_name.clone(),
            "description": description,
            "parameters": normalize_gemini_parameters(&descriptor.input_schema),
        }));
    }
    if !function_declarations.is_empty() {
        tools.push(json!({ "function_declarations": function_declarations }));
    }
    if tools.is_empty() { None } else { Some(Value::Array(tools)) }
}

pub fn build_gemini_tool_config(
    web_search: bool,
    selected_tools: &[String],
    mcp_tool_lookup: &HashMap<String, McpToolDescriptor>,
) -> Option<Value> {
    // function_calling_config is only valid when function_declarations are present.
    // Sending it with only google_search causes a 400.
    if mcp_tool_lookup.is_empty() {
        return None;
    }
    let mut config = serde_json::Map::new();
    if web_search {
        config.insert(
            "include_server_side_tool_invocations".to_string(),
            json!(true),
        );
    }
    if !selected_tools.is_empty() && mcp_tool_lookup.len() == 1 {
        let tool_name = mcp_tool_lookup.keys().next().cloned().unwrap_or_default();
        config.insert(
            "function_calling_config".to_string(),
            json!({ "mode": "ANY", "allowed_function_names": [tool_name] }),
        );
    } else {
        config.insert(
            "function_calling_config".to_string(),
            json!({ "mode": "AUTO" }),
        );
    }
    Some(Value::Object(config))
}

pub fn build_gemini_tool_messages(
    call_id: String,
    tool_name: String,
    args: Value,
    output: &Value,
    thought_signature: Option<Value>,
) -> (GeminiContent, GeminiContent) {
    let model_turn = GeminiContent {
        role: "model".to_string(),
        parts: vec![GeminiPart {
            function_call: Some(GeminiFunctionCall {
                id: call_id.clone(),
                name: tool_name.clone(),
                args,
            }),
            thought_signature,
            ..GeminiPart::default()
        }],
    };
    let user_turn = GeminiContent {
        role: "user".to_string(),
        parts: vec![GeminiPart {
            function_response: Some(GeminiFunctionResponse {
                id: call_id,
                name: tool_name,
                response: json!({ "output": output }),
            }),
            ..GeminiPart::default()
        }],
    };
    (model_turn, user_turn)
}

pub async fn continue_gemini_stream(
    client: &ReqwestClient,
    settings: &GeminiSettings,
    model_name: String,
    temperature: Option<f32>,
    system_instruction: Option<Value>,
    contents: Vec<Value>,
    tools: Option<Value>,
    tool_config: Option<Value>,
) -> Result<EventSource, Error> {
    client
        .gemini_chat_stream_with_contents(
            settings,
            model_name,
            temperature,
            system_instruction,
            Value::Array(contents),
            tools,
            tool_config,
        )
        .await
}
