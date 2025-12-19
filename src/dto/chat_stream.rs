use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use crate::{dto::files::{File}, models::messages::ChatRole};

#[derive(Serialize, ToSchema, IntoParams)]
pub struct ChatStream {
   pub id:Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
   pub role:Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
   pub content:Option<String>,
}

impl ChatStream {
    pub fn to_string(&self) -> String{
       serde_json::to_string(self).unwrap()
    }
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct ChatInitRequest{
  pub provider: Option<String>,
  pub model_name: Option<String>,
  pub config: Option<serde_json::Value>,
  #[serde(default)]
  pub web_search: bool,
  pub selected_tools: Option<Vec<String>>,
  pub conversation_id: Option<Uuid>,
  pub messages: Vec<MessageRequest>,
  pub temperature:Option<f32>,
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct MessageRequest {
  pub role:ChatRole,
  pub content:String,
  pub files:Vec<File>
}