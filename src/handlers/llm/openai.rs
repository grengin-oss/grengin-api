use crate::dto::llm::openai::{OpenaiResponseStreamEvent, OpenaiChatCompletionChunk};
use super::{StreamParser, StreamParseResult};

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

fn u64_to_u32(v: Option<u64>) -> Option<u32> {
    v.and_then(|x| u32::try_from(x).ok())
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
                }

                _ => {}
            }
        }

        // 2) Raw JSON fallback for Responses SSE: response.completed includes usage
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
            if v.get("type").and_then(|t| t.as_str()) == Some("response.completed") {
                let request_id = v
                    .pointer("/response/id")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string());

                let input_tokens = u64_to_u32(v.pointer("/response/usage/input_tokens").and_then(|x| x.as_u64()));
                let output_tokens = u64_to_u32(v.pointer("/response/usage/output_tokens").and_then(|x| x.as_u64()));
                let total_tokens = u64_to_u32(v.pointer("/response/usage/total_tokens").and_then(|x| x.as_u64()));

                if input_tokens.is_some() || output_tokens.is_some() || total_tokens.is_some() {
                    return StreamParseResult::TokenUsage {
                        request_id,
                        input_tokens,
                        output_tokens,
                        total_tokens,
                    };
                }
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

        StreamParseResult::None
    }
}
