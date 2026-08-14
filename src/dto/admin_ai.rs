// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::models::ai_engines::{ApiKeyStatus, PluginConfig};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AIEnginePluginValidationRequest {
    pub plugin_config: PluginConfig,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
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
