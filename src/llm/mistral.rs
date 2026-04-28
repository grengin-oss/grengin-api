use crate::{
    config::setting::MistralSettings,
    dto::llm::mistral::{
        MistralAgentCreateRequest, MistralAgentCreateResponse, MistralChatCompletionRequest,
        MistralChatCompletionResponse, MistralMessage, MistralTool,
    },
    llm::{
        prompt::{Prompt, PromptTitleResponse},
        provider::{MistralApis, MistralHeaders},
    },
};
use anyhow::{Error, anyhow};
use async_trait::async_trait;
use reqwest::{Client as ReqwestClient, RequestBuilder};
use reqwest_eventsource::EventSource;

pub const MISTRAL_API_URL: &str = "https://api.mistral.ai";

#[derive(serde::Serialize)]
struct MistralConversationStreamRequest {
    inputs: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<MistralTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completion_args: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

impl MistralHeaders for RequestBuilder {
    fn add_mistral_headers(self, mistral_settings: &MistralSettings) -> Self {
        self.bearer_auth(&mistral_settings.api_key)
            .header("content-type", "application/json")
    }
}

#[async_trait]
impl MistralApis for ReqwestClient {
    async fn mistral_chat_stream(
        &self,
        mistral_settings: &MistralSettings,
        model_name: String,
        temperature: Option<f32>,
        prompts: Vec<Prompt>,
        tools: Option<Vec<MistralTool>>,
        tool_choice: Option<serde_json::Value>,
    ) -> Result<EventSource, Error> {
        let messages = MistralMessage::from_prompts(prompts);
        self.mistral_chat_stream_with_messages(
            mistral_settings,
            model_name,
            temperature,
            messages,
            tools,
            tool_choice,
        )
        .await
    }

    async fn mistral_chat_stream_with_messages(
        &self,
        mistral_settings: &MistralSettings,
        model_name: String,
        temperature: Option<f32>,
        messages: Vec<MistralMessage>,
        tools: Option<Vec<MistralTool>>,
        tool_choice: Option<serde_json::Value>,
    ) -> Result<EventSource, Error> {
        let body = MistralChatCompletionRequest {
            model: model_name,
            messages,
            stream: true,
            temperature,
            tools,
            tool_choice,
            parallel_tool_calls: None,
        };
        if std::env::var("MISTRAL_TOOL_DEBUG").as_deref() == Ok("1") {
            if let Ok(payload) = serde_json::to_string(&body) {
                println!("mistral chat request: {}", payload);
            }
        }
        let request = self
            .post(format!("{MISTRAL_API_URL}/v1/chat/completions"))
            .add_mistral_headers(mistral_settings)
            .json(&body);
        let es = EventSource::new(request)?;
        Ok(es)
    }

    async fn mistral_get_title(
        &self,
        mistral_settings: &MistralSettings,
        model_name: String,
        prompt: String,
    ) -> Result<PromptTitleResponse, Error> {
        let title_prompt = format!(
            "Write a short title for the given prompt respond only in title name: {prompt}"
        );
        let messages = vec![MistralMessage {
            role: crate::models::messages::ChatRole::User,
            content: Some(title_prompt),
            name: None,
            tool_call_id: None,
            tool_calls: None,
        }];
        let body = MistralChatCompletionRequest {
            model: model_name,
            messages,
            stream: false,
            temperature: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
        };
        let response: MistralChatCompletionResponse = self
            .post(format!("{MISTRAL_API_URL}/v1/chat/completions"))
            .add_mistral_headers(mistral_settings)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let title = response
            .choices
            .first()
            .and_then(|choice| choice.message.content.clone())
            .ok_or(anyhow!("mistral response content is empty"))?;
        let (input_tokens, output_tokens) = response
            .usage
            .map(|usage| (usage.prompt_tokens as i32, usage.completion_tokens as i32))
            .unwrap_or((0, 0));
        Ok(PromptTitleResponse {
            title,
            input_tokens,
            output_tokens,
        })
    }

    async fn mistral_conversation_start_stream(
        &self,
        mistral_settings: &MistralSettings,
        inputs: serde_json::Value,
        tools: Option<Vec<MistralTool>>,
        completion_args: Option<serde_json::Value>,
        model: Option<String>,
        agent_id: Option<String>,
        instructions: Option<String>,
    ) -> Result<EventSource, Error> {
        let body = MistralConversationStreamRequest {
            inputs,
            tools,
            completion_args,
            model,
            agent_id,
            instructions,
            stream: Some(true),
        };
        let request = self
            .post(format!("{MISTRAL_API_URL}/v1/conversations"))
            .add_mistral_headers(mistral_settings)
            .json(&body);
        let es = EventSource::new(request)?;
        Ok(es)
    }

    async fn mistral_conversation_append_stream(
        &self,
        mistral_settings: &MistralSettings,
        conversation_id: String,
        inputs: serde_json::Value,
        tools: Option<Vec<MistralTool>>,
        completion_args: Option<serde_json::Value>,
    ) -> Result<EventSource, Error> {
        let body = MistralConversationStreamRequest {
            inputs,
            tools,
            completion_args,
            model: None,
            agent_id: None,
            instructions: None,
            stream: Some(true),
        };
        let request = self
            .post(format!(
                "{MISTRAL_API_URL}/v1/conversations/{conversation_id}"
            ))
            .add_mistral_headers(mistral_settings)
            .json(&body);
        let es = EventSource::new(request)?;
        Ok(es)
    }

    async fn mistral_create_agent(
        &self,
        mistral_settings: &MistralSettings,
        model: String,
        name: String,
        description: Option<String>,
        instructions: String,
        tools: Option<Vec<MistralTool>>,
        completion_args: Option<serde_json::Value>,
    ) -> Result<String, Error> {
        let body = MistralAgentCreateRequest {
            model,
            name,
            description,
            instructions,
            tools,
            completion_args,
        };
        let response: MistralAgentCreateResponse = self
            .post(format!("{MISTRAL_API_URL}/v1/agents"))
            .add_mistral_headers(mistral_settings)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(response.id)
    }
}
