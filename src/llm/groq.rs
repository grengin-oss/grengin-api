use anyhow::{Error, Ok, anyhow};
use async_trait::async_trait;
use reqwest::{Client as ReqwestClient, RequestBuilder};
use reqwest_eventsource::EventSource;
use crate::{
    config::setting::GroqSettings,
    dto::llm::groq::{
        GroqChatRequest, GroqChatCompletionResponse, GroqMessage, GROQ_DEFAULT_MAX_TOKENS,
    },
    llm::{prompt::Prompt, provider::{GroqApis, GroqHeaders}},
};

pub const GROQ_API_URL: &str = "https://api.groq.com/openai/v1";

impl GroqHeaders for RequestBuilder {
    fn add_groq_headers(self, groq_settings: &GroqSettings) -> Self {
        self.bearer_auth(&groq_settings.api_key)
            .header("content-type", "application/json")
    }
}

#[async_trait]
impl GroqApis for ReqwestClient {
    async fn groq_chat_stream(
        &self,
        groq_settings: &GroqSettings,
        model_name: String,
        max_tokens: Option<i32>,
        temperature: Option<f32>,
        prompts: Vec<Prompt>,
    ) -> Result<EventSource, Error> {
        let body = GroqChatRequest {
            model: model_name,
            messages: GroqMessage::from_prompts(prompts),
            stream: true,
            temperature,
            max_tokens: Some(max_tokens.unwrap_or(GROQ_DEFAULT_MAX_TOKENS)),
        };

        let request = self
            .post(format!("{GROQ_API_URL}/chat/completions"))
            .add_groq_headers(groq_settings)
            .json(&body);

        let es = EventSource::new(request)?;
        Ok(es)
    }

    async fn groq_chat_stream_text(
        &self,
        groq_settings: &GroqSettings,
        model_name: String,
        max_tokens: Option<i32>,
        temperature: Option<f32>,
        prompt: Vec<String>,
    ) -> Result<EventSource, Error> {
        let messages: Vec<GroqMessage> = prompt
            .into_iter()
            .map(|text| GroqMessage::from_text("user", text))
            .collect();

        let body = GroqChatRequest {
            model: model_name,
            messages,
            stream: true,
            temperature,
            max_tokens: Some(max_tokens.unwrap_or(GROQ_DEFAULT_MAX_TOKENS)),
        };

        let request = self
            .post(format!("{GROQ_API_URL}/chat/completions"))
            .add_groq_headers(groq_settings)
            .json(&body);

        let es = EventSource::new(request)?;
        Ok(es)
    }

    async fn groq_get_title(
        &self,
        groq_settings: &GroqSettings,
        prompt: String,
    ) -> Result<String, Error> {
        let title_prompt = format!(
            "Write a short title for the given prompt respond only in title name: {prompt}"
        );

        let body = GroqChatRequest {
            model: "openai/gpt-oss-120b".to_string(),
            messages: vec![GroqMessage::from_text("user", title_prompt)],
            stream: false,
            temperature: None,
            max_tokens: Some(100),
        };

        let response: GroqChatCompletionResponse = self
            .post(format!("{GROQ_API_URL}/chat/completions"))
            .add_groq_headers(groq_settings)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let title = response
            .choices
            .first()
            .map(|choice| choice.message.content.clone())
            .ok_or(anyhow!("groq response content is empty"))?;

        Ok(title)
    }
}
