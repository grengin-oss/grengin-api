pub mod openai;
pub mod anthropic;
pub mod mistral;
pub mod mistral_conversations;
pub mod tool;

pub use tool::{
    tool_name_is_web_search,
    ToolCall,
    ToolInput,
    ToolInputDelta,
    ToolKind,
    ToolResult,
};

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

    ToolInput(ToolInputDelta),

    EventLog {
        event_type: String,
        message: Option<String>,
        data: Option<serde_json::Value>,
    },

    ToolCall(ToolCall),

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

    ToolResult(ToolResult),

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

pub(crate) fn build_tool_call(
    tool_name: String,
    tool_id: Option<String>,
    input: Option<ToolInput>,
    index: Option<u32>,
    raw: Option<serde_json::Value>,
) -> ToolCall {
    let web_search = if tool_name_is_web_search(&tool_name) {
        input
            .as_ref()
            .and_then(|value| value.as_json())
            .and_then(parse_web_search_action)
    } else {
        None
    };
    ToolCall {
        tool_name,
        tool_id,
        input,
        index,
        raw,
        web_search,
    }
}

pub(crate) fn build_tool_input_delta(
    partial_json: String,
    index: Option<u32>,
    tool_name: Option<String>,
    tool_id: Option<String>,
    web_search: Option<StreamWebSearchAction>,
) -> ToolInputDelta {
    ToolInputDelta {
        partial_json,
        index,
        tool_name,
        tool_id,
        web_search,
    }
}

#[derive(Debug, Clone)]
pub struct StreamWebSearchAction {
    pub query: Option<String>,
    pub queries: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct StreamWebSearchState {
    pub query: Option<String>,
    pub queries: Option<Vec<String>>,
    pub results: Vec<StreamWebSearchResult>,
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

pub fn parse_web_search_action(input: &serde_json::Value) -> Option<StreamWebSearchAction> {
    let query = input
        .get("query")
        .and_then(|q| q.as_str())
        .map(|s| s.to_string());
    let mut queries = input
        .get("queries")
        .and_then(|q| q.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<String>>()
        });
    if queries.is_none() {
        if let Some(query_value) = query.as_ref() {
            queries = Some(vec![query_value.clone()]);
        }
    }
    if query.is_none() && queries.is_none() {
        None
    } else {
        Some(StreamWebSearchAction { query, queries })
    }
}

pub fn update_web_search_action_state(
    state: &mut std::collections::HashMap<String, StreamWebSearchState>,
    last_call_id: &mut Option<String>,
    tool_id: Option<String>,
    action: Option<StreamWebSearchAction>,
) -> Option<(String, StreamWebSearchState)> {
    let call_id = tool_id.or_else(|| last_call_id.clone());
    if let Some(id) = call_id {
        *last_call_id = Some(id.clone());
        let entry = state.entry(id.clone()).or_insert_with(|| StreamWebSearchState {
            query: None,
            queries: None,
            results: Vec::new(),
        });
        if let Some(action) = action {
            if action.query.is_some() {
                entry.query = action.query;
            }
            if action.queries.is_some() {
                entry.queries = action.queries;
            }
        }
        return Some((id, entry.clone()));
    }
    None
}

pub fn update_web_search_results_state(
    state: &mut std::collections::HashMap<String, StreamWebSearchState>,
    last_call_id: &mut Option<String>,
    tool_id: Option<String>,
    results: Vec<StreamWebSearchResult>,
) -> Option<(String, StreamWebSearchState)> {
    let call_id = tool_id.or_else(|| last_call_id.clone());
    if let Some(id) = call_id {
        *last_call_id = Some(id.clone());
        let entry = state.entry(id.clone()).or_insert_with(|| StreamWebSearchState {
            query: None,
            queries: None,
            results: Vec::new(),
        });
        entry.results.extend(results);
        return Some((id.clone(), entry.clone()));
    }
    None
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
