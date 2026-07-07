use serde_json::{Value, json};
use uuid::Uuid;

use crate::dto::chat_stream::{
    ArtifactStreamEvent, ChatStream, ChatStreamToolCall, ChatStreamToolInput, ChatStreamToolResult,
    ChatToolKind,
};

pub const ARTIFACT_TOOL_NAME: &str = "create_artifact";

pub const ARTIFACT_TOOL_DESC: &str = "Use this tool to output a standalone document — \
a complete HTML page, a full Markdown file, or a long code file — \
that is best viewed separately from your conversational reply. \
Do not repeat the artifact content in your reply.";

pub const ARTIFACT_SYSTEM_HINT: &str = "When the user asks you to produce a standalone HTML page, \
Markdown document, or a complete self-contained code file, always call the \
`create_artifact` tool instead of writing the content inline in your reply.";

pub fn artifact_tool_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "title":       { "type": "string", "description": "Short descriptive title for the artifact" },
            "contentType": { "type": "string", "enum": ["text/html", "text/markdown"], "description": "MIME type of the content" },
            "content":     { "type": "string", "description": "Full content of the artifact" }
        },
        "required": ["title", "contentType", "content"]
    })
}

pub struct ArtifactEmit {
    pub tool_call_event: ChatStream,
    pub artifact_event: ArtifactStreamEvent,
    pub tool_result_event: ChatStream,
}

/// Build the two SSE events for an artifact emission from parsed tool args.
/// Returns `None` when content is empty (Anthropic's initial empty ToolCall block).
pub fn build_artifact_emit(args: &Value, tool_id: Option<String>) -> Option<ArtifactEmit> {
    let content = args["content"].as_str().unwrap_or("").to_string();
    if content.is_empty() {
        return None;
    }
    let title = args["title"].as_str().unwrap_or("Untitled").to_string();
    let content_type = args["contentType"].as_str().unwrap_or("text/html").to_string();

    let tool_call_event = ChatStream {
        id: None,
        title: None,
        message_id: None,
        is_new: None,
        content: None,
        input_tokens: None,
        output_tokens: None,
        latency_ms: None,
        cost: None,
        event: None,
        tool_call: Some(ChatStreamToolCall {
            tool_name: ARTIFACT_TOOL_NAME.to_string(),
            tool_id: tool_id.clone(),
            input_text: None,
            input: Some(ChatStreamToolInput::Json {
                value: serde_json::json!({
                    "title": title,
                    "contentType": content_type,
                }),
            }),
            kind: Some(ChatToolKind::Other),
            web_search: None,
        }),
        tool_result: None,
    };

    let artifact_id = Uuid::new_v4().to_string();
    let artifact_event = ArtifactStreamEvent {
        id: artifact_id.clone(),
        title: title.clone(),
        content_type: content_type.clone(),
        content,
    };

    let tool_result = ChatStreamToolResult {
        tool_name: Some(ARTIFACT_TOOL_NAME.to_string()),
        tool_id: tool_id.clone(),
        kind: Some(ChatToolKind::Other),
        status: Some("success".to_string()),
        output: Some(json!({ "artifactId": artifact_id, "title": title, "contentType": content_type })),
        web_search: None,
    };
    let tool_result_event = ChatStream {
        id: None,
        title: None,
        message_id: None,
        is_new: None,
        content: None,
        input_tokens: None,
        output_tokens: None,
        latency_ms: None,
        cost: None,
        event: None,
        tool_call: None,
        tool_result: Some(tool_result),
    };

    Some(ArtifactEmit { tool_call_event, artifact_event, tool_result_event })
}
