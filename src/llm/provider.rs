use anyhow::Error;
use async_trait::async_trait;
use reqwest_eventsource::EventSource;
use serde::{Deserialize, Serialize};
use utoipa::{ToSchema};
use uuid::Uuid;
use crate::{config::setting::{AnthropicSettings, OpenaiSettings}, dto::{files::Attachment, llm::{anthropic::AnthropicListModelsResponse, openai::OpenaiModel}}, llm::prompt::Prompt};

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum LlmProviders{
    OpenAI,
    Anthropic,
    Google,
    Groq
}

#[async_trait]
pub trait OpenaiApis {
    async fn openai_chat_stream(&self,openai_settings:&OpenaiSettings,model_name:String,temperature:Option<f32>,mut prompts:Vec<Prompt>,user_id:&Uuid) -> Result<EventSource,Error>;
    async fn openai_chat_stream_text(&self,openai_settings:&OpenaiSettings,model_name:String,temperature:Option<f32>,prompt:Vec<String>) -> Result<EventSource,Error>;
    async fn openai_upload_file(&self,openai_settings:&OpenaiSettings,attachment:&Attachment) -> Result<String,Error>;
    async fn openai_get_title(&self,openai_settings:&OpenaiSettings,prompt:String) -> Result<String,Error>;
    async fn openai_list_models(&self,openai_settings: &OpenaiSettings) -> Result<Vec<OpenaiModel>, Error>;
} 

pub trait OpenaiHeaders: Send + Sync {
    fn add_openai_headers(self,openai_settings:&OpenaiSettings) -> Self;
}

/// Type alias for file data loader function
pub type FileDataLoader = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

#[async_trait]
pub trait AnthropicApis {
    async fn anthropic_chat_stream(
        &self,
        anthropic_settings: &AnthropicSettings,
        model_name: String,
        max_tokens: i32,
        temperature: Option<f32>,
        prompts: Vec<Prompt>,
        web_search: bool,
        user_id:&Uuid,
    ) -> Result<EventSource, Error>;

    async fn anthropic_chat_stream_text(
        &self,
        anthropic_settings: &AnthropicSettings,
        model_name: String,
        max_tokens: i32,
        temperature: Option<f32>,
        prompt: Vec<String>,
    ) -> Result<EventSource, Error>;

    async fn anthropic_get_title(
        &self,
        anthropic_settings: &AnthropicSettings,
        prompt: String,
    ) -> Result<String, Error>;

    async fn anthropic_get_models(
        &self,
        anthropic_settings: &AnthropicSettings
    ) -> Result<AnthropicListModelsResponse, Error>;
}

pub trait AnthropicHeaders: Send + Sync {
    fn add_anthropic_headers(self, anthropic_settings: &AnthropicSettings) -> Self;
}