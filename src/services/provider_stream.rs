// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamErrorKind {
    QuotaExhausted,
    ProviderError,
}

impl StreamErrorKind {
    pub fn from_provider_str(value: &str) -> Self {
        match value {
            "RESOURCE_EXHAUSTED"
            | "rate_limit_error"
            | "insufficient_quota"
            | "rate_limit_exceeded" => Self::QuotaExhausted,
            _ => Self::ProviderError,
        }
    }
}

#[derive(Debug, Clone)]
pub enum StreamParseResult {
    None,
    MessageStart {
        request_id: String,
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
        cached_input_tokens: Option<u32>,
        cache_creation_tokens: Option<u32>,
    },
    TextDelta {
        text: String,
        request_id: Option<String>,
    },
    ToolInput(ToolInputDelta),
    EventLog {
        event_type: String,
        message: Option<String>,
        data: Option<Value>,
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
    TokenUsage {
        request_id: Option<String>,
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
        total_tokens: Option<u32>,
        cached_input_tokens: Option<u32>,
        cache_creation_tokens: Option<u32>,
    },
    Error {
        kind: StreamErrorKind,
        message: String,
    },
}

pub trait StreamParser: Send + Sync {
    fn parse_event(&self, data: &str) -> StreamParseResult;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    WebSearch,
    Other,
}

pub fn tool_name_is_web_search(name: &str) -> bool {
    name.contains("web_search")
}

#[derive(Debug, Clone)]
pub enum ToolInput {
    Json(Value),
    Text(String),
}

impl ToolInput {
    pub fn as_json(&self) -> Option<&Value> {
        match self {
            Self::Json(value) => Some(value),
            Self::Text(_) => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Json(_) => None,
            Self::Text(value) => Some(value),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub tool_name: String,
    pub tool_id: Option<String>,
    pub input: Option<ToolInput>,
    pub index: Option<u32>,
    pub raw: Option<Value>,
    pub web_search: Option<StreamWebSearchAction>,
}

impl ToolCall {
    pub fn is_web_search(&self) -> bool {
        self.web_search.is_some() || tool_name_is_web_search(&self.tool_name)
    }

    pub fn kind(&self) -> ToolKind {
        if self.is_web_search() {
            ToolKind::WebSearch
        } else {
            ToolKind::Other
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolInputDelta {
    pub partial_json: String,
    pub index: Option<u32>,
    pub tool_name: Option<String>,
    pub tool_id: Option<String>,
    pub web_search: Option<StreamWebSearchAction>,
}

impl ToolInputDelta {
    pub fn is_web_search(&self) -> bool {
        self.web_search.is_some()
            || self
                .tool_name
                .as_deref()
                .map(tool_name_is_web_search)
                .unwrap_or(false)
    }
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub tool_name: Option<String>,
    pub tool_id: Option<String>,
    pub output: Option<Value>,
    pub index: Option<u32>,
    pub raw: Option<Value>,
}

impl ToolResult {
    pub fn is_web_search(&self) -> bool {
        self.tool_name
            .as_deref()
            .map(tool_name_is_web_search)
            .unwrap_or(false)
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

pub fn parse_web_search_action(input: &Value) -> Option<StreamWebSearchAction> {
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut queries = input
        .get("queries")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        });
    if queries.is_none()
        && let Some(query) = query.as_ref()
    {
        queries = Some(vec![query.clone()]);
    }
    if query.is_none() && queries.is_none() {
        None
    } else {
        Some(StreamWebSearchAction { query, queries })
    }
}

pub fn update_web_search_action_state(
    state: &mut HashMap<String, StreamWebSearchState>,
    last_call_id: &mut Option<String>,
    tool_id: Option<String>,
    action: Option<StreamWebSearchAction>,
) -> Option<(String, StreamWebSearchState)> {
    let call_id = tool_id.or_else(|| last_call_id.clone())?;
    *last_call_id = Some(call_id.clone());
    let entry = state
        .entry(call_id.clone())
        .or_insert_with(empty_web_search_state);
    if let Some(action) = action {
        if action.query.is_some() {
            entry.query = action.query;
        }
        if action.queries.is_some() {
            entry.queries = action.queries;
        }
    }
    Some((call_id, entry.clone()))
}

pub fn update_web_search_results_state(
    state: &mut HashMap<String, StreamWebSearchState>,
    last_call_id: &mut Option<String>,
    tool_id: Option<String>,
    results: Vec<StreamWebSearchResult>,
) -> Option<(String, StreamWebSearchState)> {
    let call_id = tool_id.or_else(|| last_call_id.clone())?;
    *last_call_id = Some(call_id.clone());
    let entry = state
        .entry(call_id.clone())
        .or_insert_with(empty_web_search_state);
    entry.results.extend(results);
    Some((call_id, entry.clone()))
}

fn empty_web_search_state() -> StreamWebSearchState {
    StreamWebSearchState {
        query: None,
        queries: None,
        results: Vec::new(),
    }
}

impl StreamParseResult {
    pub fn request_id(&self) -> Option<String> {
        match self {
            Self::TextDelta { request_id, .. } => request_id.clone(),
            Self::MessageStart { request_id, .. } => Some(request_id.clone()),
            _ => None,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Self::TextDelta { text, .. } => Some(text),
            _ => None,
        }
    }
}
