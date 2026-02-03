pub mod openai;
pub mod anthropic;

/// Result of parsing a streaming event
#[derive(Debug, Clone)]
pub enum StreamParseResult {
    None,

    // PATCH: include optional usage on start (useful for Anthropic message_start)
    MessageStart {
        request_id: String,
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
    },

    TextDelta {
        text: String,
        request_id: Option<String>,
    },

    ToolInput {
        partial_json: String,
        index: Option<u32>,
    },

    EventLog {
        event_type: String,
        message: Option<String>,
        data: Option<serde_json::Value>,
    },

    ToolCall {
        tool_name: String,
        tool_id: Option<String>,
        input: Option<serde_json::Value>,
        index: Option<u32>,
        raw: Option<serde_json::Value>,
    },

    WebSearchAction {
        tool_name: String,
        tool_id: Option<String>,
        query: Option<String>,
        queries: Option<Vec<String>>,
    },

    WebSearchResult {
        tool_name: String,
        tool_id: Option<String>,
        results: Vec<StreamWebSearchResult>,
    },

    ToolResult {
        tool_name: Option<String>,
        tool_id: Option<String>,
        output: Option<serde_json::Value>,
        index: Option<u32>,
        raw: Option<serde_json::Value>,
    },

    // NEW: token usage updates mid/final stream
    TokenUsage {
        request_id: Option<String>,
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
        total_tokens: Option<u32>,
    },

    Error {
        error_type: String,
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct StreamWebSearchResult {
    pub title: String,
    pub url: String,
    pub source: Option<String>,
    pub page_age: Option<String>,
    pub snippet: Option<String>,
}

/// Trait for parsing provider-specific streaming events
pub trait StreamParser: Send + Sync {
    /// Parse a raw SSE message data string into a StreamParseResult
    fn parse_event(&self, data: &str) -> StreamParseResult;
}

impl StreamParseResult {

    /// Extract request_id if available
    pub fn request_id(&self) -> Option<String> {
        match self {
            StreamParseResult::TextDelta { request_id, .. } => request_id.clone(),
            StreamParseResult::MessageStart { request_id,input_tokens:_,output_tokens:_ } => Some(request_id.clone()),
            _ => None,
        }
    }

    /// Extract text content if available
    pub fn text(&self) -> Option<&str> {
        match self {
            StreamParseResult::TextDelta { text, .. } => Some(text),
            _ => None,
        }
    }
}
