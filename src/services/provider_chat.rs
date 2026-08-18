// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{collections::HashMap, pin::Pin};

use futures_util::{Stream, StreamExt};
use llm_plugin::{
    ChatMessage, ChatRequest, ChatRole as PluginChatRole, ContentPart, ModelId, ProviderError,
    ProviderEvent, ProviderEventErrorKind, ProviderEventStream, ProviderPlugin, TokenUsage,
    ToolCallId, ToolChoice, ToolDefinition,
};
use serde_json::Value;

use crate::{
    dto::prompt::{Prompt, PromptTextResponse, PromptTitleResponse},
    models::messages::ChatRole,
    services::mcp_tools::McpToolDescriptor,
    services::provider_stream::{
        StreamErrorKind, StreamParseResult, StreamParser, StreamWebSearchResult, ToolCall,
        ToolInput, build_tool_input_delta, parse_web_search_action,
    },
};

pub enum LlmStreamEvent {
    Open,
    Message { event: String, data: String },
}

pub enum LlmStreamError {
    Ended,
    Provider(ProviderError),
}

pub type LlmEventStream =
    Pin<Box<dyn Stream<Item = Result<LlmStreamEvent, LlmStreamError>> + Send>>;

pub fn plugin_event_stream(mut source: ProviderEventStream) -> LlmEventStream {
    Box::pin(async_stream::stream! {
        yield Ok(LlmStreamEvent::Open);
        while let Some(event) = source.next().await {
            match event {
                Ok(event) => match serde_json::to_string(&event) {
                    Ok(data) => yield Ok(LlmStreamEvent::Message {
                        event: "provider_event".to_string(),
                        data,
                    }),
                    Err(error) => {
                        yield Err(LlmStreamError::Provider(ProviderError::ResponseMapping(
                            error.to_string(),
                        )));
                        return;
                    }
                },
                Err(error) => {
                    yield Err(LlmStreamError::Provider(error));
                    return;
                }
            }
        }
        yield Err(LlmStreamError::Ended);
    })
}

pub fn provider_error_class(error: &ProviderError) -> &'static str {
    match error {
        ProviderError::InvalidManifest(_) => "invalid_manifest",
        ProviderError::Configuration(_) => "configuration",
        ProviderError::MissingCredential(_) => "missing_credential",
        ProviderError::UnsupportedCapability(_) => "unsupported_capability",
        ProviderError::PayloadMapping(_) => "payload_mapping",
        ProviderError::ResponseMapping(_) => "response_mapping",
        ProviderError::UrlNotAllowed(_) => "url_not_allowed",
        ProviderError::HeaderNotAllowed(_) => "header_not_allowed",
        ProviderError::Transport(_) => "transport",
        ProviderError::HttpStatus { .. } => "http_status",
        ProviderError::QuotaExhausted => "quota_exhausted",
        ProviderError::PaymentRequired => "payment_required",
        ProviderError::StreamEnded => "stream_ended",
        ProviderError::Cancelled => "cancelled",
        ProviderError::ResponseTooLarge => "response_too_large",
    }
}

pub fn build_plugin_chat_request(
    model: impl Into<String>,
    prompts: Vec<Prompt>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    tools: &HashMap<String, McpToolDescriptor>,
    web_search: bool,
    options: Value,
) -> ChatRequest {
    let mut tool_definitions = tools
        .values()
        .map(|tool| ToolDefinition {
            name: tool.openai_name.clone(),
            description: tool.description.clone(),
            parameters: tool.input_schema.clone(),
        })
        .collect::<Vec<_>>();
    tool_definitions.sort_by(|left, right| left.name.cmp(&right.name));

    ChatRequest {
        model: ModelId::new(model),
        messages: prompts.into_iter().map(prompt_to_message).collect(),
        temperature,
        max_tokens,
        tool_choice: (!tool_definitions.is_empty()).then_some(ToolChoice::Auto),
        tools: tool_definitions,
        web_search,
        options,
    }
}

pub async fn generate_provider_title(
    provider: &dyn ProviderPlugin,
    model: impl Into<String>,
    prompt: String,
) -> Result<PromptTitleResponse, ProviderError> {
    let response = generate_provider_text(
        provider,
        model,
        Some("Write a short conversation title. Return only the title.".to_string()),
        prompt,
        Some(32),
    )
    .await?;
    let title = response
        .text
        .trim()
        .trim_matches(['"', '\'', '`'])
        .chars()
        .take(120)
        .collect::<String>();
    if title.is_empty() {
        return Err(ProviderError::ResponseMapping(
            "title generation returned no text".to_string(),
        ));
    }
    Ok(PromptTitleResponse {
        title,
        input_tokens: response.input_tokens,
        output_tokens: response.output_tokens,
    })
}

pub async fn generate_provider_text(
    provider: &dyn ProviderPlugin,
    model: impl Into<String>,
    system: Option<String>,
    prompt: String,
    max_tokens: Option<u32>,
) -> Result<PromptTextResponse, ProviderError> {
    let mut messages = Vec::with_capacity(2);
    if let Some(system) = system.filter(|value| !value.trim().is_empty()) {
        messages.push(ChatMessage {
            role: PluginChatRole::System,
            content: vec![ContentPart::Text { text: system }],
            tool_calls: Vec::new(),
            tool_result: None,
        });
    }
    messages.push(ChatMessage {
        role: PluginChatRole::User,
        content: vec![ContentPart::Text { text: prompt }],
        tool_calls: Vec::new(),
        tool_result: None,
    });
    let request = ChatRequest {
        model: ModelId::new(model),
        messages,
        temperature: None,
        max_tokens,
        tools: Vec::new(),
        tool_choice: None,
        web_search: false,
        options: Value::Null,
    };
    generate_provider_response(provider, request).await
}

pub async fn generate_provider_response(
    provider: &dyn ProviderPlugin,
    request: ChatRequest,
) -> Result<PromptTextResponse, ProviderError> {
    let chat = provider
        .chat()
        .ok_or(ProviderError::UnsupportedCapability("chat"))?;
    let mut session = chat.start(request).await?;
    let mut stream = session.stream().await?;
    let mut text = String::new();
    let mut input_tokens = 0;
    let mut output_tokens = 0;
    while let Some(event) = stream.next().await {
        match event {
            Ok(ProviderEvent::TextDelta { text: delta }) => text.push_str(&delta),
            Ok(ProviderEvent::Usage { usage }) => {
                input_tokens = usage.input_tokens.unwrap_or(input_tokens);
                output_tokens = usage.output_tokens.unwrap_or(output_tokens);
            }
            Ok(ProviderEvent::Error { kind, message }) => {
                return Err(match kind {
                    ProviderEventErrorKind::QuotaExhausted => ProviderError::QuotaExhausted,
                    ProviderEventErrorKind::Provider => ProviderError::Transport(message),
                });
            }
            // Stream ended before the mapper saw a completion marker (e.g. max_tokens hit
            // before the provider sends its done event). Use whatever text was accumulated.
            Err(ProviderError::StreamEnded) => break,
            Err(e) => return Err(e),
            Ok(_) => {}
        }
    }
    if text.trim().is_empty() {
        return Err(ProviderError::ResponseMapping(
            "text generation returned no text".to_string(),
        ));
    }
    Ok(PromptTextResponse {
        text,
        input_tokens: i32::try_from(input_tokens).unwrap_or(i32::MAX),
        output_tokens: i32::try_from(output_tokens).unwrap_or(i32::MAX),
    })
}

fn prompt_to_message(prompt: Prompt) -> ChatMessage {
    let mut content = Vec::with_capacity(prompt.files.len() + 1);
    if !prompt.text.is_empty() {
        content.push(ContentPart::Text { text: prompt.text });
    }
    for file in prompt.files {
        match file.base64 {
            Some(data) if file.content_type.starts_with("image/") => {
                content.push(ContentPart::ImageBase64 {
                    data,
                    media_type: file.content_type,
                });
            }
            Some(data) => {
                content.push(ContentPart::File {
                    name: file.name,
                    data,
                    media_type: file.content_type,
                });
            }
            None => {
                content.push(ContentPart::FileReference {
                    id: file.id.to_string(),
                    name: file.name,
                    media_type: file.content_type,
                });
            }
        }
    }
    ChatMessage {
        role: match prompt.role {
            ChatRole::System => llm_plugin::ChatRole::System,
            ChatRole::User => llm_plugin::ChatRole::User,
            ChatRole::Assistant => llm_plugin::ChatRole::Assistant,
            ChatRole::Tool => llm_plugin::ChatRole::Tool,
        },
        content,
        tool_calls: Vec::new(),
        tool_result: None,
    }
}

#[derive(Default)]
pub struct PluginStreamParser {
    tools: std::sync::Mutex<HashMap<ToolCallId, PendingToolCall>>,
    server_tools: std::sync::Mutex<PendingServerTools>,
}

struct PendingToolCall {
    name: String,
    index: u32,
    arguments: String,
}

#[derive(Default)]
struct PendingServerTools {
    next_id: u64,
    ids_by_name: HashMap<String, String>,
    query_buffers: HashMap<String, String>,
}

impl PendingServerTools {
    fn start_id(&mut self, provider_id: Option<ToolCallId>, name: &str) -> String {
        let id = provider_id.map_or_else(
            || {
                let id = format!("plugin-web-search-{}", self.next_id);
                self.next_id = self.next_id.saturating_add(1);
                id
            },
            |id| id.to_string(),
        );
        self.ids_by_name.insert(name.to_string(), id.clone());
        id
    }

    fn event_id(&mut self, provider_id: Option<ToolCallId>, name: &str) -> String {
        if let Some(id) = provider_id {
            let id = id.to_string();
            self.ids_by_name.insert(name.to_string(), id.clone());
            return id;
        }
        self.ids_by_name
            .get(name)
            .cloned()
            .unwrap_or_else(|| self.start_id(None, name))
    }

    fn push_query(
        &mut self,
        id: &str,
        fragment: &str,
    ) -> Option<crate::services::provider_stream::StreamWebSearchAction> {
        let buffer = self.query_buffers.entry(id.to_string()).or_default();
        buffer.push_str(fragment);
        serde_json::from_str::<Value>(buffer)
            .ok()
            .and_then(|value| parse_web_search_action(&value))
    }
}

impl PluginStreamParser {
    pub fn new() -> Self {
        Self::default()
    }

    fn provider_event(&self, event: ProviderEvent) -> StreamParseResult {
        match event {
            ProviderEvent::MessageStart { request_id } => request_id
                .map(|request_id| request_id.to_string())
                .map(|request_id| StreamParseResult::MessageStart {
                    request_id,
                    input_tokens: None,
                    output_tokens: None,
                    cached_input_tokens: None,
                    cache_creation_tokens: None,
                })
                .unwrap_or(StreamParseResult::None),
            ProviderEvent::TextDelta { text } => StreamParseResult::TextDelta {
                text,
                request_id: None,
            },
            ProviderEvent::ReasoningDelta { text } => StreamParseResult::EventLog {
                event_type: "thinking_delta".to_string(),
                message: Some(text),
                data: None,
            },
            ProviderEvent::ToolCallStart { id, name, index } => {
                let Ok(mut tools) = self.tools.lock() else {
                    return provider_parse_error("plugin tool state is unavailable");
                };
                tools.insert(
                    id,
                    PendingToolCall {
                        name,
                        index,
                        arguments: String::new(),
                    },
                );
                StreamParseResult::None
            }
            ProviderEvent::ToolArgumentsDelta { id, fragment } => {
                let Ok(mut tools) = self.tools.lock() else {
                    return provider_parse_error("plugin tool state is unavailable");
                };
                let Some(tool) = tools.get_mut(&id) else {
                    return provider_parse_error("tool arguments arrived before tool start");
                };
                tool.arguments.push_str(&fragment);
                StreamParseResult::ToolInput(build_tool_input_delta(
                    fragment,
                    Some(tool.index),
                    Some(tool.name.clone()),
                    Some(id.to_string()),
                    None,
                ))
            }
            ProviderEvent::ToolCallEnd { id } => {
                let Ok(mut tools) = self.tools.lock() else {
                    return provider_parse_error("plugin tool state is unavailable");
                };
                let Some(tool) = tools.remove(&id) else {
                    return provider_parse_error("tool end arrived before tool start");
                };
                let input = if tool.arguments.trim().is_empty() {
                    Value::Object(Default::default())
                } else {
                    match serde_json::from_str(&tool.arguments) {
                        Ok(value) => value,
                        Err(_) => return provider_parse_error("tool arguments are not valid JSON"),
                    }
                };
                StreamParseResult::ToolCall(ToolCall {
                    tool_name: tool.name,
                    tool_id: Some(id.to_string()),
                    input: Some(ToolInput::Json(input)),
                    index: Some(tool.index),
                    raw: None,
                    web_search: None,
                })
            }
            ProviderEvent::ServerToolStart {
                id,
                name,
                query,
                queries,
            } => {
                let Ok(mut server_tools) = self.server_tools.lock() else {
                    return provider_parse_error("plugin server-tool state is unavailable");
                };
                let id = server_tools.start_id(id, &name);
                StreamParseResult::WebSearchAction {
                    tool_name: name,
                    tool_id: Some(id),
                    query,
                    queries: (!queries.is_empty()).then_some(queries),
                }
            }
            // The provider streams its search query in fragments; forward them the same way client
            // tool input is forwarded so the UI can show the query as it forms.
            ProviderEvent::ServerToolQueryDelta { id, name, fragment } => {
                let Ok(mut server_tools) = self.server_tools.lock() else {
                    return provider_parse_error("plugin server-tool state is unavailable");
                };
                let id = server_tools.event_id(id, &name);
                let web_search = server_tools.push_query(&id, &fragment);
                StreamParseResult::ToolInput(build_tool_input_delta(
                    fragment,
                    None,
                    Some(name),
                    Some(id),
                    web_search,
                ))
            }
            ProviderEvent::ServerToolResult { id, name, results } => {
                let Ok(mut server_tools) = self.server_tools.lock() else {
                    return provider_parse_error("plugin server-tool state is unavailable");
                };
                let id = server_tools.event_id(id, &name);
                StreamParseResult::WebSearchResult {
                    tool_name: name,
                    tool_id: Some(id),
                    results: results
                        .into_iter()
                        .map(|result| StreamWebSearchResult {
                            title: result.title,
                            url: result.url,
                            source: result.source,
                            page_age: result.page_age,
                            snippet: result.snippet,
                        })
                        .collect(),
                }
            }
            ProviderEvent::Usage { usage } => usage_result(usage),
            ProviderEvent::ProviderEvent { kind, data } => StreamParseResult::EventLog {
                event_type: kind,
                message: None,
                data: Some(data),
            },
            ProviderEvent::Error { kind, message } => StreamParseResult::Error {
                kind: match kind {
                    ProviderEventErrorKind::QuotaExhausted => StreamErrorKind::QuotaExhausted,
                    ProviderEventErrorKind::Provider => StreamErrorKind::ProviderError,
                },
                message,
            },
            ProviderEvent::Completed { .. } => StreamParseResult::None,
        }
    }
}

impl StreamParser for PluginStreamParser {
    fn parse_event(&self, data: &str) -> StreamParseResult {
        if data.is_empty() {
            return StreamParseResult::None;
        }
        match serde_json::from_str::<ProviderEvent>(data) {
            Ok(event) => self.provider_event(event),
            Err(_) => provider_parse_error("provider plugin emitted an invalid event"),
        }
    }
}

fn usage_result(usage: TokenUsage) -> StreamParseResult {
    StreamParseResult::TokenUsage {
        request_id: None,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        total_tokens: usage.total_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        cache_creation_tokens: usage.cache_creation_tokens,
    }
}

fn provider_parse_error(message: &str) -> StreamParseResult {
    StreamParseResult::Error {
        kind: StreamErrorKind::ProviderError,
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use futures_util::stream;
    use llm_plugin::{
        ChatCapabilities, ChatProvider, ChatSession, ProviderCapabilities, ProviderDescriptor,
        ProviderEvent, ProviderEventStream, ProviderId, ServerToolResultItem, ToolCallId,
        ToolResult,
    };

    use super::*;

    struct TitleProvider {
        descriptor: ProviderDescriptor,
    }

    impl TitleProvider {
        fn new() -> Self {
            Self {
                descriptor: ProviderDescriptor {
                    id: ProviderId::new("title-test"),
                    version: "1".to_string(),
                    name: "Title Test".to_string(),
                    capabilities: ProviderCapabilities {
                        chat: Some(ChatCapabilities {
                            streaming: true,
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                },
            }
        }
    }

    impl ProviderPlugin for TitleProvider {
        fn descriptor(&self) -> &ProviderDescriptor {
            &self.descriptor
        }

        fn chat(&self) -> Option<&dyn ChatProvider> {
            Some(self)
        }

        fn embeddings(&self) -> Option<&dyn llm_plugin::EmbeddingProvider> {
            None
        }

        fn images(&self) -> Option<&dyn llm_plugin::ImageProvider> {
            None
        }

        fn models(&self) -> Option<&dyn llm_plugin::ModelProvider> {
            None
        }
    }

    #[async_trait]
    impl ChatProvider for TitleProvider {
        async fn start(&self, request: ChatRequest) -> Result<Box<dyn ChatSession>, ProviderError> {
            assert_eq!(request.max_tokens, Some(32));
            assert!(!request.web_search);
            assert!(request.tools.is_empty());
            Ok(Box::new(TitleSession))
        }
    }

    struct TitleSession;

    #[async_trait]
    impl ChatSession for TitleSession {
        async fn stream(&mut self) -> Result<ProviderEventStream, ProviderError> {
            Ok(Box::pin(stream::iter(vec![
                Ok(ProviderEvent::TextDelta {
                    text: "`Provider-neutral title`".to_string(),
                }),
                Ok(ProviderEvent::Usage {
                    usage: TokenUsage {
                        input_tokens: Some(9),
                        output_tokens: Some(3),
                        ..Default::default()
                    },
                }),
                Ok(ProviderEvent::Completed {
                    finish_reason: None,
                }),
            ])))
        }

        async fn continue_with_tools(
            &mut self,
            _results: Vec<ToolResult>,
        ) -> Result<ProviderEventStream, ProviderError> {
            Err(ProviderError::UnsupportedCapability("tools"))
        }
    }

    #[test]
    fn plugin_tool_deltas_become_one_typed_call() {
        let parser = PluginStreamParser::new();
        let id = ToolCallId::new("call-1");
        assert!(matches!(
            parser.provider_event(ProviderEvent::ToolCallStart {
                id: id.clone(),
                name: "weather".to_string(),
                index: 0,
            }),
            StreamParseResult::None
        ));
        assert!(matches!(
            parser.provider_event(ProviderEvent::ToolArgumentsDelta {
                id: id.clone(),
                fragment: "{\"city\":".to_string(),
            }),
            StreamParseResult::ToolInput(_)
        ));
        let _ = parser.provider_event(ProviderEvent::ToolArgumentsDelta {
            id: id.clone(),
            fragment: "\"Paris\"}".to_string(),
        });
        let StreamParseResult::ToolCall(call) =
            parser.provider_event(ProviderEvent::ToolCallEnd { id })
        else {
            panic!("expected a completed tool call");
        };
        assert_eq!(call.tool_name, "weather");
        assert_eq!(
            call.input.and_then(|input| input.as_json().cloned()),
            Some(serde_json::json!({"city": "Paris"}))
        );
    }

    #[tokio::test]
    async fn title_generation_uses_the_canonical_chat_capability() {
        let provider = TitleProvider::new();
        let response = generate_provider_title(&provider, "model", "Explain Rust".to_string())
            .await
            .expect("title response");
        assert_eq!(response.title, "Provider-neutral title");
        assert_eq!(response.input_tokens, 9);
        assert_eq!(response.output_tokens, 3);
    }

    #[test]
    fn malformed_tool_arguments_are_rejected() {
        let parser = PluginStreamParser::new();
        let id = ToolCallId::new("call-1");
        let _ = parser.provider_event(ProviderEvent::ToolCallStart {
            id: id.clone(),
            name: "weather".to_string(),
            index: 0,
        });
        let _ = parser.provider_event(ProviderEvent::ToolArgumentsDelta {
            id: id.clone(),
            fragment: "{".to_string(),
        });
        assert!(matches!(
            parser.provider_event(ProviderEvent::ToolCallEnd { id }),
            StreamParseResult::Error { .. }
        ));
    }

    #[test]
    fn server_tool_events_receive_one_stable_stream_local_id() {
        let parser = PluginStreamParser::new();
        let StreamParseResult::WebSearchAction {
            tool_id: Some(id), ..
        } = parser.provider_event(ProviderEvent::ServerToolStart {
            id: None,
            name: "web_search".to_string(),
            query: None,
            queries: Vec::new(),
        })
        else {
            panic!("expected a web-search start");
        };

        let StreamParseResult::ToolInput(delta) =
            parser.provider_event(ProviderEvent::ServerToolQueryDelta {
                id: None,
                name: "web_search".to_string(),
                fragment: r#"{"query":"Rust"}"#.to_string(),
            })
        else {
            panic!("expected a web-search query delta");
        };
        assert_eq!(delta.tool_id.as_deref(), Some(id.as_str()));
        assert_eq!(
            delta.web_search.and_then(|action| action.query),
            Some("Rust".to_string())
        );

        let StreamParseResult::WebSearchResult {
            tool_id: Some(result_id),
            ..
        } = parser.provider_event(ProviderEvent::ServerToolResult {
            id: None,
            name: "web_search".to_string(),
            results: vec![ServerToolResultItem {
                title: "Rust".to_string(),
                url: "https://www.rust-lang.org/".to_string(),
                source: None,
                page_age: None,
                snippet: None,
            }],
        })
        else {
            panic!("expected web-search results");
        };
        assert_eq!(result_id, id);
    }

    #[test]
    fn result_only_server_tool_events_still_receive_an_id() {
        let parser = PluginStreamParser::new();
        assert!(matches!(
            parser.provider_event(ProviderEvent::ServerToolResult {
                id: None,
                name: "web_search".to_string(),
                results: Vec::new(),
            }),
            StreamParseResult::WebSearchResult {
                tool_id: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn chat_handler_has_one_provider_neutral_execution_path() {
        let source = include_str!("../handlers/chat_stream.rs");
        assert!(source.contains("resolve_provider("));
        assert!(source.contains("session.continue_with_tools(results)"));
        for provider_specific_api in [
            "LlmProviderConfig",
            "OpenaiStreamParser",
            "AnthropicStreamParser",
            "MistralStreamParser",
            "GeminiStreamParser",
            ".openai_chat_stream(",
            ".anthropic_chat_stream(",
            ".mistral_chat_stream(",
            ".gemini_chat_stream(",
            "get_title_generation_model",
            ".openai_get_title(",
            ".anthropic_get_title(",
            ".mistral_get_title(",
            ".gemini_get_title(",
        ] {
            assert!(
                !source.contains(provider_specific_api),
                "chat handler contains provider-specific integration: {provider_specific_api}"
            );
        }
    }
}
