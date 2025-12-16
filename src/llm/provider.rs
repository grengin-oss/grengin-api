use anyhow::Error;
use async_trait::async_trait;
use reqwest_eventsource::EventSource;
use serde::{Deserialize, Serialize};
use utoipa::{ToSchema};
use crate::{config::setting::{OpenaiSettings, AnthropicSettings, GroqSettings, GeminiSettings}, dto::files::Attachment, llm::prompt::Prompt};

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
    async fn openai_chat_stream(&self,openai_sesstings:&OpenaiSettings,model_name:String,temperature:Option<f32>,prompts:Vec<Prompt>) -> Result<EventSource,Error>;
    async fn openai_chat_stream_text(&self,openai_sesstings:&OpenaiSettings,model_name:String,temperature:Option<f32>,prompt:Vec<String>) -> Result<EventSource,Error>;
    async fn openai_upload_file(&self,openai_settings:&OpenaiSettings,attachment:&Attachment) -> Result<String,Error>;
    async fn openai_get_title(&self,openai_settings:&OpenaiSettings,prompt:String) -> Result<String,Error>;
} 

pub trait OpenaiHeaders: Send + Sync {
    fn add_openai_headers(self,openai_settings:&OpenaiSettings) -> Self;
}

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
}

pub trait AnthropicHeaders: Send + Sync {
    fn add_anthropic_headers(self, anthropic_settings: &AnthropicSettings) -> Self;
}

#[async_trait]
pub trait GroqApis {
    async fn groq_chat_stream(
        &self,
        groq_settings: &GroqSettings,
        model_name: String,
        max_tokens: Option<i32>,
        temperature: Option<f32>,
        prompts: Vec<Prompt>,
    ) -> Result<EventSource, Error>;

    async fn groq_chat_stream_text(
        &self,
        groq_settings: &GroqSettings,
        model_name: String,
        max_tokens: Option<i32>,
        temperature: Option<f32>,
        prompt: Vec<String>,
    ) -> Result<EventSource, Error>;

    async fn groq_get_title(
        &self,
        groq_settings: &GroqSettings,
        prompt: String,
    ) -> Result<String, Error>;
}

pub trait GroqHeaders: Send + Sync {
    fn add_groq_headers(self, groq_settings: &GroqSettings) -> Self;
}

#[async_trait]
pub trait GeminiApis {
    async fn gemini_chat_stream(
        &self,
        gemini_settings: &GeminiSettings,
        model_name: String,
        max_tokens: Option<i32>,
        temperature: Option<f32>,
        prompts: Vec<Prompt>,
    ) -> Result<EventSource, Error>;

    async fn gemini_chat_stream_text(
        &self,
        gemini_settings: &GeminiSettings,
        model_name: String,
        max_tokens: Option<i32>,
        temperature: Option<f32>,
        prompt: Vec<String>,
    ) -> Result<EventSource, Error>;

    async fn gemini_get_title(
        &self,
        gemini_settings: &GeminiSettings,
        prompt: String,
    ) -> Result<String, Error>;
}

pub trait GeminiHeaders: Send + Sync {
    fn add_gemini_headers(self, gemini_settings: &GeminiSettings) -> Self;
}