use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, base64::Base64};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use crate::models::messages::ChatRole;

#[derive(Deserialize, ToSchema, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ChatInitRequest{
  pub provider:String,
  pub model_name:String,
  pub config:serde_json::Value,
  pub web_search:bool,
  pub selected_tools:Vec<String>,
  pub messages:Vec<MessageRequest>,
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct MessageRequest {
  pub role:ChatRole,
  pub content:String,
  pub files:Vec<Attachment>
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct ArchiveChatRequest{
  pub title:String,
  pub archived: bool,
}

#[derive(Serialize, ToSchema, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ConversationResponse {
  pub id:Uuid,
   #[serde(skip_serializing_if = "Option::is_none")]
  pub title:Option<String>,
  pub archived:bool,
   #[serde(skip_serializing_if = "Option::is_none")]
  pub archived_at:Option<DateTime<Utc>>,
  pub model:String,
  pub total_tokens:u64,
  pub total_cost:f32,
  pub created_at:DateTime<Utc>,
  pub updated_at:DateTime<Utc>,
   #[serde(skip_serializing_if = "Option::is_none")]
  pub last_message_at:Option<DateTime<Utc>>,
   #[serde(skip_serializing_if = "Option::is_none")]
  pub messages:Option<Vec<MessageResponse>>,
}

#[derive(Serialize, ToSchema, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct MessageResponse {
  pub id:Uuid,
  pub role:ChatRole,
  pub cost:f32,
  pub created_at:DateTime<Utc>,
  pub updated_at:DateTime<Utc>,
  pub request_id:Option<String>,
  pub model:String,
  pub model_params:Option<serde_json::Value>,
  pub tool_calls:Vec<serde_json::Value>,
  pub tools_results:Vec<serde_json::Value>,
  pub parts:MessageParts,
  pub usage:TokenUsage,
}

#[serde_as]
#[derive(Deserialize,Serialize, ToSchema, IntoParams)]
pub struct Attachment {
    #[serde_as(as = "Option<Base64>")]
    #[schema(value_type = String, format = Byte)]
    pub file:Option<Vec<u8>>,
    pub name:String,
    #[serde(rename = "type")]
    pub content_type:String
}

#[derive(Serialize, Deserialize, ToSchema, IntoParams)]
pub struct File {
    pub id:Option<String>,
    pub size: Option<usize>,
    pub name:String,
    #[serde(rename = "type")]
    pub content_type:String
}

#[derive(Serialize, ToSchema, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct MessageParts {
  pub text:String,
  pub files:Option<Vec<File>>,
}

#[derive(Serialize, ToSchema, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
   pub input_tokens:i32,
   pub output_tokens:i32,
   pub total_tokens:i32,
}