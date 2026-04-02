use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

pub struct SsoProviderTemplate {
   pub name:String,
   pub provider:String,
   pub issuer_url:String,
   pub redirect_url:String,
   pub tenant_id:Option<String>,
}

#[derive(Serialize,ToSchema)]
pub struct SsoProvider {
   pub id: Uuid,
   pub provider:String,
   pub name: String,
   pub client_id:String,
   #[serde(rename = "client_secret_preview")]
   #[schema(value_type = String, rename = "client_secret_preview")]
   pub client_secret:String,
   pub issuer_url:String,
   pub redirect_url:String,
   #[serde(skip_serializing_if = "Option::is_none")]
   pub tenant_id:Option<String>,
   pub allowed_domains:Vec<String>,
   pub is_enabled:bool,
   pub created_at:DateTime<Utc>,
   pub updated_at:DateTime<Utc>,
}

#[derive(Serialize,ToSchema)]
pub struct EditableField{
   pub editable:bool,
   pub value:String,
}

#[derive(Serialize,ToSchema)]
pub struct SsoProviderEditable {
   pub id: Uuid,
   pub provider:EditableField,
   pub name: EditableField,
   pub client_id:EditableField,
   #[serde(rename = "client_secret_preview")]
   #[schema(value_type = String, rename = "client_secret_preview")]
   pub client_secret:Option<EditableField>,
   pub issuer_url:EditableField,
   pub redirect_url:EditableField,
   pub tenant_id:Option<EditableField>,
   pub allowed_domains:Vec<String>,
   pub is_enabled:bool,
   pub created_at:DateTime<Utc>,
   pub updated_at:DateTime<Utc>,
}

#[derive(Deserialize,ToSchema)]
pub struct SsoProviderUpdate {
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
