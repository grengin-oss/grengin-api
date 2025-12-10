use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use crate::{dto::files::File, models::messages::ChatRole};

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
  pub total_tokens:i64,
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