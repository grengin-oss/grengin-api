use anyhow::Error;
use async_trait::async_trait;
use reqwest_eventsource::EventSource;
use serde::{Deserialize, Serialize};
use utoipa::{ToSchema};
use crate::{config::setting::OpenaiSettings, dto::chat::{Attachment, File}};

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum LlmProviders{
    OpenAI,
    Claude,
    Gemini,
    Groq
}

#[async_trait]
pub trait OpenaiApis {
    async fn openai_chat_stream(&self,openai_sesstings:&OpenaiSettings,model_name:String,prompt:String,temperature:Option<f32>,files:Vec<File>) -> Result<EventSource,Error>;
    async fn openai_upload_file(&self,openai_settings:&OpenaiSettings,attachment:&Attachment) -> Result<String,Error>;
} 

pub trait OpenaiHeaders: Send + Sync {
    fn add_openai_headers(self,openai_sesstings:&OpenaiSettings) -> Self;
}