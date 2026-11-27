// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    dto::models::ModelType,
    models::{
        ai_engines::{ApiKeyStatus, PluginConfig},
        grengin_engine_operations::OperationState,
    },
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AIEngineCreate {
    pub engine_key: Option<String>,
    pub display_name: Option<String>,
    pub api_key: Option<String>,
    pub is_enabled: Option<bool>,
    pub whitelisted_models: Option<Vec<String>>,
    pub default_model: Option<String>,
    pub default_image_gen_model: Option<String>,
    pub plugin_config: Option<PluginConfig>,
    pub base_url_override: Option<String>,
    /// Grengin only: `grengin-models-v1`, required to install with `is_enabled: true`.
    pub consent_version: Option<String>,
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
    pub base_url_override: Option<String>,
    /// Grengin only: `grengin-models-v1`, required to switch `is_enabled` on.
    pub consent_version: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url_override: Option<String>,
    /// Grengin only: an enable/disable still waiting for service confirmation (HTTP 202).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_operation: Option<GrenginOperationStatus>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Serialize, ToSchema)]
pub struct GrenginOperationStatus {
    pub id: uuid::Uuid,
    pub revision: i64,
    pub desired_enabled: bool,
    pub state: OperationState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GrenginConnectionState {
    SetupNeeded,
    Connected,
    Disconnected,
    Unavailable,
}

#[derive(Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GrenginFundingSource {
    Grengin,
    OwnKey,
}

/// One model the Grengin service offers, with this workspace's local policy applied.
/// Prices are Grengin tariffs even when an own-key engine would fund the model.
#[derive(Serialize, ToSchema)]
pub struct GrenginCatalogModel {
    pub id: String,
    pub name: String,
    pub provider_id: String,
    pub native_model_id: String,
    pub capability: ModelType,
    pub input_per_million_microdollars: Option<i64>,
    pub output_per_million_microdollars: Option<i64>,
    pub processing_region: Option<String>,
    pub status: String,
    pub is_allowed: bool,
    pub is_default: bool,
    pub effective_engine: String,
    pub funding_source: GrenginFundingSource,
}

#[derive(Serialize, ToSchema)]
pub struct GrenginCatalog {
    pub observed_at: DateTime<Utc>,
    pub models: Vec<GrenginCatalogModel>,
}

/// Local and service status of the Grengin engine. Unknown values are null, never zero.
#[derive(Serialize, ToSchema)]
pub struct GrenginStatus {
    pub installed: bool,
    pub local_enabled: bool,
    pub connection_state: GrenginConnectionState,
    pub engine_state: Option<String>,
    pub remote_enabled: Option<bool>,
    pub installation_id: Option<String>,
    pub workspace_id: Option<String>,
    pub balance_microdollars: Option<i64>,
    pub available_microdollars: Option<i64>,
    pub low_balance_microdollars: Option<i64>,
    pub minimum_version: Option<String>,
    pub balance_url: Option<String>,
    pub top_up_url: Option<String>,
    pub observed_at: Option<DateTime<Utc>>,
    pub stale: bool,
    pub error_code: Option<String>,
    pub pending_operation: Option<GrenginOperationStatus>,
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
    pub models: Vec<PluginModel>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct PluginModel {
    pub key: String,
    pub name: String,
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
    fn catalog_engines_accept_their_own_endpoint() {
        let create: AIEngineCreate = serde_json::from_value(json!({
            "engine_key": "azure-openai",
            "api_key": "secret",
            "base_url_override": "https://contoso.services.ai.azure.com/openai/v1/"
        }))
        .expect("catalog create with endpoint");
        let update: AIEngineUpdate = serde_json::from_value(json!({
            "base_url_override": "https://bedrock-runtime.eu-west-1.amazonaws.com/openai/v1/"
        }))
        .expect("catalog update with endpoint");

        assert_eq!(
            create.base_url_override.as_deref(),
            Some("https://contoso.services.ai.azure.com/openai/v1/")
        );
        assert_eq!(
            update.base_url_override.as_deref(),
            Some("https://bedrock-runtime.eu-west-1.amazonaws.com/openai/v1/")
        );
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
            models: vec![PluginModel {
                key: "custom-model".to_string(),
                name: "Custom Model".to_string(),
            }],
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
