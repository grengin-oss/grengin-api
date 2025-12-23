use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use crate::models::ai_engines::ApiKeyStatus;

#[derive(Deserialize,ToSchema)]
 pub struct AiEngineUpdateRequest{
    pub is_enabled:bool,
    pub api_key:String,
    pub whitelisted_models:Vec<String>,
    pub default_model:String,
}

#[derive(Serialize,ToSchema)]
 pub struct AiEngineResponse{
     pub engine_key:String,
     pub display_name:String,
     pub is_enabled:bool,
     pub api_key_configured:bool,
     pub api_key_status:ApiKeyStatus,
     pub api_key_preview:Option<String>,
     pub api_key_last_validated_at:Option<DateTime<Utc>>,
     pub whitelisted_models:Vec<String>,
     pub default_model:Option<String>,
     pub created_at:DateTime<Utc>,
     pub updated_at:DateTime<Utc>,
  }

#[derive(Serialize,ToSchema)]
 pub struct AiEngineValidationResponse {
     pub valid:bool,
     pub message:String,
     pub models_available:i64,
}