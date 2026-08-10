// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{collections::BTreeMap, env};

use futures_util::StreamExt;
use grengin_provider::{
    ChatMessage, ChatRequest, ChatRole, ContentPart, DeclarativeProvider, ModelId, ProviderEvent,
    ProviderManifestV1, ProviderPlugin, ProviderRuntimeConfig,
};
use serde_json::Value;

struct LiveProvider {
    name: &'static str,
    key_env: &'static str,
    base_url: &'static str,
    model: &'static str,
    manifest: &'static [u8],
}

const OPENAI_COMPATIBLE: &[u8] = include_bytes!("../examples/openai-compatible.provider.json");
const ANTHROPIC: &[u8] = include_bytes!("../examples/anthropic.provider.json");

fn enabled() -> bool {
    env::var("GRENGIN_LIVE_PROVIDER_TESTS").as_deref() == Ok("1")
}

async fn smoke(provider: LiveProvider) {
    if !enabled() {
        eprintln!(
            "skipping {}: GRENGIN_LIVE_PROVIDER_TESTS is not enabled",
            provider.name
        );
        return;
    }
    let Some(api_key) = env::var(provider.key_env)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!(
            "skipping {}: {} is not configured",
            provider.name, provider.key_env
        );
        return;
    };
    let model = provider.model;
    let mut manifest = ProviderManifestV1::from_json(provider.manifest).unwrap();
    manifest.base_url = provider.base_url.to_string();
    let runtime = ProviderRuntimeConfig {
        credentials: BTreeMap::from([("api_key".to_string(), api_key)]),
        default_timeout_ms: 30_000,
        max_response_bytes: 2 * 1024 * 1024,
        ..Default::default()
    };
    let provider = DeclarativeProvider::new(manifest, runtime).unwrap();
    let mut session = provider
        .chat()
        .unwrap()
        .start(ChatRequest {
            model: ModelId::new(model),
            messages: vec![ChatMessage {
                role: ChatRole::User,
                content: vec![ContentPart::Text {
                    text: "Reply with OK.".to_string(),
                }],
                tool_calls: Vec::new(),
                tool_result: None,
            }],
            temperature: Some(0.0),
            max_tokens: Some(16),
            tools: Vec::new(),
            tool_choice: None,
            options: Value::Null,
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{} chat setup failed: {}",
                provider.descriptor().id,
                error_class(&error)
            )
        });
    let mut stream = session.stream().await.unwrap_or_else(|error| {
        panic!(
            "{} request failed: {}",
            provider.descriptor().id,
            error_class(&error)
        )
    });
    let mut received_text = false;
    let mut completed = false;
    while let Some(event) = stream.next().await {
        let event = event.unwrap_or_else(|error| {
            panic!(
                "{} stream failed: {}",
                provider.descriptor().id,
                error_class(&error)
            )
        });
        received_text |= matches!(event, ProviderEvent::TextDelta { .. });
        completed |= matches!(event, ProviderEvent::Completed { .. });
    }
    assert!(
        received_text,
        "{} emitted no text",
        provider.descriptor().id
    );
    assert!(
        completed,
        "{} emitted no completion",
        provider.descriptor().id
    );
}

fn error_class(error: &grengin_provider::ProviderError) -> &'static str {
    match error {
        grengin_provider::ProviderError::InvalidManifest(_) => "invalid_manifest",
        grengin_provider::ProviderError::Configuration(_) => "configuration",
        grengin_provider::ProviderError::MissingCredential(_) => "missing_credential",
        grengin_provider::ProviderError::UnsupportedCapability(_) => "unsupported_capability",
        grengin_provider::ProviderError::PayloadMapping(_) => "payload_mapping",
        grengin_provider::ProviderError::ResponseMapping(_) => "response_mapping",
        grengin_provider::ProviderError::UrlNotAllowed(_) => "url_not_allowed",
        grengin_provider::ProviderError::HeaderNotAllowed(_) => "header_not_allowed",
        grengin_provider::ProviderError::Transport(_) => "transport",
        grengin_provider::ProviderError::HttpStatus { .. } => "http_status",
        grengin_provider::ProviderError::QuotaExhausted => "quota_exhausted",
        grengin_provider::ProviderError::PaymentRequired => "payment_required",
        grengin_provider::ProviderError::StreamEnded => "stream_ended",
        grengin_provider::ProviderError::Cancelled => "cancelled",
        grengin_provider::ProviderError::ResponseTooLarge => "response_too_large",
    }
}

macro_rules! live_test {
    ($name:ident, $display:literal, $key:literal, $base:literal, $model:literal, $manifest:expr) => {
        #[tokio::test]
        #[ignore = "requires GRENGIN_LIVE_PROVIDER_TESTS=1 and a provider credential"]
        async fn $name() {
            let provider = LiveProvider {
                name: $display,
                key_env: $key,
                base_url: $base,
                model: $model,
                manifest: $manifest,
            };
            smoke(provider).await;
        }
    };
}

live_test!(
    openai_chat_smoke,
    "OpenAI",
    "OPENAI_API_KEY",
    "https://api.openai.com/v1/",
    "gpt-4o-mini",
    OPENAI_COMPATIBLE
);
live_test!(
    groq_chat_smoke,
    "Groq",
    "GROQ_API_KEY",
    "https://api.groq.com/openai/v1/",
    "llama-3.1-8b-instant",
    OPENAI_COMPATIBLE
);
live_test!(
    openrouter_chat_smoke,
    "OpenRouter",
    "OPEN_ROUTER_API_KEY",
    "https://openrouter.ai/api/v1/",
    "openrouter/auto",
    OPENAI_COMPATIBLE
);
live_test!(
    cerebras_chat_smoke,
    "Cerebras",
    "CEREBRAS_API_KEY",
    "https://api.cerebras.ai/v1/",
    "gpt-oss-120b",
    OPENAI_COMPATIBLE
);
live_test!(
    deepseek_chat_smoke,
    "DeepSeek",
    "DEEPSEEK_API_KEY",
    "https://api.deepseek.com/v1/",
    "deepseek-v4-flash",
    OPENAI_COMPATIBLE
);
live_test!(
    mistral_chat_smoke,
    "Mistral",
    "MISTRAL_API_KEY",
    "https://api.mistral.ai/v1/",
    "mistral-small-latest",
    OPENAI_COMPATIBLE
);
live_test!(
    gemini_openai_compatible_chat_smoke,
    "Gemini",
    "GEMINI_API_KEY",
    "https://generativelanguage.googleapis.com/v1beta/openai/",
    "gemini-2.0-flash",
    OPENAI_COMPATIBLE
);
live_test!(
    anthropic_chat_smoke,
    "Anthropic",
    "ANTHROPIC_API_KEY",
    "https://api.anthropic.com/v1/",
    "claude-haiku-4-5-20251001",
    ANTHROPIC
);
