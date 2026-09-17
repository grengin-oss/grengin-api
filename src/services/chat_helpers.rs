// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

pub fn resolve_web_search_enabled(metadata: Option<&serde_json::Value>) -> bool {
    metadata
        .and_then(|value| value.get("webSearch"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

pub fn effective_native_web_search(requested: bool, model_supports_web_search: bool) -> bool {
    requested && model_supports_web_search
}

pub fn supports_native_web_search(
    provider_key: &str,
    plugin_model_support: bool,
    catalog_model_support: bool,
) -> bool {
    matches!(provider_key, "openai" | "anthropic" | "mistral" | "gemini")
        || plugin_model_support
        || catalog_model_support
}

pub fn effective_max_tokens(requested: Option<u32>, model_max: Option<u32>) -> Option<u32> {
    requested.map(|requested| model_max.map_or(requested, |model_max| requested.min(model_max)))
}

#[cfg(test)]
mod tests {
    use super::{effective_max_tokens, effective_native_web_search, supports_native_web_search};

    #[test]
    fn embedded_providers_keep_native_web_search_without_live_model_metadata() {
        for provider in ["openai", "anthropic", "mistral", "gemini"] {
            assert!(supports_native_web_search(provider, false, false));
        }
    }

    #[test]
    fn unsupported_provider_remains_disabled() {
        assert!(!supports_native_web_search("tinker", false, false));
        assert!(supports_native_web_search("custom", true, false));
        assert!(supports_native_web_search("custom", false, true));
    }

    #[test]
    fn native_web_search_requires_request_and_model_support() {
        assert!(effective_native_web_search(true, true));
        assert!(!effective_native_web_search(true, false));
        assert!(!effective_native_web_search(false, true));
        assert!(!effective_native_web_search(false, false));
    }

    #[test]
    fn omitted_max_tokens_uses_the_provider_default() {
        assert_eq!(effective_max_tokens(None, Some(128_000)), None);
    }

    #[test]
    fn requested_max_tokens_is_capped_by_the_model_limit() {
        assert_eq!(
            effective_max_tokens(Some(2_048), Some(128_000)),
            Some(2_048)
        );
        assert_eq!(effective_max_tokens(Some(8_192), Some(4_096)), Some(4_096));
        assert_eq!(effective_max_tokens(Some(2_048), None), Some(2_048));
    }
}
