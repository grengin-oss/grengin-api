use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::{base64::{Base64}, serde_as};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use crate::{models::files::FileUploadStatus};

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

impl Attachment {
    pub fn get_base64(self) -> Option<String> {
        self.file
          .map(|buff|{
            BASE64.encode(&buff)
          })
    }
}

#[derive(Debug,Serialize, Deserialize, ToSchema, IntoParams)]
pub struct File {
    pub id:Uuid,
    pub size: Option<usize>,
    pub name:String,
    #[serde(rename = "type")]
    pub content_type:String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openai_id:Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base64:Option<String>,
}

#[derive(Serialize, Deserialize, ToSchema, IntoParams)]
pub struct FileUploadRequest{
    pub provider:Option<String>,
    pub description:Option<String>,
    pub attachment:Attachment
}

#[derive(Serialize, Deserialize, ToSchema, IntoParams)]
pub struct FileResponse {
  pub id:Uuid,
  pub name:String,
  pub size:i64,
    #[serde(rename = "type")]
  pub content_type:String,
  pub description:Option<String>,
  pub url:Option<String>,
  pub download_url:String,
  pub created_at:DateTime<Utc>,
  pub updated_at:DateTime<Utc>,
  pub status:FileUploadStatus,
}

#[derive(Serialize, Deserialize, ToSchema, IntoParams)]
pub struct FilePaginatedResponse {
   pub total:u64,
   pub limit:u64,
   pub offset:u64,
   pub files:Vec<FileResponse>,
}