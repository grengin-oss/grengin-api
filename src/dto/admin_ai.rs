use crate::models::ai_engines::ApiKeyStatus;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
pub struct AIEngineUpdate {
    pub is_enabled: Option<bool>,
    pub api_key: Option<String>,
    pub whitelisted_models: Option<Vec<String>>,
    pub default_model: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct AIEngineDetail {
    pub icon: Option<String>,
    pub icon_dark: Option<String>,
    pub engine_key: String,
    pub display_name: String,
    pub is_enabled: bool,
    pub api_key_configured: bool,
    pub api_key_status: ApiKeyStatus,
    pub api_key_preview: Option<String>,
    pub api_key_last_validated_at: Option<DateTime<Utc>>,
    pub whitelisted_models: Vec<String>,
    pub default_model: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
