// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use llm_plugin::{ProviderError, ProviderModel, ProviderModelType, ProviderPlugin};

use crate::{
    dto::models::{ModelInfo, ModelType},
    services::live_models_cache::LiveModelsCache,
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

pub fn model_type(model: &ProviderModel) -> ModelType {
    match model.model_type {
        ProviderModelType::TextGenerator => ModelType::TextGenerator,
        ProviderModelType::TextEmbedder => ModelType::TextEmbedder,
        ProviderModelType::ImageGenerator => ModelType::ImageGenerator,
    }
}

pub fn to_model_info(provider_key: &str, model: ProviderModel) -> ModelInfo {
    let chat = model.capabilities.chat.as_ref();
    let resolved_model_type = model_type(&model);
    let comment = metadata_string(&model, "comment");
    ModelInfo {
        key: model.id.to_string(),
        engine: provider_key.to_string(),
        model_type: resolved_model_type,
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
                "maxOutputTokens": 64000
            }),
        };

        let info = to_model_info("custom-provider", model);
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
}
