// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Serialize, Deserialize, Clone, ToSchema, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ModelType {
    TextGenerator,
    ImageGenerator,
    TextEmbedder,
}

#[derive(Serialize, ToSchema)]
pub struct ModelsResponse {
    pub providers: Vec<ProviderInfo>,
}

impl ModelsResponse {
    pub fn get_icons<S: Into<String>>(&self, provider: S) -> (Option<String>, Option<String>) {
        let provider_key = provider.into();
        let provider = self
            .providers
            .iter()
            .find(|provider| provider.key == provider_key);
        let icon = provider.map(|provider| provider.icon.clone());
        let icon_dark = provider.map(|provider| provider.icon_dark.clone());
        (icon, icon_dark)
    }

    pub fn default() -> Self {
        ModelsResponse {
            providers: Vec::new(),
        }
    }
}

#[derive(Serialize, Clone, ToSchema)]
pub struct ProviderInfo {
    pub key: String,
    pub name: String,
    pub icon: String,
    pub icon_dark: String,
    pub status: String,
    pub models: Vec<ModelInfo>,
}

#[derive(Serialize, Clone, ToSchema)]
pub struct ModelInfo {
    pub key: String,
    pub name: String,
    pub engine: String,
    pub model_type: ModelType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_token_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_token_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_input_token_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_cached_input_token_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_output_token_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_input_token_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_token_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<i32>,
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub supports_reasoning: bool,
    pub supports_vision: bool,
    pub supports_audio: bool,
    pub supports_pdf_native: bool,
    pub supports_web_search: bool,
    pub supports_multiple_images: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_images: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_per_image: Option<f64>,
}
