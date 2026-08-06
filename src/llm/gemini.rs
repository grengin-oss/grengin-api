// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use anyhow::{Error, anyhow};
use async_trait::async_trait;
use reqwest::{Client as ReqwestClient, RequestBuilder};
use reqwest_eventsource::EventSource;
use serde_json::{Map, Value, json};

use crate::{
    config::setting::GeminiSettings,
    dto::llm::gemini::prompts_to_gemini_payload,
    llm::{
        prompt::{Prompt, PromptTitleResponse},
        provider::GeminiApis,
    },
};

pub const GEMINI_API_URL: &str = "https://generativelanguage.googleapis.com";

fn add_gemini_headers(builder: RequestBuilder, settings: &GeminiSettings) -> RequestBuilder {
    builder.header("x-goog-api-key", settings.api_key.clone())
}

#[async_trait]
impl GeminiApis for ReqwestClient {
    async fn gemini_chat_stream(
        &self,
        gemini_settings: &GeminiSettings,
        model_name: String,
        temperature: Option<f32>,
        prompts: Vec<Prompt>,
        tools: Option<Value>,
        tool_config: Option<Value>,
    ) -> Result<EventSource, Error> {
        let (system_instruction, contents) = prompts_to_gemini_payload(&prompts);
        self.gemini_chat_stream_with_contents(
            gemini_settings,
            model_name,
            temperature,
            system_instruction,
            Value::Array(contents),
            tools,
            tool_config,
        )
        .await
    }

    async fn gemini_chat_stream_with_contents(
        &self,
        gemini_settings: &GeminiSettings,
        model_name: String,
        temperature: Option<f32>,
        system_instruction: Option<Value>,
        contents: Value,
        tools: Option<Value>,
        tool_config: Option<Value>,
    ) -> Result<EventSource, Error> {
        let mut body = Map::new();
        if let Some(system_instruction) = system_instruction {
            body.insert("system_instruction".to_string(), system_instruction);
        }
        body.insert("contents".to_string(), contents);
        if let Some(tools) = tools {
            body.insert("tools".to_string(), tools);
        }
        if let Some(tool_config) = tool_config {
            body.insert("tool_config".to_string(), tool_config);
        }
        if let Some(temp) = temperature {
            body.insert(
                "generationConfig".to_string(),
                json!({ "temperature": temp }),
            );
        }

        let request = self
            .post(format!(
                "{GEMINI_API_URL}/v1beta/models/{model_name}:streamGenerateContent?alt=sse"
            ))
            .header("Content-Type", "application/json")
            // Gemini requires proper SSE negotiation; without this it may respond with JSON and end immediately.
            .header("Accept", "text/event-stream");
        let request = add_gemini_headers(request, gemini_settings).json(&Value::Object(body));
        Ok(EventSource::new(request)?)
    }

    async fn gemini_get_title(
        &self,
        gemini_settings: &GeminiSettings,
        model_name: String,
        prompt: String,
    ) -> Result<PromptTitleResponse, Error> {
        let title_prompt = format!(
            "Write a short title for the given prompt respond only in title name: {prompt}"
        );

        let body = json!({
            "contents": [{
                "role": "user",
                "parts": [{ "text": title_prompt }]
            }]
        });

        let response = self
            .post(format!(
                "{GEMINI_API_URL}/v1beta/models/{model_name}:generateContent"
            ))
            .header("Content-Type", "application/json");
        let response: Value = add_gemini_headers(response, gemini_settings)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let text = response
            .get("candidates")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|cand| cand.get("content"))
            .and_then(|content| content.get("parts"))
            .and_then(|p| p.as_array())
            .and_then(|arr| arr.first())
            .and_then(|part| part.get("text"))
            .and_then(|t| t.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                anyhow!("gemini response missing candidates[0].content.parts[0].text")
            })?;

        // Gemini REST doesn't always provide token usage in this response shape; keep it optional.
        let input_tokens = response
            .get("usageMetadata")
            .and_then(|u| u.get("promptTokenCount"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32;
        let output_tokens = response
            .get("usageMetadata")
            .and_then(|u| u.get("candidatesTokenCount"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32;

        Ok(PromptTitleResponse {
            title: text,
            input_tokens,
            output_tokens,
        })
    }

}
