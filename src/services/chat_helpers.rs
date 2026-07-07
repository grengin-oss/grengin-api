pub fn resolve_web_search_enabled(metadata: Option<&serde_json::Value>) -> bool {
    metadata
        .and_then(|value| value.get("webSearch"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}
