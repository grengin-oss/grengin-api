// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    dto::models::ModelType,
    models::ai_engines::{ApiKeyStatus, PluginConfig},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AIEngineCreate {
    pub display_name: Option<String>,
    pub api_key: Option<String>,
    #[serde(default)]
    pub is_enabled: bool,
    #[serde(default)]
    pub whitelisted_models: Vec<String>,
    pub default_model: Option<String>,
    pub default_image_gen_model: Option<String>,
    pub plugin_config: PluginConfig,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AIEngineUpdate {
    pub display_name: Option<String>,
    pub is_enabled: Option<bool>,
    pub api_key: Option<String>,
    pub whitelisted_models: Option<Vec<String>>,
    pub default_model: Option<String>,
    pub default_image_gen_model: Option<String>,
    pub plugin_config: Option<PluginConfig>,
}

#[derive(Serialize, ToSchema)]
pub struct AIEngineDetail {
    pub icon: Option<String>,
    pub icon_dark: Option<String>,
    pub engine_key: String,
    pub plugin_version: Option<String>,
    pub display_name: String,
    pub is_enabled: bool,
    pub api_key_configured: bool,
    pub api_key_status: ApiKeyStatus,
    pub api_key_preview: Option<String>,
    pub api_key_last_validated_at: Option<DateTime<Utc>>,
    pub whitelisted_models: Vec<String>,
    pub default_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_image_gen_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_config: Option<PluginConfig>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AIEnginePluginValidationRequest {
    pub plugin_config: PluginConfig,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AIEnginePluginValidationResponse {
    pub valid: bool,
    pub engine_key: Option<String>,
    pub version: Option<String>,
    pub name: Option<String>,
    pub destination: Option<String>,
    #[schema(value_type = Object)]
    pub capabilities: Option<serde_json::Value>,
    pub credential_required: bool,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct AIEngineConnectionTest {
    pub valid: bool,
    pub mode: String,
    pub models_available: Option<usize>,
    pub error_class: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct AIEngineValidation {
    pub valid: bool,
    pub message: String,
    pub models_available: i64,
}

#[derive(Serialize, ToSchema)]
pub struct AIEngineModels {
    pub models: Vec<AiModel>,
}

#[derive(Serialize, ToSchema)]
pub struct AiModel {
    pub model_id: String,
    pub display_name: String,
    pub model_type: ModelType,
    pub is_whitelisted: bool,
    pub capabilities: AiModelCapabilities,
    pub comment: Option<String>,
    pub input_token_rate: Option<f64>,
    pub output_token_rate: Option<f64>,
    pub image_input_token_rate: Option<f64>,
    pub image_cached_input_token_rate: Option<f64>,
    pub image_output_token_rate: Option<f64>,
    pub cached_input_token_rate: Option<f64>,
    pub cache_creation_token_rate: Option<f64>,
    pub max_input_tokens: Option<i32>,
    pub max_output_tokens: Option<i32>,
    pub max_images: Option<i32>,
    pub dimensions: Option<i32>,
    pub price_per_image: Option<f64>,
}

#[derive(Serialize, ToSchema)]
pub struct AiModelCapabilities {
    pub vision: bool,
    pub function_calling: bool,
    pub streaming: bool,
    pub reasoning: bool,
    pub audio: bool,
    pub pdf_native: bool,
    pub web_search: bool,
    pub multiple_images: bool,
    pub embeddings: bool,
    pub image_generation: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plugin_document() -> serde_json::Value {
        json!({
            "manifest": {},
            "configuration": {},
            "baseUrlOverride": "https://example.com",
            "allowInsecureHttp": false,
            "allowPrivateNetwork": false
        })
    }

    #[test]
    fn plugin_validation_accepts_snake_case() {
        let request: AIEnginePluginValidationRequest = serde_json::from_value(json!({
            "plugin_config": plugin_document()
        }))
        .expect("snake_case plugin validation request");

        assert_eq!(
            request.plugin_config.base_url_override.as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn ai_engine_create_and_update_accept_snake_case() {
        let create: AIEngineCreate = serde_json::from_value(json!({
            "display_name": "Example",
            "api_key": "secret",
            "is_enabled": false,
            "whitelisted_models": ["example-chat"],
            "default_model": "example-chat",
            "default_image_gen_model": null,
            "plugin_config": plugin_document()
        }))
        .expect("snake_case create request");
        let update: AIEngineUpdate = serde_json::from_value(json!({
            "display_name": "Example 2",
            "is_enabled": true,
            "whitelisted_models": ["example-chat"],
            "default_model": "example-chat",
            "plugin_config": plugin_document()
        }))
        .expect("snake_case update request");

        assert_eq!(create.display_name.as_deref(), Some("Example"));
        assert_eq!(update.display_name.as_deref(), Some("Example 2"));
        assert_eq!(update.is_enabled, Some(true));
    }

    #[test]
    fn ai_engine_requests_reject_camel_case_envelopes() {
        let validation = serde_json::from_value::<AIEnginePluginValidationRequest>(json!({
            "pluginConfig": plugin_document()
        }));
        let create = serde_json::from_value::<AIEngineCreate>(json!({
            "displayName": "Example",
            "pluginConfig": plugin_document()
        }));
        let update = serde_json::from_value::<AIEngineUpdate>(json!({
            "isEnabled": true
        }));

        assert!(validation.is_err());
        assert!(create.is_err());
        assert!(update.is_err());
    }

    #[test]
    fn plugin_validation_response_serializes_as_snake_case() {
        let response = AIEnginePluginValidationResponse {
            valid: true,
            engine_key: Some("custom".to_string()),
            version: Some("1.0".to_string()),
            name: Some("Custom".to_string()),
            destination: Some("https://example.com".to_string()),
            capabilities: Some(json!({})),
            credential_required: true,
            error: None,
        };
        let value = serde_json::to_value(response).expect("validation response JSON");

        assert!(value.get("engine_key").is_some());
        assert!(value.get("credential_required").is_some());
        assert!(value.get("engineKey").is_none());
        assert!(value.get("credentialRequired").is_none());
    }

    #[test]
    fn connection_test_response_serializes_as_snake_case() {
        let response = AIEngineConnectionTest {
            valid: true,
            mode: "model_list".to_string(),
            models_available: Some(3),
            error_class: None,
        };
        let value = serde_json::to_value(response).expect("connection test response JSON");

        assert_eq!(value["models_available"], 3);
        assert!(value.get("error_class").is_some());
        assert!(value.get("modelsAvailable").is_none());
        assert!(value.get("errorClass").is_none());
    }
}
