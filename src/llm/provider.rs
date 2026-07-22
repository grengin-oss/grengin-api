use crate::{
    config::setting::{AnthropicSettings, GeminiSettings, MistralSettings, OpenaiSettings},
    dto::{
        files::Attachment,
        llm::{
            anthropic::{AnthropicListModelsResponse, AnthropicMessage, AnthropicToolUnion},
            openai::OpenaiModel,
        },
    },
    llm::prompt::{Prompt, PromptTextResponse, PromptTitleResponse},
};
use anyhow::Error;
use async_trait::async_trait;
use reqwest_eventsource::EventSource;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum LlmProviders {
    OpenAI,
    Anthropic,
    Mistral,
    Gemini,
    Google,
    Groq,
}

pub fn get_title_generation_model(provider: &str) -> Option<&str> {
    match provider {
        "openai" => "o3-mini".into(),
        "anthropic" => "claude-haiku-4-5".into(),
        "mistral" => "mistral-small-2603".into(),
        "gemini" => "gemini-2.5-flash".into(),
        _ => None,
    }
}

#[async_trait]
pub trait OpenaiApis {
    async fn openai_chat_stream(
        &self,
        openai_settings: &OpenaiSettings,
        model_name: String,
        temperature: Option<f32>,
        prompts: Vec<Prompt>,
        user_id: &Uuid,
        tools: Option<Vec<crate::dto::llm::openai::OpenaiTool>>,
        tool_choice: Option<crate::dto::llm::openai::OpenaiToolChoice>,
        previous_response_id: Option<String>,
        input: Option<Vec<crate::dto::llm::openai::OpenaiInputItem>>,
    ) -> Result<EventSource, Error>;
    async fn openai_chat_stream_text(
        &self,
        openai_settings: &OpenaiSettings,
        model_name: String,
        temperature: Option<f32>,
        prompt: Vec<String>,
    ) -> Result<EventSource, Error>;
    async fn openai_upload_file(
        &self,
        openai_settings: &OpenaiSettings,
        attachment: &Attachment,
    ) -> Result<String, Error>;
    async fn openai_get_title(
        &self,
        openai_settings: &OpenaiSettings,
        prompt: String,
    ) -> Result<PromptTitleResponse, Error>;
    async fn openai_generate_text(
        &self,
        openai_settings: &OpenaiSettings,
        model_name: String,
        messages: Vec<crate::dto::llm::openai::OpenaiMessage>,
        temperature: Option<f32>,
    ) -> Result<PromptTextResponse, Error>;
    async fn openai_create_embedding(
        &self,
        openai_settings: &OpenaiSettings,
        model_name: String,
        input: Vec<String>,
        dimensions: Option<i32>,
    ) -> Result<crate::dto::llm::openai::OpenaiEmbeddingResponse, Error>;
    async fn openai_list_models(
        &self,
        openai_settings: &OpenaiSettings,
    ) -> Result<Vec<OpenaiModel>, Error>;
}

pub trait OpenaiHeaders: Send + Sync {
    fn add_openai_headers(self, openai_settings: &OpenaiSettings) -> Self;
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
        tools: Option<Vec<AnthropicToolUnion>>,
        user_id: &Uuid,
    ) -> Result<EventSource, Error>;

    async fn anthropic_chat_stream_with_messages(
        &self,
        anthropic_settings: &AnthropicSettings,
        model_name: String,
        max_tokens: i32,
        temperature: Option<f32>,
        messages: Vec<AnthropicMessage>,
        system: Option<String>,
        tools: Option<Vec<AnthropicToolUnion>>,
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
    ) -> Result<PromptTitleResponse, Error>;
    async fn anthropic_generate_text(
        &self,
        anthropic_settings: &AnthropicSettings,
        model_name: String,
        max_tokens: i32,
        messages: Vec<AnthropicMessage>,
        system: Option<String>,
        temperature: Option<f32>,
    ) -> Result<PromptTextResponse, Error>;

    async fn anthropic_get_models(
        &self,
        anthropic_settings: &AnthropicSettings,
    ) -> Result<AnthropicListModelsResponse, Error>;
}

pub trait AnthropicHeaders: Send + Sync {
    fn add_anthropic_headers(self, anthropic_settings: &AnthropicSettings) -> Self;
}

#[async_trait]
pub trait MistralApis {
    async fn mistral_chat_stream(
        &self,
        mistral_settings: &MistralSettings,
        model_name: String,
        temperature: Option<f32>,
        prompts: Vec<Prompt>,
        tools: Option<Vec<crate::dto::llm::mistral::MistralTool>>,
        tool_choice: Option<serde_json::Value>,
    ) -> Result<EventSource, Error>;

    async fn mistral_chat_stream_with_messages(
        &self,
        mistral_settings: &MistralSettings,
        model_name: String,
        temperature: Option<f32>,
        messages: Vec<crate::dto::llm::mistral::MistralMessage>,
        tools: Option<Vec<crate::dto::llm::mistral::MistralTool>>,
        tool_choice: Option<serde_json::Value>,
    ) -> Result<EventSource, Error>;

    async fn mistral_get_title(
        &self,
        mistral_settings: &MistralSettings,
        model_name: String,
        prompt: String,
    ) -> Result<PromptTitleResponse, Error>;

    async fn mistral_conversation_start_stream(
        &self,
        mistral_settings: &MistralSettings,
        inputs: serde_json::Value,
        tools: Option<Vec<crate::dto::llm::mistral::MistralTool>>,
        completion_args: Option<serde_json::Value>,
        model: Option<String>,
        agent_id: Option<String>,
        instructions: Option<String>,
    ) -> Result<EventSource, Error>;

    async fn mistral_conversation_append_stream(
        &self,
        mistral_settings: &MistralSettings,
        conversation_id: String,
        inputs: serde_json::Value,
        tools: Option<Vec<crate::dto::llm::mistral::MistralTool>>,
        completion_args: Option<serde_json::Value>,
    ) -> Result<EventSource, Error>;

    async fn mistral_create_agent(
        &self,
        mistral_settings: &MistralSettings,
        model: String,
        name: String,
        description: Option<String>,
        instructions: String,
        tools: Option<Vec<crate::dto::llm::mistral::MistralTool>>,
        completion_args: Option<serde_json::Value>,
    ) -> Result<String, Error>;
}

pub trait MistralHeaders: Send + Sync {
    fn add_mistral_headers(self, mistral_settings: &MistralSettings) -> Self;
}

#[async_trait]
pub trait GeminiApis {
    async fn gemini_chat_stream(
        &self,
        gemini_settings: &GeminiSettings,
        model_name: String,
        temperature: Option<f32>,
        prompts: Vec<Prompt>,
        tools: Option<serde_json::Value>,
        tool_config: Option<serde_json::Value>,
    ) -> Result<EventSource, Error>;

    async fn gemini_chat_stream_with_contents(
        &self,
        gemini_settings: &GeminiSettings,
        model_name: String,
        temperature: Option<f32>,
        system_instruction: Option<serde_json::Value>,
        contents: serde_json::Value,
        tools: Option<serde_json::Value>,
        tool_config: Option<serde_json::Value>,
    ) -> Result<EventSource, Error>;

    async fn gemini_get_title(
        &self,
        gemini_settings: &GeminiSettings,
        model_name: String,
        prompt: String,
    ) -> Result<PromptTitleResponse, Error>;

}
