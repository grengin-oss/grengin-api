use anyhow::Error;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use utoipa::{ToSchema};
use crate::{config::setting::OpenaiSettings, dto::chat::Attachment};

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum LlmProviders{
    OpenAI,
    Claude,
    Gemini,
    Groq
}

pub struct LLMResponse{
    
}

#[async_trait]
pub trait LLM:OpenAI {
    async fn get_models(&self) -> Vec<String>;
    async fn chat(&self,model_name:String,prompt:String,attachments:Vec<Attachment>) -> Result<String,Error>;
    async fn chat_stream(&self,model_name:String,prompt:String,attachments:Vec<Attachment>) -> Result<String,Error>;
} 

pub trait OpenAI: Send + Sync {
    fn add_openai_headers(self,openai_sesstings:&OpenaiSettings) -> Self;
}