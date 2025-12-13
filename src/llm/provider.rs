use anyhow::Error;
use async_trait::async_trait;
use reqwest_eventsource::EventSource;
use serde::{Deserialize, Serialize};
use utoipa::{ToSchema};
use crate::{config::setting::OpenaiSettings, dto::files::{Attachment}, llm::prompt::Prompt};

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
    async fn openai_chat_stream(&self,openai_sesstings:&OpenaiSettings,model_name:String,temperature:Option<f32>,prompts:Vec<Prompt>) -> Result<EventSource,Error>;
    async fn openai_chat_stream_text(&self,openai_sesstings:&OpenaiSettings,model_name:String,temperature:Option<f32>,prompt:Vec<String>) -> Result<EventSource,Error>;
    async fn openai_upload_file(&self,openai_settings:&OpenaiSettings,attachment:&Attachment) -> Result<String,Error>;
    async fn openai_get_title(&self,openai_settings:&OpenaiSettings,prompt:String) -> Result<String,Error>;
} 

pub trait OpenaiHeaders: Send + Sync {
    fn add_openai_headers(self,openai_settings:&OpenaiSettings) -> Self;
}