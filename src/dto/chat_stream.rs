use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use crate::{dto::files::File, models::messages::ChatRole};

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
#[serde(rename_all = "camelCase")]
pub struct ChatInitRequest{
  pub provider:String,
  pub model_name:String,
  pub config:serde_json::Value,
  pub web_search:bool,
  pub selected_tools:Vec<String>,
  pub temperature:Option<f32>,
  pub conversation_id:Option<Uuid>,
  pub messages:Vec<MessageRequest>,
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct MessageRequest {
  pub role:ChatRole,
  pub content:String,
  pub files:Vec<File>
}