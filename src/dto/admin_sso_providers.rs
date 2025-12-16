use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize,ToSchema)]
pub struct SsoProviderResponse {
   pub provider:String,
   pub name: String,
   pub client_id:String,
   pub client_secret:String,
   pub issuer_url:String,
   pub allowed_domains:Vec<String>,
   pub is_enabled:bool,
}