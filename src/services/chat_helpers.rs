pub enum LlmProviderConfig {
    OpenAI(crate::config::setting::OpenaiSettings),
    Anthropic(crate::config::setting::AnthropicSettings),
    Mistral(crate::config::setting::MistralSettings),
    Gemini(crate::config::setting::GeminiSettings),
}

pub fn resolve_web_search_enabled(metadata: Option<&serde_json::Value>) -> bool {
    metadata
        .and_then(|value| value.get("webSearch"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}
