// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use llm_plugin::{ProviderError, ProviderModel, ProviderPlugin};

use crate::dto::models::{ModelInfo, ModelType};

pub async fn find_provider_model(
    provider: &dyn ProviderPlugin,
    model_id: &str,
) -> Result<Option<ProviderModel>, ProviderError> {
    let Some(models) = provider.models() else {
        return Ok(None);
    };
    Ok(models
        .list_models()
        .await?
        .into_iter()
        .find(|model| model.id.as_str() == model_id || model.name == model_id))
}

pub fn model_type(model: &ProviderModel) -> ModelType {
    if let Some(model_type) = model
        .metadata
        .get("modelType")
        .and_then(|value| value.as_str())
    {
        return match model_type {
            "image_generator" => ModelType::ImageGenerator,
            "text_embedder" => ModelType::TextEmbedder,
            _ => ModelType::TextGenerator,
        };
    }
    if model.capabilities.image_generation && model.capabilities.chat.is_none() {
        ModelType::ImageGenerator
    } else if model.capabilities.embeddings && model.capabilities.chat.is_none() {
        ModelType::TextEmbedder
    } else {
        ModelType::TextGenerator
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
        image_output_token_rate: metadata_f64(&model, "imageOutputTokenRate"),
        cached_input_token_rate: metadata_f64(&model, "cachedInputTokenRate"),
        cache_creation_token_rate: metadata_f64(&model, "cacheCreationTokenRate"),
        max_output_tokens: metadata_i32(&model, "maxOutputTokens"),
        supports_streaming: chat.is_some_and(|chat| chat.streaming),
        supports_tools: chat.is_some_and(|chat| chat.tools),
        supports_vision: chat.is_some_and(|chat| chat.vision),
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
    fn explicit_model_type_wins_for_multi_capability_provider() {
        let model = ProviderModel {
            id: ModelId::new("image-1"),
            name: "Image 1".to_string(),
            capabilities: ProviderCapabilities {
                chat: Some(Default::default()),
                embeddings: true,
                image_generation: true,
                model_listing: true,
            },
            metadata: serde_json::json!({"modelType": "image_generator"}),
        };
        assert_eq!(model_type(&model), ModelType::ImageGenerator);
    }

    #[test]
    fn image_only_capability_is_classified_without_metadata() {
        let model = ProviderModel {
            id: ModelId::new("image-1"),
            name: "Image 1".to_string(),
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
            capabilities: ProviderCapabilities {
                chat: Some(Default::default()),
                ..Default::default()
            },
            metadata: serde_json::json!({
                "inputTokenRate": 1.0,
                "outputTokenRate": 5.0,
                "cachedInputTokenRate": 0.1,
                "cacheCreationTokenRate": 1.25,
                "maxOutputTokens": 64000
            }),
        };

        let info = to_model_info("custom-provider", model);
        assert_eq!(info.input_token_rate, Some(1.0));
        assert_eq!(info.output_token_rate, Some(5.0));
        assert_eq!(info.cached_input_token_rate, Some(0.1));
        assert_eq!(info.cache_creation_token_rate, Some(1.25));
        assert_eq!(info.max_output_tokens, Some(64000));
    }
}
