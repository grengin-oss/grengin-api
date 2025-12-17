use crate::dto::llm::openai::OpenaiResponseStreamEvent;
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
        match serde_json::from_str::<OpenaiResponseStreamEvent>(data) {
            Ok(stream_event) => match stream_event {
                OpenaiResponseStreamEvent::OutputTextDelta(delta) => {
                    StreamParseResult::TextDelta {
                        text: delta.delta,
                        request_id: Some(delta.item_id),
                    }
                }
                OpenaiResponseStreamEvent::Other => StreamParseResult::None,
            },
            Err(_) => StreamParseResult::None,
        }
    }
}
