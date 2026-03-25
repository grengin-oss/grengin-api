use serde_json::Value;

use super::StreamWebSearchAction;

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
            ToolInput::Json(value) => Some(value),
            ToolInput::Text(_) => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            ToolInput::Json(_) => None,
            ToolInput::Text(value) => Some(value.as_str()),
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
