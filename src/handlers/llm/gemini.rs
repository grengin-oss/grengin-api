use crate::dto::llm::gemini::GeminiStreamChunk;
use super::{StreamParser, StreamParseResult};

/// Gemini stream parser
pub struct GeminiStreamParser;

impl GeminiStreamParser {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GeminiStreamParser {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamParser for GeminiStreamParser {
    fn parse_event(&self, data: &str) -> StreamParseResult {
        // Gemini streaming uses newline-delimited JSON (not SSE)
        // Each line is a complete JSON object
        let trimmed = data.trim();

        // Skip empty lines
        if trimmed.is_empty() {
            return StreamParseResult::None;
        }

        // Handle array wrapper that Gemini uses for streaming
        // The response comes as a JSON array, we need to parse individual objects
        let json_str = if trimmed.starts_with('[') {
            // Beginning of array, skip
            return StreamParseResult::None;
        } else if trimmed.starts_with(',') {
            // Continuation in array, strip comma
            trimmed.trim_start_matches(',').trim()
        } else if trimmed == "]" {
            // End of array
            return StreamParseResult::None;
        } else {
            trimmed
        };

        match serde_json::from_str::<GeminiStreamChunk>(json_str) {
            Ok(chunk) => {
                if let Some(text) = chunk.get_text() {
                    if !text.is_empty() {
                        return StreamParseResult::TextDelta {
                            text,
                            request_id: None,
                        };
                    }
                }
                StreamParseResult::None
            }
            Err(_) => StreamParseResult::None,
        }
    }
}
