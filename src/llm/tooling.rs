use serde_json::Value;
use uuid::Uuid;

use crate::dto::llm::openai::OpenaiTool;

#[derive(Debug, Clone)]
pub struct UnifiedToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
    pub server_id: Option<Uuid>,
    pub tool_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct UnifiedToolCall {
    pub tool_name: String,
    pub tool_id: Option<String>,
    pub server_id: Option<Uuid>,
    pub tool_ref: Option<Uuid>,
    pub arguments: Value,
}

#[derive(Debug, Clone)]
pub struct UnifiedToolResult {
    pub tool_name: String,
    pub tool_id: Option<String>,
    pub server_id: Option<Uuid>,
    pub tool_ref: Option<Uuid>,
    pub output: Value,
    pub is_error: bool,
}

pub fn sanitize_tool_name(name: &str) -> String {
    let mut sanitized = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            sanitized.push(ch.to_ascii_lowercase());
        } else {
            sanitized.push('_');
        }
    }
    sanitized
}

pub fn mcp_openai_tool_name(server_id: &Uuid, tool_name: &str) -> String {
    let server_part = server_id.to_string().replace('-', "");
    let tool_part = sanitize_tool_name(tool_name);
    format!("mcp__{server_part}__{tool_part}")
}

impl UnifiedToolDefinition {
    pub fn openai_tool_name(&self) -> String {
        if let Some(server_id) = self.server_id.as_ref() {
            mcp_openai_tool_name(server_id, &self.name)
        } else {
            sanitize_tool_name(&self.name)
        }
    }

    pub fn to_openai_tool(&self) -> OpenaiTool {
        OpenaiTool::Function {
            name: self.openai_tool_name(),
            description: self.description.clone(),
            parameters: self.input_schema.clone(),
            strict: None,
        }
    }
}
