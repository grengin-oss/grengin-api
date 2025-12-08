use serde::{Deserialize, Serialize};
use crate::dto::chat::File;

#[derive(Serialize, Deserialize)]
pub struct OpenaiChatCompletionRequest {
    pub model: String,
    pub messages: Vec<OpenaiMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenaiChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<OpenaiChatChunkChoice>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenaiChatChunkChoice {
    pub index: u32,
    pub delta: OpenaiMessageDelta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenaiMessageDelta {
    pub role: Option<String>,      
    pub content: Option<String>,
}

#[derive(Serialize,Deserialize)]
pub enum OpenaiContentType{
 #[serde(rename = "text")]
  InputText,
 #[serde(rename = "file")]
  InputFile
}

#[derive(Serialize,Deserialize)]
pub struct OpenaiMessage{
    pub role:String,
    pub content:Vec<OpenaiContent>
}

impl OpenaiMessage {
    pub fn from_text_and_files(prompt: String,files:Vec<File>) -> OpenaiMessage {
     let mut content = vec![
            OpenaiContent {
                content_type: OpenaiContentType::InputText,
                content: Some(prompt),
                file_id: None,
            }
     ];
     for file in files {
            content.push(OpenaiContent {
                content_type: OpenaiContentType::InputFile,
                content: None,
                file_id: file.id,
            });
     }
     OpenaiMessage {
         role: "user".to_string(),
         content,
    }
  }
}

#[derive(Serialize,Deserialize)]
 pub struct OpenaiContent{
    #[serde(rename = "type")]
    pub content_type:OpenaiContentType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content:Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_id:Option<String>,
 }

 #[derive(Deserialize)]
pub struct FileUploadResponse {
    pub id: String,
}