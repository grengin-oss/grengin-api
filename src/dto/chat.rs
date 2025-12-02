use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_with::{serde_as, base64::Base64};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use crate::models::messages::ChatRole;

#[derive(Deserialize, ToSchema, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ConversationResponse {
  pub id:Uuid,
  pub title:String,
  pub archived:bool,
  pub archived_at:DateTime<Utc>,
  pub model:String,
  pub total_tokens:i32,
  pub total_cost:f32,
  pub created_at:DateTime<Utc>,
  pub updated_at:DateTime<Utc>,
  pub last_message_at:DateTime<Utc>,
  pub messages:Option<Vec<MessageResponse>>,
}

#[derive(Deserialize, ToSchema, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct MessageResponse {
  pub id:Uuid,
  pub role:ChatRole,
  pub cost:f32,
  pub created_at:DateTime<Utc>,
  pub updated_at:DateTime<Utc>,
  pub request_id:Option<String>,
  pub model:String,
  pub model_params:serde_json::Value,
  pub tool_calls:Vec<serde_json::Value>,
  pub tools_results:Vec<serde_json::Value>,
  pub parts:MessageParts,
  pub usage:TokenUsage,
}

#[serde_as]
#[derive(Deserialize, ToSchema, IntoParams)]
pub struct Attachment {
    #[serde_as(as = "Option<Base64>")]
    #[schema(value_type = String, format = Byte)]
    pub file:Option<Vec<u8>>,
    pub name:String,
    #[serde(rename = "type")]
    pub content_type:String
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct File {
    pub id:Option<String>,
    pub size: usize,
    pub name:String,
    #[serde(rename = "type")]
    pub content_type:String
}

#[derive(Deserialize, ToSchema, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct MessageParts {
  pub text:String,
  pub files:Vec<File>,
}

#[derive(Deserialize, ToSchema, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
   pub input_tokens:i32,
   pub output_tokens:i32,
   pub total_tokens:i32,
}