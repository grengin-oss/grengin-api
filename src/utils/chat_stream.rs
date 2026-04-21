use crate::{
    dto::chat_stream::{
        ChatStreamToolInput, ChatStreamWebSearchResult, ChatStreamWebSearchResultItem,
    },
    handlers::llm::{StreamWebSearchState, ToolInput},
};
use serde_json::Value;

pub fn to_chat_web_search_result(state: &StreamWebSearchState) -> ChatStreamWebSearchResult {
    ChatStreamWebSearchResult {
        query: state.query.clone(),
        queries: state.queries.clone(),
        results: state
            .results
            .iter()
            .map(|result| ChatStreamWebSearchResultItem {
                title: result.title.clone(),
                url: result.url.clone(),
                source: result.source.clone(),
                page_age: result.page_age.clone(),
                snippet: result.snippet.clone(),
            })
            .collect(),
    }
}

pub fn to_chat_tool_input(input: &ToolInput) -> ChatStreamToolInput {
    match input {
        ToolInput::Text(text) => ChatStreamToolInput::Text { text: text.clone() },
        ToolInput::Json(value) => ChatStreamToolInput::Json {
            value: value.clone(),
        },
    }
}

pub fn tool_input_to_value(input: Option<&ToolInput>) -> Value {
    match input {
        Some(ToolInput::Json(value)) => value.clone(),
        Some(ToolInput::Text(text)) => {
            serde_json::from_str::<Value>(text).unwrap_or(Value::String(text.clone()))
        }
        None => Value::Null,
    }
}

pub fn output_indicates_error(output: &Value) -> bool {
    match output {
        Value::Object(map) => {
            if map
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return true;
            }
            if map.get("isError").and_then(Value::as_bool).unwrap_or(false) {
                return true;
            }
            if map.get("success").and_then(Value::as_bool) == Some(false) {
                return true;
            }
            if map.get("ok").and_then(Value::as_bool) == Some(false) {
                return true;
            }
            if let Some(status) = map.get("status").and_then(Value::as_str) {
                let status = status.to_ascii_lowercase();
                if status == "error" || status == "failed" {
                    return true;
                }
            }
            if let Some(err) = map.get("error") {
                if !err.is_null() {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

pub fn tool_result_status_from_output(output: &Option<Value>) -> Option<String> {
    let is_error = output.as_ref().map(output_indicates_error).unwrap_or(false);
    Some(if is_error { "error" } else { "success" }.to_string())
}
