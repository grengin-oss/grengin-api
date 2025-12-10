use serde::{Deserialize, Serialize};
use crate::{dto::files::File, llm::prompt::Prompt, models::messages::ChatRole};

#[derive(Serialize, Deserialize)]
pub struct OpenaiChatRequest {
    pub model: String,
    pub input: Vec<OpenaiMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum OpenaiResponseStreamEvent {
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta(OpenaiOutputTextDelta),
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub struct OpenaiOutputTextDelta {
    pub item_id: String,
    pub delta: String,
}

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
pub struct OpenaiChatCompletionResponse {
    pub choices: Vec<OpenaiChatChoiceResponse>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenaiChatChoiceResponse {
    pub index: u32,
    pub message:OpenaiMessageDelta,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenaiMessageDelta {
    pub role: Option<String>,      
    pub content: Option<String>,
}

#[derive(Serialize,Deserialize)]
#[serde(rename_all="lowercase")]
pub enum OpenaiContentType{
 #[serde(rename = "input_text")]
  InputText,
 #[serde(rename = "output_text")]
  OutputText,
  Text,
 #[serde(rename = "input_file")]
  InputFile,
 #[serde(rename = "input_image")]
  InputImage,
}

#[derive(Serialize,Deserialize)]
pub struct OpenaiMessage{
    pub role:ChatRole,
    pub content:Vec<OpenaiContent>
}

impl OpenaiMessage {
    pub fn from_text_and_files_input(prompts: Vec<String>,files:Vec<File>) -> Self {
     let mut content = vec![];
     for file in files {
          content.push(OpenaiContent {
                content_type: OpenaiContentType::InputFile,
                text: None,
                file_id:file.id,
          });
     }
     for prompt in prompts {
          content.push(OpenaiContent {
                content_type: OpenaiContentType::InputText,
                text: Some(prompt),
                file_id:None,
            })
     }
     OpenaiMessage {
         role:ChatRole::User,
         content,
    }
  }

  pub fn from_text(prompts: Vec<String>) -> Self {
     let mut content = vec![];
     for prompt in prompts {
          content.push(OpenaiContent {
                content_type: OpenaiContentType::Text,
                text: Some(prompt),
                file_id:None,
            })
     }
     OpenaiMessage {
         role:ChatRole::User,
         content,
    }
  }

  pub fn from_prompts(prompts:Vec<Prompt>) -> Vec<Self> {
      let mut messages = vec![];
      for prompt in prompts {
         let mut content = vec![];
         let content_type = if prompt.role == ChatRole::Assistant{
            OpenaiContentType::OutputText
         }else{
            OpenaiContentType::InputText
         };
         content.push(OpenaiContent {
                content_type,
                text: Some(prompt.text),
                file_id:None,
         });
         for file in prompt.files{
            let content_type = if file.content_type.contains("image"){
                OpenaiContentType::InputImage
            }else{
                OpenaiContentType::InputFile
            };
            content.push(OpenaiContent{
                content_type,
                file_id:file.id,
                text:None
            })
         }
         messages.push(Self { role:prompt.role, content});
      }
    messages   
  }
}

#[derive(Serialize,Deserialize)]
 pub struct OpenaiContent{
    #[serde(rename = "type")]
    pub content_type:OpenaiContentType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text:Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_id:Option<String>,
 }

 #[derive(Deserialize)]
pub struct FileUploadResponse {
    pub id: String,
}