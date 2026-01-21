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

        StreamParseResult::None
    }
}
