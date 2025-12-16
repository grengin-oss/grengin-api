use crate::dto::llm::groq::GroqChatCompletionChunk;
use super::{StreamParser, StreamParseResult};

/// Groq stream parser (OpenAI-compatible chat completions format)
pub struct GroqStreamParser;

impl GroqStreamParser {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GroqStreamParser {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamParser for GroqStreamParser {
    fn parse_event(&self, data: &str) -> StreamParseResult {
        // Handle [DONE] message
        if data.trim() == "[DONE]" {
            return StreamParseResult::None;
        }

        match serde_json::from_str::<GroqChatCompletionChunk>(data) {
            Ok(chunk) => {
                // Extract request ID from x_groq metadata if available
                let request_id = chunk.x_groq.and_then(|meta| meta.id);

                // Get the first choice's delta content
                if let Some(choice) = chunk.choices.first() {
                    if let Some(content) = &choice.delta.content {
                        if !content.is_empty() {
                            return StreamParseResult::TextDelta {
                                text: content.clone(),
                                request_id: request_id.or(Some(chunk.id)),
                            };
                        }
                    }
                }
                StreamParseResult::None
            }
            Err(_) => StreamParseResult::None,
        }
    }
}
