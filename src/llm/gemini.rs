use anyhow::{Error, Ok, anyhow};
use async_trait::async_trait;
use reqwest::{Client as ReqwestClient, RequestBuilder};
use reqwest_eventsource::EventSource;
use crate::{
    config::setting::GeminiSettings,
    dto::llm::gemini::{
        GeminiGenerateContentRequest, GeminiGenerateContentResponse, GeminiContent,
        GeminiGenerationConfig, GEMINI_DEFAULT_MAX_TOKENS,
    },
    llm::{prompt::Prompt, provider::{GeminiApis, GeminiHeaders}},
};

pub const GEMINI_API_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

impl GeminiHeaders for RequestBuilder {
    fn add_gemini_headers(self, gemini_settings: &GeminiSettings) -> Self {
        self.header("x-goog-api-key", &gemini_settings.api_key)
            .header("content-type", "application/json")
    }
}

#[async_trait]
impl GeminiApis for ReqwestClient {
    async fn gemini_chat_stream(
        &self,
        gemini_settings: &GeminiSettings,
        model_name: String,
        max_tokens: Option<i32>,
        temperature: Option<f32>,
        prompts: Vec<Prompt>,
    ) -> Result<EventSource, Error> {
        let (contents, system_instruction) = GeminiContent::from_prompts(prompts);

        let body = GeminiGenerateContentRequest {
            contents,
            system_instruction,
            generation_config: Some(GeminiGenerationConfig::new(
                temperature,
                Some(max_tokens.unwrap_or(GEMINI_DEFAULT_MAX_TOKENS)),
            )),
        };

        // Gemini uses streamGenerateContent endpoint with alt=sse for SSE streaming
        let request = self
            .post(format!(
                "{GEMINI_API_URL}/models/{model_name}:streamGenerateContent?alt=sse"
            ))
            .add_gemini_headers(gemini_settings)
            .json(&body);

        let es = EventSource::new(request)?;
        Ok(es)
    }

    async fn gemini_chat_stream_text(
        &self,
        gemini_settings: &GeminiSettings,
        model_name: String,
        max_tokens: Option<i32>,
        temperature: Option<f32>,
        prompt: Vec<String>,
    ) -> Result<EventSource, Error> {
        let contents: Vec<GeminiContent> = prompt
            .into_iter()
            .map(|text| GeminiContent::new_text(Some("user"), text))
            .collect();

        let body = GeminiGenerateContentRequest {
            contents,
            system_instruction: None,
            generation_config: Some(GeminiGenerationConfig::new(
                temperature,
                Some(max_tokens.unwrap_or(GEMINI_DEFAULT_MAX_TOKENS)),
            )),
        };

        let request = self
            .post(format!(
                "{GEMINI_API_URL}/models/{model_name}:streamGenerateContent?alt=sse"
            ))
            .add_gemini_headers(gemini_settings)
            .json(&body);

        let es = EventSource::new(request)?;
        Ok(es)
    }

    async fn gemini_get_title(
        &self,
        gemini_settings: &GeminiSettings,
        prompt: String,
    ) -> Result<String, Error> {
        let title_prompt = format!(
            "Write a short title for the given prompt respond only in title name: {prompt}"
        );

        let body = GeminiGenerateContentRequest {
            contents: vec![GeminiContent::new_text(Some("user"), title_prompt)],
            system_instruction: None,
            generation_config: Some(GeminiGenerationConfig::new(None, Some(100))),
        };

        let response: GeminiGenerateContentResponse = self
            .post(format!(
                "{GEMINI_API_URL}/models/gemini-2.5-flash:generateContent"
            ))
            .add_gemini_headers(gemini_settings)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let title = response
            .candidates
            .and_then(|candidates| candidates.into_iter().next())
            .and_then(|candidate| candidate.content)
            .and_then(|content| content.parts.into_iter().next())
            .and_then(|part| match part {
                crate::dto::llm::gemini::GeminiPart::Text { text } => Some(text),
            })
            .ok_or(anyhow!("gemini response content is empty"))?;

        Ok(title)
    }
}
