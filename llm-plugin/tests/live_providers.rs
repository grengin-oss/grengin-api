// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{collections::BTreeMap, env};

use futures_util::StreamExt;
use llm_plugin::{
    ChatMessage, ChatRequest, ChatRole, ContentPart, DeclarativeProvider, EmbeddingRequest,
    ModelId, ProviderEvent, ProviderManifestV1, ProviderPlugin, ProviderRuntimeConfig,
};
use serde_json::Value;

struct LiveProvider {
    name: &'static str,
    key_env: &'static str,
    base_url: Option<&'static str>,
    model: &'static str,
    manifest: Vec<u8>,
}

const OPENAI_COMPATIBLE: &[u8] = include_bytes!("../examples/openai-compatible.provider.json");

fn enabled() -> bool {
    env::var("GRENGIN_LIVE_PROVIDER_TESTS").as_deref() == Ok("1")
}

fn embeddings_enabled() -> bool {
    env::var("GRENGIN_LIVE_EMBEDDING_TESTS").as_deref() == Ok("1")
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
    let mut manifest = ProviderManifestV1::from_json(&provider.manifest).unwrap();
    if let Some(base_url) = provider.base_url {
        manifest.base_url = base_url.to_string();
    }
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
            messages: vec![
                ChatMessage {
                    role: ChatRole::System,
                    content: vec![ContentPart::Text {
                        text: "Be concise.".to_string(),
                    }],
                    tool_calls: Vec::new(),
                    tool_result: None,
                },
                ChatMessage {
                    role: ChatRole::User,
                    content: vec![ContentPart::Text {
                        text: "Reply with OK.".to_string(),
                    }],
                    tool_calls: Vec::new(),
                    tool_result: None,
                },
            ],
            temperature: Some(0.0),
            max_tokens: Some(16),
            tools: Vec::new(),
            tool_choice: None,
            web_search: false,
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
    let mut input_tokens = None;
    let mut output_tokens = None;
    let mut total_tokens = None;
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
        if let ProviderEvent::Usage { usage } = event {
            input_tokens = usage.input_tokens.or(input_tokens);
            output_tokens = usage.output_tokens.or(output_tokens);
            total_tokens = usage.total_tokens.or(total_tokens);
        }
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
    assert!(
        input_tokens.is_some_and(|tokens| tokens > 0),
        "{} emitted no positive input token usage",
        provider.descriptor().id
    );
    assert!(
        output_tokens.is_some_and(|tokens| tokens > 0),
        "{} emitted no positive output token usage",
        provider.descriptor().id
    );
    assert!(
        total_tokens.is_none_or(|tokens| {
            tokens >= input_tokens.unwrap_or(0) + output_tokens.unwrap_or(0)
        }),
        "{} emitted invalid total token usage",
        provider.descriptor().id
    );
}

async fn embedding_smoke(provider: LiveProvider) {
    if !embeddings_enabled() {
        eprintln!(
            "skipping {} embeddings: GRENGIN_LIVE_EMBEDDING_TESTS is not enabled",
            provider.name
        );
        return;
    }
    let Some(api_key) = env::var(provider.key_env)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!(
            "skipping {} embeddings: {} is not configured",
            provider.name, provider.key_env
        );
        return;
    };
    let model = provider.model;
    let mut manifest = ProviderManifestV1::from_json(&provider.manifest).unwrap();
    if let Some(base_url) = provider.base_url {
        manifest.base_url = base_url.to_string();
    }
    let runtime = ProviderRuntimeConfig {
        credentials: BTreeMap::from([("api_key".to_string(), api_key)]),
        default_timeout_ms: 30_000,
        max_response_bytes: 8 * 1024 * 1024,
        ..Default::default()
    };
    let provider = DeclarativeProvider::new(manifest, runtime).unwrap();
    let embedder = provider
        .embeddings()
        .expect("OpenAI-compatible manifest must provide embeddings");
    let result = embedder
        .embed(EmbeddingRequest {
            model: ModelId::new(model),
            inputs: vec![
                "Grengin embedding provider smoke test".to_string(),
                "A second input verifies batch ordering".to_string(),
            ],
            dimensions: None,
            options: Value::Null,
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{} embedding request failed: {}",
                provider.descriptor().id,
                error_class(&error)
            )
        });

    assert_eq!(result.vectors.len(), 2, "embedding batch size changed");
    let dimensions = result.vectors[0].len();
    assert!(dimensions > 0, "provider returned an empty embedding");
    assert!(
        result
            .vectors
            .iter()
            .all(|vector| vector.len() == dimensions),
        "provider returned inconsistent embedding dimensions"
    );
    assert!(
        result
            .vectors
            .iter()
            .flatten()
            .all(|value| value.is_finite()),
        "provider returned a non-finite embedding value"
    );
    assert!(
        result.vectors.iter().flatten().any(|value| *value != 0.0),
        "provider returned only zero values"
    );
    if let Some(usage) = result.usage {
        assert!(
            usage.total_tokens.is_none_or(|tokens| tokens > 0),
            "provider returned invalid embedding token usage"
        );
    }
}

fn error_class(error: &llm_plugin::ProviderError) -> &'static str {
    match error {
        llm_plugin::ProviderError::InvalidManifest(_) => "invalid_manifest",
        llm_plugin::ProviderError::Configuration(_) => "configuration",
        llm_plugin::ProviderError::MissingCredential(_) => "missing_credential",
        llm_plugin::ProviderError::UnsupportedCapability(_) => "unsupported_capability",
        llm_plugin::ProviderError::PayloadMapping(_) => "payload_mapping",
        llm_plugin::ProviderError::ResponseMapping(_) => "response_mapping",
        llm_plugin::ProviderError::UrlNotAllowed(_) => "url_not_allowed",
        llm_plugin::ProviderError::HeaderNotAllowed(_) => "header_not_allowed",
        llm_plugin::ProviderError::Transport(_) => "transport",
        llm_plugin::ProviderError::HttpStatus { .. } => "http_status",
        llm_plugin::ProviderError::QuotaExhausted => "quota_exhausted",
        llm_plugin::ProviderError::PaymentRequired => "payment_required",
        llm_plugin::ProviderError::StreamEnded => "stream_ended",
        llm_plugin::ProviderError::Cancelled => "cancelled",
        llm_plugin::ProviderError::ResponseTooLarge => "response_too_large",
    }
}

#[tokio::test]
#[ignore = "requires GRENGIN_LIVE_PROVIDER_TESTS=1, GRENGIN_PROVIDER_CATALOG_DIR, and OPEN_ROUTER_API_KEY"]
async fn openrouter_web_search_smoke() {
    if !enabled() {
        return;
    }
    let Some(api_key) = env::var("OPEN_ROUTER_API_KEY")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("skipping OpenRouter web search: OPEN_ROUTER_API_KEY is not configured");
        return;
    };
    let catalog = env::var("GRENGIN_PROVIDER_CATALOG_DIR")
        .expect("set GRENGIN_PROVIDER_CATALOG_DIR to master-data/providers");
    let manifest = std::fs::read(format!("{catalog}/openrouter/plugin.json"))
        .expect("OpenRouter catalog manifest");
    let manifest = ProviderManifestV1::from_json(&manifest).unwrap();
    let provider = DeclarativeProvider::new(
        manifest,
        ProviderRuntimeConfig {
            credentials: BTreeMap::from([("api_key".to_string(), api_key)]),
            default_timeout_ms: 60_000,
            max_response_bytes: 2 * 1024 * 1024,
            ..Default::default()
        },
    )
    .unwrap();
    let mut session = provider
        .chat()
        .unwrap()
        .start(ChatRequest {
            model: ModelId::new("nvidia/nemotron-3-ultra-550b-a55b:free"),
            messages: vec![ChatMessage {
                role: ChatRole::User,
                content: vec![ContentPart::Text {
                    text: "What is the current stable Rust version? Cite one source.".to_string(),
                }],
                tool_calls: Vec::new(),
                tool_result: None,
            }],
            temperature: Some(0.0),
            max_tokens: Some(96),
            tools: Vec::new(),
            tool_choice: None,
            web_search: true,
            options: Value::Null,
        })
        .await
        .unwrap();
    let mut stream = session.stream().await.unwrap_or_else(|error| {
        panic!(
            "OpenRouter web-search request failed: {}",
            error_class(&error)
        )
    });
    let mut received_text = false;
    let mut completed = false;
    let mut citation_count = 0usize;
    while let Some(event) = stream.next().await {
        match event.unwrap_or_else(|error| {
            panic!(
                "OpenRouter web-search stream failed: {}",
                error_class(&error)
            )
        }) {
            ProviderEvent::TextDelta { .. } => received_text = true,
            ProviderEvent::ServerToolResult { results, .. } => {
                citation_count += results
                    .iter()
                    .filter(|result| !result.url.trim().is_empty())
                    .count();
            }
            ProviderEvent::Completed { .. } => completed = true,
            _ => {}
        }
    }
    assert!(received_text, "OpenRouter web search emitted no text");
    assert!(completed, "OpenRouter web search emitted no completion");
    assert!(
        citation_count > 0,
        "OpenRouter web search emitted no citations"
    );
}

macro_rules! live_test {
    ($name:ident, $display:literal, $key:literal, $base:literal, $model:literal, $manifest:expr) => {
        #[tokio::test]
        #[ignore = "requires GRENGIN_LIVE_PROVIDER_TESTS=1 and a provider credential"]
        async fn $name() {
            let provider = LiveProvider {
                name: $display,
                key_env: $key,
                base_url: Some($base),
                model: $model,
                manifest: $manifest.to_vec(),
            };
            smoke(provider).await;
        }
    };
}

macro_rules! catalog_live_test {
    ($name:ident, $display:literal, $key:literal, $provider:literal, $model:literal) => {
        #[tokio::test]
        #[ignore = "requires GRENGIN_LIVE_PROVIDER_TESTS=1, GRENGIN_PROVIDER_CATALOG_DIR, and a provider credential"]
        async fn $name() {
            let catalog = env::var("GRENGIN_PROVIDER_CATALOG_DIR")
                .expect("set GRENGIN_PROVIDER_CATALOG_DIR to master-data/providers");
            let manifest = std::fs::read(format!("{catalog}/{}/plugin.json", $provider))
                .expect("catalog provider manifest");
            let provider = LiveProvider {
                name: $display,
                key_env: $key,
                base_url: None,
                model: $model,
                manifest,
            };
            smoke(provider).await;
        }
    };
}

macro_rules! live_embedding_test {
    ($name:ident, $display:literal, $key:literal, $base:literal, $model:literal) => {
        #[tokio::test]
        #[ignore = "requires GRENGIN_LIVE_EMBEDDING_TESTS=1 and a provider credential"]
        async fn $name() {
            let provider = LiveProvider {
                name: $display,
                key_env: $key,
                base_url: Some($base),
                model: $model,
                manifest: OPENAI_COMPATIBLE.to_vec(),
            };
            embedding_smoke(provider).await;
        }
    };
}

catalog_live_test!(
    openai_chat_smoke,
    "OpenAI",
    "OPENAI_API_KEY",
    "openai",
    "gpt-5.4-nano"
);
live_test!(
    groq_chat_smoke,
    "Groq",
    "GROQ_API_KEY",
    "https://api.groq.com/openai/v1/",
    "llama-3.1-8b-instant",
    OPENAI_COMPATIBLE
);
catalog_live_test!(
    openrouter_chat_smoke,
    "OpenRouter",
    "OPEN_ROUTER_API_KEY",
    "openrouter",
    "nvidia/nemotron-3-ultra-550b-a55b:free"
);
live_test!(
    huggingface_chat_smoke,
    "HuggingFace",
    "HF_TOKEN",
    "https://router.huggingface.co/v1/",
    "Qwen/Qwen3-32B",
    OPENAI_COMPATIBLE
);
live_test!(
    azure_openai_chat_smoke,
    "Azure OpenAI",
    "AZURE_OPENAI_API_KEY",
    "https://south-india-default-resource.services.ai.azure.com/openai/v1/",
    "gpt-5-nano",
    OPENAI_COMPATIBLE
);
live_test!(
    orca_router_chat_smoke,
    "OrcaRouter",
    "ORCA_API_KEY",
    "https://api.orcarouter.ai/v1/",
    "orcarouter/free",
    OPENAI_COMPATIBLE
);
live_test!(
    bedrock_chat_smoke,
    "AWS Bedrock",
    "AWS_BEARER_TOKEN_BEDROCK",
    "https://bedrock-runtime.us-east-1.amazonaws.com/openai/v1/",
    "openai.gpt-oss-20b-1:0",
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
catalog_live_test!(
    mistral_chat_smoke,
    "Mistral",
    "MISTRAL_API_KEY",
    "mistral",
    "mistral-small-2603"
);
catalog_live_test!(
    gemini_openai_compatible_chat_smoke,
    "Gemini",
    "GEMINI_API_KEY",
    "gemini",
    "gemini-3.1-flash-lite"
);
catalog_live_test!(
    anthropic_chat_smoke,
    "Anthropic",
    "ANTHROPIC_API_KEY",
    "anthropic",
    "claude-haiku-4-5"
);

live_embedding_test!(
    openai_embedding_smoke,
    "OpenAI",
    "OPENAI_API_KEY",
    "https://api.openai.com/v1/",
    "text-embedding-3-small"
);
live_embedding_test!(
    gemini_embedding_smoke,
    "Gemini",
    "GEMINI_API_KEY",
    "https://generativelanguage.googleapis.com/v1beta/openai/",
    "gemini-embedding-001"
);
live_embedding_test!(
    mistral_embedding_smoke,
    "Mistral",
    "MISTRAL_API_KEY",
    "https://api.mistral.ai/v1/",
    "mistral-embed"
);
