use std::str::Bytes;
use anyhow::Error;
use async_trait::async_trait;
use crate::config::setting::OpenaiSettings;

pub enum LlmProviders{
    OpenAI,
    Claude,
    Gemini,
    Groq
}

#[async_trait]
pub trait LLM: Send + Sync {
    async fn get_models(&self,settings:&OpenaiSettings) -> Vec<String>;
    async fn chat(&self,prompt:String,attachments:Vec<Bytes>) -> Result<String,Error>;
    async fn chat_stream(&self,prompt:String,attachments:Vec<Bytes>) -> Result<String,Error>;
} 