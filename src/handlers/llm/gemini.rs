use std::collections::VecDeque;
use std::sync::Mutex;

use serde_json::Value;

use crate::handlers::llm::{
    StreamParseResult, StreamParser, StreamWebSearchResult, ToolInput, build_tool_call,
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
                    error_type: "gemini_parse_error".to_string(),
                    message: err.to_string(),
                };
            }
        };
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("gemini stream error")
                .to_string();
            let error_type = error
                .get("status")
                .or_else(|| error.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("gemini_error")
                .to_string();
            return StreamParseResult::Error {
                error_type,
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
