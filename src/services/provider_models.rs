// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use llm_plugin::{ProviderError, ProviderModel, ProviderModelType, ProviderPlugin};

use crate::{
    dto::models::{ModelInfo, ModelType, ProviderInfo},
    services::{
        live_models_cache::LiveModelsCache,
        model_picker::{catalog_recommended, catalog_specialty, catalog_summary},
        models_cache::get_model_info_cached,
        provider_chat::provider_error_class,
    },
    state::SharedState,
};

pub async fn find_provider_model(
    provider: &dyn ProviderPlugin,
    engine_key: &str,
    live_models_cache: &LiveModelsCache,
    model_id: &str,
) -> Result<Option<ProviderModel>, ProviderError> {
    let Some(models) = provider.models() else {
        return Ok(None);
    };
    // Static manifest models carry catalog pricing without any network call. Try them first
    // so rate lookups always work even when the live list-models API is unreachable or returns
    // different IDs (e.g. OpenAI's /v1/models returns "gpt-4o", not our "gpt-5.6-sol" alias).
    if let Some(model) = models
        .static_models()
        .into_iter()
        .find(|m| m.id.as_str() == model_id || m.name == model_id)
    {
        return Ok(Some(model));
    }
    Ok(live_models_cache
        .get_or_fetch(engine_key, provider)
        .await?
        .into_iter()
        .find(|model| model.id.as_str() == model_id || model.name == model_id))
}

// Same precedence as chat pricing: catalog entry first, then the provider plugin's own listing.
pub async fn resolve_model_info(
    state: &SharedState,
    provider: &dyn ProviderPlugin,
    provider_key: &str,
    model_id: &str,
) -> Option<ModelInfo> {
    match get_model_info_cached(&state.req_client, provider_key, model_id).await {
        Ok(Some(model)) => return Some(model),
        Ok(None) => {}
        Err(error) => eprintln!("models cache error: {error}"),
    }
    match find_provider_model(provider, provider_key, &state.live_models_cache, model_id).await {
        Ok(model) => model.map(|model| to_model_info(provider_key, model)),
        Err(error) => {
            eprintln!(
                "provider plugin model lookup failed: {}",
                provider_error_class(&error)
            );
            None
        }
    }
}

pub fn model_type(model: &ProviderModel) -> ModelType {
    match model.model_type {
        ProviderModelType::TextGenerator => ModelType::TextGenerator,
        ProviderModelType::TextEmbedder => ModelType::TextEmbedder,
        ProviderModelType::ImageGenerator => ModelType::ImageGenerator,
    }
}

// An engine's live models join its catalog entry, so each provider is listed once; the catalog
// entry keeps its icons and models, and only models it lacks are added.
pub fn add_provider_models(providers: &mut Vec<ProviderInfo>, provider: ProviderInfo) {
    let Some(listed) = providers
        .iter_mut()
        .find(|listed| listed.key.eq_ignore_ascii_case(&provider.key))
    else {
        providers.push(provider);
        return;
    };
    for model in provider.models {
        if !listed.models.iter().any(|known| known.key == model.key) {
            listed.models.push(model);
        }
    }
}

// Newest launch first; models without a known date follow in the order they were listed.
pub fn sort_models_newest_first(providers: &mut [ProviderInfo]) {
    for provider in providers {
        provider.models.sort_by(
            |left, right| match (&left.launch_date, &right.launch_date) {
                (Some(left), Some(right)) => right.cmp(left),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            },
        );
    }
}

pub fn is_chat_selectable_model(model: &ModelInfo) -> bool {
    model.model_type != ModelType::TextEmbedder
}

pub fn native_web_search_support(model: &ProviderModel) -> Option<bool> {
    model
        .metadata
        .get("supportsWebSearch")
        .map(|value| value.as_bool().unwrap_or(false))
}

pub fn to_model_info(provider_key: &str, model: ProviderModel) -> ModelInfo {
    let chat = model.capabilities.chat.as_ref();
    let resolved_model_type = model_type(&model);
    let comment = metadata_string(&model, "comment");
    ModelInfo {
        launch_date: crate::services::models_cache::catalog_launch_date(&model.metadata),
        key: model.id.to_string(),
        engine: provider_key.to_string(),
        model_type: resolved_model_type,
        specialty: catalog_specialty(&model.metadata),
        summaries: catalog_summary(&model.metadata),
        summary: None,
        recommended: catalog_recommended(&model.metadata),
        r#default: crate::services::model_picker::catalog_default(&model.metadata),
        launch_date: crate::services::model_picker::catalog_launch_date(&model.metadata),
        price_tier: None,
        comment,
        input_token_rate: metadata_f64(&model, "inputTokenRate"),
        output_token_rate: metadata_f64(&model, "outputTokenRate"),
        image_input_token_rate: metadata_f64(&model, "imageInputTokenRate"),
        image_cached_input_token_rate: metadata_f64(&model, "imageCachedInputTokenRate"),
        image_output_token_rate: metadata_f64(&model, "imageOutputTokenRate"),
        cached_input_token_rate: metadata_f64(&model, "cachedInputTokenRate"),
        cache_creation_token_rate: metadata_f64(&model, "cacheCreationTokenRate"),
        max_input_tokens: metadata_i32(&model, "maxInputTokens"),
        max_output_tokens: metadata_i32(&model, "maxOutputTokens"),
        supports_streaming: chat.is_some_and(|chat| chat.streaming),
        supports_tools: chat.is_some_and(|chat| chat.tools),
        supports_reasoning: chat.is_some_and(|chat| chat.reasoning),
        supports_vision: chat.is_some_and(|chat| chat.vision),
        supports_audio: metadata_bool(&model, "supportsAudio"),
        supports_pdf_native: metadata_bool(&model, "supportsPdfNative"),
        supports_web_search: metadata_bool(&model, "supportsWebSearch"),
        supports_multiple_images: metadata_bool(&model, "supportsMultipleImages"),
        max_images: metadata_i32(&model, "maxImages"),
        dimensions: metadata_i32(&model, "dimensions"),
        price_per_image: metadata_f64(&model, "pricePerImage"),
        name: model.name,
    }
}

fn metadata_string(model: &ProviderModel, key: &str) -> Option<String> {
    model.metadata.get(key)?.as_str().map(str::to_string)
}

fn metadata_f64(model: &ProviderModel, key: &str) -> Option<f64> {
    model.metadata.get(key)?.as_f64()
}

fn metadata_i32(model: &ProviderModel, key: &str) -> Option<i32> {
    model
        .metadata
        .get(key)?
        .as_i64()
        .and_then(|value| i32::try_from(value).ok())
}

fn metadata_bool(model: &ProviderModel, key: &str) -> bool {
    model
        .metadata
        .get(key)
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use llm_plugin::{ModelId, ProviderCapabilities, ProviderModel};

    use super::*;

    #[test]
    fn native_search_distinguishes_a_missing_flag_from_an_explicit_denial() {
        let mut model = ProviderModel {
            id: ModelId::new("fixture"),
            name: "Fixture".into(),
            model_type: ProviderModelType::TextGenerator,
            capabilities: ProviderCapabilities::default(),
            metadata: serde_json::json!({}),
        };
        assert_eq!(native_web_search_support(&model), None);
        for enabled in [true, false] {
            model.metadata = serde_json::json!({"supportsWebSearch": enabled});
            assert_eq!(native_web_search_support(&model), Some(enabled));
        }
        model.metadata = serde_json::json!({"supportsWebSearch": "false"});
        assert_eq!(native_web_search_support(&model), Some(false));
    }

    fn model_info(model_type: ModelType) -> ModelInfo {
        ModelInfo {
            launch_date: None,
            key: "model-1".to_string(),
            name: "Model 1".to_string(),
            engine: "example".to_string(),
            model_type,
            specialty: crate::services::model_picker::DEFAULT_SPECIALTY.to_string(),
            summaries: None,
            summary: None,
            recommended: false,
            r#default: false,
            launch_date: None,
            price_tier: None,
            comment: None,
            input_token_rate: None,
            output_token_rate: None,
            image_input_token_rate: None,
            image_cached_input_token_rate: None,
            image_output_token_rate: None,
            cached_input_token_rate: None,
            cache_creation_token_rate: None,
            max_input_tokens: None,
            max_output_tokens: None,
            supports_streaming: false,
            supports_tools: false,
            supports_reasoning: false,
            supports_vision: false,
            supports_audio: false,
            supports_pdf_native: false,
            supports_web_search: false,
            supports_multiple_images: false,
            max_images: None,
            dimensions: None,
            price_per_image: None,
        }
    }

    #[test]
    fn chat_model_endpoints_exclude_only_embedding_models() {
        assert!(is_chat_selectable_model(&model_info(
            ModelType::TextGenerator
        )));
        assert!(is_chat_selectable_model(&model_info(
            ModelType::ImageGenerator
        )));
        assert!(!is_chat_selectable_model(&model_info(
            ModelType::TextEmbedder
        )));
    }

    #[test]
    fn canonical_model_type_wins_for_multi_capability_provider() {
        let model = ProviderModel {
            id: ModelId::new("image-1"),
            name: "Image 1".to_string(),
            model_type: ProviderModelType::ImageGenerator,
            capabilities: ProviderCapabilities {
                chat: Some(Default::default()),
                embeddings: true,
                image_generation: true,
                model_listing: true,
            },
            metadata: serde_json::Value::Null,
        };
        assert_eq!(model_type(&model), ModelType::ImageGenerator);
    }

    #[test]
    fn image_generator_type_maps_without_metadata() {
        let model = ProviderModel {
            id: ModelId::new("image-1"),
            name: "Image 1".to_string(),
            model_type: ProviderModelType::ImageGenerator,
            capabilities: ProviderCapabilities {
                image_generation: true,
                ..Default::default()
            },
            metadata: serde_json::Value::Null,
        };
        assert_eq!(model_type(&model), ModelType::ImageGenerator);
    }

    #[test]
    fn plugin_model_pricing_maps_every_token_bucket() {
        let model = ProviderModel {
            id: ModelId::new("priced-model"),
            name: "Priced model".to_string(),
            model_type: ProviderModelType::TextGenerator,
            capabilities: ProviderCapabilities {
                chat: Some(llm_plugin::ChatCapabilities {
                    streaming: true,
                    tools: true,
                    vision: true,
                    reasoning: true,
                }),
                ..Default::default()
            },
            metadata: serde_json::json!({
                "inputTokenRate": 1.0,
                "outputTokenRate": 5.0,
                "imageInputTokenRate": 2.0,
                "imageCachedInputTokenRate": 0.2,
                "imageOutputTokenRate": 8.0,
                "cachedInputTokenRate": 0.1,
                "cacheCreationTokenRate": 1.25,
                "maxInputTokens": 128000,
                "maxOutputTokens": 64000,
                "specialty": "coding",
                "summary": {"en": "Writes code for the team", "ko": "팀을 위한 코드 작성"}
            }),
        };

        let info = to_model_info("custom-provider", model);
        assert_eq!(
            info.specialty, "coding",
            "a plugin may name its own specialty"
        );
        assert_eq!(
            info.summaries
                .as_ref()
                .and_then(|summary| summary.get("ko"))
                .map(String::as_str),
            Some("팀을 위한 코드 작성")
        );
        assert_eq!(info.input_token_rate, Some(1.0));
        assert_eq!(info.output_token_rate, Some(5.0));
        assert_eq!(info.cached_input_token_rate, Some(0.1));
        assert_eq!(info.cache_creation_token_rate, Some(1.25));
        assert_eq!(info.image_input_token_rate, Some(2.0));
        assert_eq!(info.image_cached_input_token_rate, Some(0.2));
        assert_eq!(info.image_output_token_rate, Some(8.0));
        assert_eq!(info.max_input_tokens, Some(128000));
        assert_eq!(info.max_output_tokens, Some(64000));
        assert!(info.supports_reasoning);
    }

    #[test]
    fn live_models_join_their_catalog_entry_and_every_list_is_newest_first() {
        let model = |key: &str, date: Option<&str>| ModelInfo {
            key: key.to_string(),
            launch_date: date.map(str::to_string),
            ..model_info(ModelType::TextGenerator)
        };
        let provider = |key: &str, icon: &str, models: Vec<ModelInfo>| ProviderInfo {
            key: key.to_string(),
            name: key.to_string(),
            icon: icon.to_string(),
            icon_dark: String::new(),
            status: "enabled".to_string(),
            models,
        };
        let mut providers = vec![provider(
            "openai",
            "openai.svg",
            vec![
                model("gpt-6.1-sol", Some("2026-09-29")),
                model("gpt-6.1-terra", Some("2026-10-26")),
            ],
        )];
        let live = vec![
            model("ft-custom", None),
            model("gpt-6.1-sol", Some("2026-09-29")),
            model("gpt-6.2-astra", Some("2026-11-17")),
        ];
        add_provider_models(&mut providers, provider("OpenAI", "", live));
        let gateway = vec![
            model("openai/gpt-5.2", Some("2025-12-11")),
            model("zz/unknown", None),
            model("anthropic/claude-opus-5-5", Some("2026-11-01")),
            model("aa/unknown", None),
        ];
        add_provider_models(&mut providers, provider("grengin", "", gateway));

        sort_models_newest_first(&mut providers);

        let keys = |provider: &ProviderInfo| {
            provider
                .models
                .iter()
                .map(|model| model.key.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(providers.len(), 2);
        assert_eq!(providers[0].icon, "openai.svg");
        assert_eq!(
            keys(&providers[0]),
            ["gpt-6.2-astra", "gpt-6.1-terra", "gpt-6.1-sol", "ft-custom"]
        );
        assert_eq!(
            keys(&providers[1]),
            [
                "anthropic/claude-opus-5-5",
                "openai/gpt-5.2",
                "zz/unknown",
                "aa/unknown"
            ]
        );
        assert_eq!(
            crate::services::models_cache::catalog_launch_date(
                &serde_json::json!({"launchDate": " 2026-11-17 "})
            )
            .as_deref(),
            Some("2026-11-17")
        );
        let json = serde_json::to_value(&providers[0].models[0]).unwrap();
        assert!(json.get("launch_date").is_none());
    }
}
