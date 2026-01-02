use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize,ToSchema)]
pub struct SsoProviderResponse {
   pub id: Uuid,
   pub provider:String,
   pub name: String,
   pub client_id:String,
   #[serde(rename = "client_secret_preview")]
   #[schema(value_type = String, rename = "client_secret_preview")]
   pub client_secret:Option<String>,
   pub issuer_url:String,
   pub redirect_url:String,
   pub allowed_domains:Vec<String>,
   pub is_enabled:bool,
   pub created_at:DateTime<Utc>,
   pub updated_at:DateTime<Utc>,
}

#[derive(Deserialize,ToSchema)]
pub struct SsoProviderUpdateRequest {
   pub provider:Option<String>,
   pub tenant_id:Option<String>,
   pub name: Option<String>,
   pub client_id:Option<String>,
   pub client_secret:Option<String>,
   pub issuer_url:Option<String>,
   pub redirect_url:Option<String>,
   pub allowed_domains:Option<Vec<String>>,
   pub is_enabled:Option<bool>,
}

