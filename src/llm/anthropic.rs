use anyhow::{Error, anyhow};
use async_trait::async_trait;
use reqwest::{Client as ReqwestClient, RequestBuilder};
use reqwest_eventsource::EventSource;
use uuid::Uuid;
use crate::{
    config::setting::AnthropicSettings, dto::llm::anthropic::{
        AnthropicChatRequest, AnthropicChatResponse, AnthropicContentBlockResponse, AnthropicListModelsResponse, AnthropicMessage, AnthropicRole, AnthropicToolUnion, AnthropicWebSearchTool
    }, handlers::file::get_file_binary, llm::{prompt::{Prompt, PromptTitleResponse}, provider::{AnthropicApis, AnthropicHeaders}}
};

pub const ANTHROPIC_API_URL: &str = "https://api.anthropic.com";
pub const ANTHROPIC_API_VERSION: &str = "2023-06-01";

impl AnthropicHeaders for RequestBuilder {
    fn add_anthropic_headers(self, anthropic_settings: &AnthropicSettings) -> Self {
        self.header("x-api-key", &anthropic_settings.api_key)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .header("content-type", "application/json")
    }
}

#[async_trait]
impl AnthropicApis for ReqwestClient {
    async fn anthropic_chat_stream(
        &self,
        anthropic_settings: &AnthropicSettings,
        model_name: String,
        max_tokens: i32,
        temperature: Option<f32>,
        mut prompts: Vec<Prompt>,
        web_search: bool,
        user_id:&Uuid,
    ) -> Result<EventSource, Error> {
        for prompt in &mut prompts {
           for file in &mut prompt.files {
              if let Ok(attachment) = get_file_binary(&file, user_id){
                 file.base64 = attachment.get_base64();
              };
           }
        }
        let (messages, system_prompt) = AnthropicMessage::from_prompts(prompts);    
        let tools = if web_search {
            Some(vec![AnthropicToolUnion::WebSearchTool(
                AnthropicWebSearchTool::new(Some(5)),
            )])
        } else {
            None
        };

        let body = AnthropicChatRequest {
            model: model_name,
            max_tokens,
            messages,
            stream: true,
            temperature,
            system: system_prompt,
            tools,
            stop_sequences: None,
        };

        let request = self
            .post(format!("{ANTHROPIC_API_URL}/v1/messages"))
            .add_anthropic_headers(anthropic_settings)
            .json(&body);

        let es = EventSource::new(request)?;
        Ok(es)
    }

    async fn anthropic_chat_stream_text(
        &self,
        anthropic_settings: &AnthropicSettings,
        model_name: String,
        max_tokens: i32,
        temperature: Option<f32>,
        prompt: Vec<String>,
    ) -> Result<EventSource, Error> {
        let messages: Vec<AnthropicMessage> = prompt
            .into_iter()
            .map(|text| AnthropicMessage::from_text(AnthropicRole::User, text))
            .collect();

        let body = AnthropicChatRequest {
            model: model_name,
            max_tokens,
            messages,
            stream: true,
            temperature,
            system: None,
            tools: None,
            stop_sequences: None,
        };

        let request = self
            .post(format!("{ANTHROPIC_API_URL}/v1/messages"))
            .add_anthropic_headers(anthropic_settings)
            .json(&body);

        let es = EventSource::new(request)?;
        Ok(es)
    }

    async fn anthropic_get_title(
        &self,
        anthropic_settings: &AnthropicSettings,
        prompt: String,
    ) -> Result<PromptTitleResponse, Error> {
        let title_prompt = format!(
            "Write a short title for the given prompt respond only in title name: {prompt}"
        );

        let body = AnthropicChatRequest {
            model: "claude-haiku-4-5".to_string(),
            max_tokens: 100,
            messages: vec![AnthropicMessage::from_text(
                AnthropicRole::User,
                title_prompt,
            )],
            stream: false,
            temperature: None,
            system: None,
            tools: None,
            stop_sequences: None,
         };

         let response: AnthropicChatResponse = self
            .post(format!("{ANTHROPIC_API_URL}/v1/messages"))
            .add_anthropic_headers(anthropic_settings)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

         let title = response
            .content
            .first()
            .and_then(|block| match block {
                AnthropicContentBlockResponse::Text { text } => Some(text.clone()),
                _ => None,
            })
            .ok_or(anyhow!("anthropic response content is empty"))?;
         let input_tokens = response
            .usage
            .input_tokens;
         let output_tokens = response
            .usage
            .output_tokens;
        Ok(PromptTitleResponse { title, input_tokens, output_tokens })
    }

    async fn anthropic_get_models(
        &self,anthropic_settings:
        &AnthropicSettings) -> Result<AnthropicListModelsResponse, Error> {
        let models = self
            .get(format!("{ANTHROPIC_API_URL}/v1/models"))
            .add_anthropic_headers(anthropic_settings)
            .send()
            .await?
            .error_for_status()?
            .json::<AnthropicListModelsResponse>()
            .await?;
        Ok(models)
    }
}
