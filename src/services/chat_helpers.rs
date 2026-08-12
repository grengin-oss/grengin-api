// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use llm_plugin::ProviderPlugin;

pub enum LlmProviderConfig {
    OpenAI(crate::config::setting::OpenaiSettings),
    Anthropic(crate::config::setting::AnthropicSettings),
    Mistral(crate::config::setting::MistralSettings),
    Gemini(crate::config::setting::GeminiSettings),
    Plugin(Arc<dyn ProviderPlugin>),
}

pub fn resolve_web_search_enabled(metadata: Option<&serde_json::Value>) -> bool {
    metadata
        .and_then(|value| value.get("webSearch"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}
