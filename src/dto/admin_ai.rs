// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::models::ai_engines::{ApiKeyStatus, PluginConfig};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AIEngineCreate {
    #[serde(alias = "displayName")]
    pub display_name: Option<String>,
    #[serde(alias = "apiKey")]
    pub api_key: Option<String>,
    #[serde(default, alias = "isEnabled")]
    pub is_enabled: bool,
    #[serde(default, alias = "whitelistedModels")]
    pub whitelisted_models: Vec<String>,
    #[serde(alias = "defaultModel")]
    pub default_model: Option<String>,
    #[serde(alias = "defaultImageGenModel")]
    pub default_image_gen_model: Option<String>,
    #[serde(alias = "pluginConfig")]
    pub plugin_config: PluginConfig,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AIEngineUpdate {
    #[serde(alias = "displayName")]
    pub display_name: Option<String>,
    #[serde(alias = "isEnabled")]
    pub is_enabled: Option<bool>,
    #[serde(alias = "apiKey")]
    pub api_key: Option<String>,
    #[serde(alias = "whitelistedModels")]
    pub whitelisted_models: Option<Vec<String>>,
    #[serde(alias = "defaultModel")]
    pub default_model: Option<String>,
    #[serde(alias = "defaultImageGenModel")]
    pub default_image_gen_model: Option<String>,
    #[serde(alias = "pluginConfig")]
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
    #[serde(alias = "pluginConfig")]
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
#[serde(rename_all = "camelCase")]
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
    pub is_whitelisted: bool,
    pub capabilities: AiModelCapabilities,
}

#[derive(Serialize, ToSchema)]
pub struct AiModelCapabilities {
    pub vision: bool,
    pub function_calling: bool,
    pub streaming: bool,
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
    fn plugin_validation_accepts_legacy_camel_case_envelope() {
        let request: AIEnginePluginValidationRequest = serde_json::from_value(json!({
            "pluginConfig": plugin_document()
        }))
        .expect("legacy camelCase plugin validation request");

        assert_eq!(
            request.plugin_config.base_url_override.as_deref(),
            Some("https://example.com")
        );
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
}
