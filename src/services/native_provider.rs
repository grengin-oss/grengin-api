// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use llm_plugin::{
    ChatCapabilities, ChatProvider, ChatRequest, ChatRole as PluginChatRole, ChatSession,
    ContentPart, FinishReason, ProviderCapabilities, ProviderDescriptor, ProviderError,
    ProviderEvent, ProviderEventErrorKind, ProviderEventStream, ProviderId, ProviderPlugin,
    RequestId, ServerToolResultItem, TokenUsage, ToolCallId, ToolChoice, ToolDefinition,
    ToolResult,
};
use reqwest::Client as ReqwestClient;
use reqwest_eventsource::{Event as ReqwestEvent, EventSource};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    config::setting::{AnthropicSettings, GeminiSettings, MistralSettings, OpenaiSettings},
    dto::{
        files::File,
        llm::{
            anthropic::{
                AnthropicContentBlock, AnthropicMessage, AnthropicRole, AnthropicTool,
                AnthropicToolUnion, AnthropicWebSearchTool,
            },
            gemini::{
                GeminiContent, GeminiFunctionCall, GeminiFunctionResponse, GeminiPart,
                normalize_gemini_parameters,
            },
            mistral::{
                MistralConversationFunctionResult, MistralMessage, MistralTool, MistralToolCall,
                MistralToolDefinition, MistralToolFunction,
            },
            openai::{
                OpenaiFunctionCallOutput, OpenaiInputItem, OpenaiTool, OpenaiToolChoice,
                OpenaiToolChoiceObject,
            },
        },
    },
    handlers::llm::{
        StreamErrorKind, StreamParseResult, StreamParser, ToolInput,
        anthropic::AnthropicStreamParser, gemini::GeminiStreamParser, mistral::MistralStreamParser,
        mistral_conversations::MistralConversationStreamParser, openai::OpenaiStreamParser,
    },
    llm::{
        prompt::Prompt,
        provider::{AnthropicApis, GeminiApis, MistralApis, OpenaiApis},
    },
    models::messages::ChatRole,
    state::SharedState,
};

#[derive(Debug, Error)]
pub enum ResolveProviderError {
    #[error("provider is not configured")]
    NotConfigured,
    #[error("provider is disabled")]
    Disabled,
    #[error("provider does not exist")]
    Unknown,
}

pub async fn resolve_provider(
    state: &SharedState,
    provider_key: &str,
    user_id: Uuid,
) -> Result<Arc<dyn ProviderPlugin>, ResolveProviderError> {
    let client = state.req_client.clone();
    let provider = match provider_key {
        "openai" => {
            let settings = state
                .settings
                .openai
                .read()
                .await
                .clone()
                .ok_or(ResolveProviderError::NotConfigured)?;
            ensure_enabled(settings.is_enabled)?;
            NativeProvider::new(client, user_id, NativeProviderKind::OpenAi(settings))
        }
        "anthropic" => {
            let settings = state
                .settings
                .anthropic
                .read()
                .await
                .clone()
                .ok_or(ResolveProviderError::NotConfigured)?;
            ensure_enabled(settings.is_enabled)?;
            NativeProvider::new(client, user_id, NativeProviderKind::Anthropic(settings))
        }
        "mistral" => {
            let settings = state
                .settings
                .mistral
                .read()
                .await
                .clone()
                .ok_or(ResolveProviderError::NotConfigured)?;
            ensure_enabled(settings.is_enabled)?;
            NativeProvider::new(client, user_id, NativeProviderKind::Mistral(settings))
        }
        "gemini" => {
            let settings = state
                .settings
                .gemini
                .read()
                .await
                .clone()
                .ok_or(ResolveProviderError::NotConfigured)?;
            ensure_enabled(settings.is_enabled)?;
            NativeProvider::new(client, user_id, NativeProviderKind::Gemini(settings))
        }
        _ => {
            return state
                .provider_registry
                .get_by_str(provider_key)
                .await
                .ok_or(ResolveProviderError::Unknown);
        }
    };
    Ok(Arc::new(provider))
}

fn ensure_enabled(enabled: bool) -> Result<(), ResolveProviderError> {
    if enabled {
        Ok(())
    } else {
        Err(ResolveProviderError::Disabled)
    }
}

#[derive(Clone)]
enum NativeProviderKind {
    OpenAi(OpenaiSettings),
    Anthropic(AnthropicSettings),
    Mistral(MistralSettings),
    Gemini(GeminiSettings),
}

impl NativeProviderKind {
    fn id(&self) -> &'static str {
        match self {
            Self::OpenAi(_) => "openai",
            Self::Anthropic(_) => "anthropic",
            Self::Mistral(_) => "mistral",
            Self::Gemini(_) => "gemini",
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::OpenAi(_) => "OpenAI",
            Self::Anthropic(_) => "Anthropic",
            Self::Mistral(_) => "Mistral",
            Self::Gemini(_) => "Gemini",
        }
    }
}

pub struct NativeProvider {
    client: ReqwestClient,
    user_id: Uuid,
    kind: NativeProviderKind,
    descriptor: ProviderDescriptor,
}

impl NativeProvider {
    fn new(client: ReqwestClient, user_id: Uuid, kind: NativeProviderKind) -> Self {
        let descriptor = ProviderDescriptor {
            id: ProviderId::new(kind.id()),
            version: "native-v1".to_string(),
            name: kind.name().to_string(),
            capabilities: ProviderCapabilities {
                chat: Some(ChatCapabilities {
                    streaming: true,
                    tools: true,
                    vision: true,
                    reasoning: true,
                }),
                embeddings: false,
                image_generation: false,
                model_listing: false,
            },
        };
        Self {
            client,
            user_id,
            kind,
            descriptor,
        }
    }
}

impl ProviderPlugin for NativeProvider {
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
impl ChatProvider for NativeProvider {
    async fn start(&self, request: ChatRequest) -> Result<Box<dyn ChatSession>, ProviderError> {
        NativeChatSession::start(
            self.client.clone(),
            self.user_id,
            self.kind.clone(),
            request,
        )
        .await
        .map(|session| Box::new(session) as Box<dyn ChatSession>)
    }
}

struct NativeChatSession {
    client: ReqwestClient,
    source: Option<EventSource>,
    protocol: NativeProtocol,
    observed: Arc<Mutex<ObservedTurn>>,
}

enum NativeProtocol {
    OpenAi {
        settings: OpenaiSettings,
        model: String,
        temperature: Option<f32>,
        user_id: Uuid,
        tools: Option<Vec<OpenaiTool>>,
        tool_choice: Option<OpenaiToolChoice>,
    },
    Anthropic {
        settings: AnthropicSettings,
        model: String,
        max_tokens: i32,
        temperature: Option<f32>,
        tools: Option<Vec<AnthropicToolUnion>>,
        messages: Vec<AnthropicMessage>,
        system: Option<String>,
    },
    Mistral {
        settings: MistralSettings,
        model: String,
        temperature: Option<f32>,
        tools: Option<Vec<MistralTool>>,
        tool_choice: Option<Value>,
        messages: Vec<MistralMessage>,
        use_conversations: bool,
        conversation_id: Option<String>,
    },
    Gemini {
        settings: GeminiSettings,
        model: String,
        temperature: Option<f32>,
        tools: Option<Value>,
        tool_config: Option<Value>,
        system_instruction: Option<Value>,
        contents: Vec<Value>,
    },
}

impl NativeProtocol {
    fn parser(&self) -> Box<dyn StreamParser> {
        match self {
            Self::OpenAi { .. } => Box::new(OpenaiStreamParser::new()),
            Self::Anthropic { .. } => Box::new(AnthropicStreamParser::new()),
            Self::Mistral {
                use_conversations: true,
                ..
            } => Box::new(MistralConversationStreamParser::new()),
            Self::Mistral { .. } => Box::new(MistralStreamParser::new()),
            Self::Gemini { .. } => Box::new(GeminiStreamParser::new()),
        }
    }

    fn wraps_named_sse_events(&self) -> bool {
        matches!(
            self,
            Self::Mistral {
                use_conversations: true,
                ..
            }
        )
    }
}

impl NativeChatSession {
    async fn start(
        client: ReqwestClient,
        user_id: Uuid,
        kind: NativeProviderKind,
        request: ChatRequest,
    ) -> Result<Self, ProviderError> {
        let prompts = request_to_prompts(&request);
        let model = request.model.to_string();
        let observed = Arc::new(Mutex::new(ObservedTurn::default()));

        let (source, protocol) = match kind {
            NativeProviderKind::OpenAi(settings) => {
                let tools = openai_tools(request.web_search, &request.tools);
                let tool_choice = openai_tool_choice(request.tool_choice.as_ref());
                let source = client
                    .openai_chat_stream(
                        &settings,
                        model.clone(),
                        request.temperature,
                        prompts,
                        &user_id,
                        tools.clone(),
                        tool_choice.clone(),
                        None,
                        None,
                    )
                    .await
                    .map_err(transport_error)?;
                (
                    source,
                    NativeProtocol::OpenAi {
                        settings,
                        model,
                        temperature: request.temperature,
                        user_id,
                        tools,
                        tool_choice,
                    },
                )
            }
            NativeProviderKind::Anthropic(settings) => {
                let tools = anthropic_tools(request.web_search, &request.tools);
                let (messages, system) = AnthropicMessage::from_prompts(prompts);
                let max_tokens = request
                    .max_tokens
                    .and_then(|value| i32::try_from(value).ok())
                    .unwrap_or(128_000);
                let source = client
                    .anthropic_chat_stream_with_messages(
                        &settings,
                        model.clone(),
                        max_tokens,
                        request.temperature,
                        messages.clone(),
                        system.clone(),
                        tools.clone(),
                    )
                    .await
                    .map_err(transport_error)?;
                (
                    source,
                    NativeProtocol::Anthropic {
                        settings,
                        model,
                        max_tokens,
                        temperature: request.temperature,
                        tools,
                        messages,
                        system,
                    },
                )
            }
            NativeProviderKind::Mistral(settings) => {
                let use_conversations = request.web_search || !request.tools.is_empty();
                let tools = mistral_tools(request.web_search, &request.tools, use_conversations);
                let tool_choice = mistral_tool_choice(request.tool_choice.as_ref(), &request.tools);
                let messages = MistralMessage::from_prompts(prompts.clone());
                let source = if use_conversations {
                    let (instructions, inputs) = mistral_agent_inputs(&prompts);
                    client
                        .mistral_conversation_start_stream(
                            &settings,
                            inputs,
                            tools.clone(),
                            mistral_completion_args(
                                request.temperature,
                                request.tool_choice.as_ref(),
                            ),
                            Some(model.clone()),
                            None,
                            (!instructions.is_empty()).then_some(instructions),
                        )
                        .await
                } else {
                    client
                        .mistral_chat_stream_with_messages(
                            &settings,
                            model.clone(),
                            request.temperature,
                            messages.clone(),
                            tools.clone(),
                            tool_choice.clone(),
                        )
                        .await
                }
                .map_err(transport_error)?;
                (
                    source,
                    NativeProtocol::Mistral {
                        settings,
                        model,
                        temperature: request.temperature,
                        tools,
                        tool_choice,
                        messages,
                        use_conversations,
                        conversation_id: None,
                    },
                )
            }
            NativeProviderKind::Gemini(settings) => {
                let tools = gemini_tools(request.web_search, &request.tools);
                let tool_config = gemini_tool_config(request.tool_choice.as_ref(), &request.tools);
                let (system_instruction, contents) = gemini_payload(&prompts);
                let source = client
                    .gemini_chat_stream_with_contents(
                        &settings,
                        model.clone(),
                        request.temperature,
                        system_instruction.clone(),
                        Value::Array(contents.clone()),
                        tools.clone(),
                        tool_config.clone(),
                    )
                    .await
                    .map_err(transport_error)?;
                (
                    source,
                    NativeProtocol::Gemini {
                        settings,
                        model,
                        temperature: request.temperature,
                        tools,
                        tool_config,
                        system_instruction,
                        contents,
                    },
                )
            }
        };

        Ok(Self {
            client,
            source: Some(source),
            protocol,
            observed,
        })
    }

    fn take_turn(&self) -> Result<ObservedTurn, ProviderError> {
        let mut observed = self.observed.lock().map_err(|_| {
            ProviderError::Transport("native provider session state is unavailable".to_string())
        })?;
        Ok(std::mem::take(&mut *observed))
    }
}

#[async_trait]
impl ChatSession for NativeChatSession {
    async fn stream(&mut self) -> Result<ProviderEventStream, ProviderError> {
        let source = self.source.take().ok_or_else(|| {
            ProviderError::Configuration("chat stream was already consumed".to_string())
        })?;
        Ok(normalize_stream(
            source,
            self.protocol.parser(),
            self.protocol.wraps_named_sse_events(),
            self.observed.clone(),
        ))
    }

    async fn continue_with_tools(
        &mut self,
        results: Vec<ToolResult>,
    ) -> Result<ProviderEventStream, ProviderError> {
        if results.is_empty() {
            return Err(ProviderError::Configuration(
                "tool continuation requires at least one result".to_string(),
            ));
        }
        let observed = self.take_turn()?;
        self.source = Some(
            match &mut self.protocol {
                NativeProtocol::OpenAi {
                    settings,
                    model,
                    temperature,
                    user_id,
                    tools,
                    tool_choice,
                } => {
                    let response_id = observed.response_id.ok_or_else(|| {
                        ProviderError::ResponseMapping(
                            "OpenAI continuation is missing the response id".to_string(),
                        )
                    })?;
                    let input = results
                        .into_iter()
                        .map(|result| {
                            OpenaiInputItem::FunctionCallOutput(OpenaiFunctionCallOutput {
                                item_type: "function_call_output".to_string(),
                                call_id: result.call_id.to_string(),
                                output: serde_json::to_string(&result.output)
                                    .unwrap_or_else(|_| "{}".to_string()),
                            })
                        })
                        .collect();
                    self.client
                        .openai_chat_stream(
                            settings,
                            model.clone(),
                            *temperature,
                            Vec::new(),
                            user_id,
                            tools.clone(),
                            tool_choice.clone(),
                            Some(response_id),
                            Some(input),
                        )
                        .await
                }
                NativeProtocol::Anthropic {
                    settings,
                    model,
                    max_tokens,
                    temperature,
                    tools,
                    messages,
                    system,
                } => {
                    let mut use_blocks = Vec::new();
                    if !observed.assistant_text.trim().is_empty() {
                        use_blocks.push(AnthropicContentBlock::Text {
                            text: observed.assistant_text.clone(),
                        });
                    }
                    let mut result_blocks = Vec::new();
                    for result in results {
                        let call = observed.call(result.call_id.as_str(), &result.name)?;
                        use_blocks.push(AnthropicContentBlock::ToolUse {
                            id: result.call_id.to_string(),
                            name: result.name.clone(),
                            input: call.arguments.clone(),
                        });
                        result_blocks.push(AnthropicContentBlock::ToolResult {
                            tool_use_id: result.call_id.to_string(),
                            content: serde_json::to_string(&result.output)
                                .unwrap_or_else(|_| "{}".to_string()),
                            is_error: Some(result.is_error),
                        });
                    }
                    messages.push(AnthropicMessage::with_blocks(
                        AnthropicRole::Assistant,
                        use_blocks,
                    ));
                    messages.push(AnthropicMessage::with_blocks(
                        AnthropicRole::User,
                        result_blocks,
                    ));
                    self.client
                        .anthropic_chat_stream_with_messages(
                            settings,
                            model.clone(),
                            *max_tokens,
                            *temperature,
                            messages.clone(),
                            system.clone(),
                            tools.clone(),
                        )
                        .await
                }
                NativeProtocol::Mistral {
                    settings,
                    model,
                    temperature,
                    tools,
                    tool_choice,
                    messages,
                    use_conversations,
                    conversation_id,
                } => {
                    if *use_conversations {
                        let resolved_conversation_id = observed
                            .conversation_id
                            .or_else(|| conversation_id.clone())
                            .ok_or_else(|| {
                                ProviderError::ResponseMapping(
                                    "Mistral continuation is missing the conversation id"
                                        .to_string(),
                                )
                            })?;
                        *conversation_id = Some(resolved_conversation_id.clone());
                        let entries = results
                            .into_iter()
                            .map(|result| MistralConversationFunctionResult {
                                object: "entry".to_string(),
                                result_type: "function.result".to_string(),
                                tool_call_id: result.call_id.to_string(),
                                result: serde_json::to_string(&result.output)
                                    .unwrap_or_else(|_| "{}".to_string()),
                            })
                            .collect::<Vec<_>>();
                        self.client
                            .mistral_conversation_append_stream(
                                settings,
                                resolved_conversation_id,
                                serde_json::to_value(entries).unwrap_or(Value::Array(Vec::new())),
                                None,
                                None,
                            )
                            .await
                    } else {
                        let mut calls = Vec::new();
                        let mut result_messages = Vec::new();
                        for result in results {
                            let call = observed.call(result.call_id.as_str(), &result.name)?;
                            calls.push(MistralToolCall {
                                id: result.call_id.to_string(),
                                call_type: "function".to_string(),
                                function: MistralToolFunction {
                                    name: result.name.clone(),
                                    arguments: serde_json::to_string(&call.arguments)
                                        .unwrap_or_else(|_| "{}".to_string()),
                                },
                            });
                            result_messages.push(MistralMessage::tool_response(
                                result.name,
                                result.call_id.to_string(),
                                serde_json::to_string(&result.output)
                                    .unwrap_or_else(|_| "{}".to_string()),
                            ));
                        }
                        messages.push(MistralMessage::assistant_with_tool_calls(
                            (!observed.assistant_text.trim().is_empty())
                                .then_some(observed.assistant_text.clone()),
                            calls,
                        ));
                        messages.extend(result_messages);
                        self.client
                            .mistral_chat_stream_with_messages(
                                settings,
                                model.clone(),
                                *temperature,
                                messages.clone(),
                                tools.clone(),
                                tool_choice.clone(),
                            )
                            .await
                    }
                }
                NativeProtocol::Gemini {
                    settings,
                    model,
                    temperature,
                    tools,
                    tool_config,
                    system_instruction,
                    contents,
                } => {
                    let mut model_parts = Vec::with_capacity(results.len());
                    let mut response_parts = Vec::with_capacity(results.len());
                    for result in results {
                        let call = observed.call(result.call_id.as_str(), &result.name)?;
                        model_parts.push(GeminiPart {
                            function_call: Some(GeminiFunctionCall {
                                id: result.call_id.to_string(),
                                name: result.name.clone(),
                                args: call.arguments.clone(),
                            }),
                            thought_signature: call.thought_signature.clone(),
                            ..GeminiPart::default()
                        });
                        response_parts.push(GeminiPart {
                            function_response: Some(GeminiFunctionResponse {
                                id: result.call_id.to_string(),
                                name: result.name,
                                response: json!({ "output": result.output }),
                            }),
                            ..GeminiPart::default()
                        });
                    }
                    contents.push(
                        serde_json::to_value(GeminiContent {
                            role: "model".to_string(),
                            parts: model_parts,
                        })
                        .unwrap_or(Value::Null),
                    );
                    contents.push(
                        serde_json::to_value(GeminiContent {
                            role: "user".to_string(),
                            parts: response_parts,
                        })
                        .unwrap_or(Value::Null),
                    );
                    self.client
                        .gemini_chat_stream_with_contents(
                            settings,
                            model.clone(),
                            *temperature,
                            system_instruction.clone(),
                            Value::Array(contents.clone()),
                            tools.clone(),
                            tool_config.clone(),
                        )
                        .await
                }
            }
            .map_err(transport_error)?,
        );

        self.stream().await
    }
}

fn transport_error(error: anyhow::Error) -> ProviderError {
    ProviderError::Transport(error.to_string())
}

#[derive(Default)]
struct ObservedTurn {
    response_id: Option<String>,
    conversation_id: Option<String>,
    assistant_text: String,
    calls: HashMap<String, ObservedCall>,
    ids_by_index: HashMap<u32, String>,
    next_index: u32,
}

#[derive(Default)]
struct ObservedCall {
    name: String,
    arguments_text: String,
    arguments: Value,
    thought_signature: Option<Value>,
    ended: bool,
}

impl ObservedTurn {
    fn call(&self, id: &str, name: &str) -> Result<&ObservedCall, ProviderError> {
        self.calls.get(id).ok_or_else(|| {
            ProviderError::ResponseMapping(format!(
                "tool result for {name} does not match a streamed tool call"
            ))
        })
    }

    fn resolve_call_id(&mut self, id: Option<String>, index: Option<u32>) -> (String, u32) {
        if let Some(id) = id.as_ref()
            && let Some((&known_index, _)) = self
                .ids_by_index
                .iter()
                .find(|(_, known_id)| *known_id == id)
        {
            return (id.clone(), index.unwrap_or(known_index));
        }

        let index = index.unwrap_or_else(|| {
            let current = self.next_index;
            self.next_index = self.next_index.saturating_add(1);
            current
        });
        self.next_index = self.next_index.max(index.saturating_add(1));
        if let Some(id) = id {
            self.ids_by_index.insert(index, id.clone());
            return (id, index);
        }
        if let Some(id) = self.ids_by_index.get(&index) {
            return (id.clone(), index);
        }
        let id = format!("native-tool-{index}");
        self.ids_by_index.insert(index, id.clone());
        (id, index)
    }
}

fn normalize_stream(
    mut source: EventSource,
    parser: Box<dyn StreamParser>,
    wraps_named_events: bool,
    observed: Arc<Mutex<ObservedTurn>>,
) -> ProviderEventStream {
    Box::pin(async_stream::stream! {
        while let Some(event) = source.next().await {
            match event {
                Ok(ReqwestEvent::Open) => {}
                Ok(ReqwestEvent::Message(message)) => {
                    let data = prepare_event_data(
                        &message.event,
                        &message.data,
                        wraps_named_events,
                        &observed,
                    );
                    let mut results = vec![parser.parse_event(&data)];
                    loop {
                        let pending = parser.parse_event("");
                        if matches!(pending, StreamParseResult::None) {
                            break;
                        }
                        results.push(pending);
                    }
                    for result in results {
                        match normalize_parse_result(result, &observed) {
                            Ok(events) => {
                                for event in events {
                                    yield Ok(event);
                                }
                            }
                            Err(error) => {
                                yield Err(error);
                                return;
                            }
                        }
                    }
                }
                Err(reqwest_eventsource::Error::StreamEnded) => {
                    match finish_pending_calls(&observed) {
                        Ok(events) => {
                            for event in events {
                                yield Ok(event);
                            }
                        }
                        Err(error) => {
                            yield Err(error);
                            return;
                        }
                    }
                    yield Ok(ProviderEvent::Completed { finish_reason: Some(FinishReason::Stop) });
                    return;
                }
                Err(reqwest_eventsource::Error::InvalidStatusCode(status, response)) => {
                    let error = match status.as_u16() {
                        402 => ProviderError::PaymentRequired,
                        429 => ProviderError::QuotaExhausted,
                        _ => ProviderError::HttpStatus {
                            status: status.as_u16(),
                            message: read_provider_error(response)
                                .await
                                .unwrap_or_else(|| status.canonical_reason().unwrap_or("provider error").to_string()),
                        },
                    };
                    yield Err(error);
                    return;
                }
                Err(error) => {
                    yield Err(ProviderError::Transport(error.to_string()));
                    return;
                }
            }
        }
        match finish_pending_calls(&observed) {
            Ok(events) => {
                for event in events {
                    yield Ok(event);
                }
            }
            Err(error) => {
                yield Err(error);
                return;
            }
        }
        yield Ok(ProviderEvent::Completed { finish_reason: Some(FinishReason::Stop) });
    })
}

async fn read_provider_error(mut response: reqwest::Response) -> Option<String> {
    const MAX_ERROR_BYTES: usize = 8 * 1024;

    let mut body = Vec::new();
    while body.len() < MAX_ERROR_BYTES {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) => return None,
        };
        let remaining = MAX_ERROR_BYTES - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if chunk.len() >= remaining {
            break;
        }
    }
    String::from_utf8(body)
        .ok()
        .and_then(|body| extract_provider_message(&body))
}

fn prepare_event_data(
    event_name: &str,
    data: &str,
    wraps_named_events: bool,
    observed: &Arc<Mutex<ObservedTurn>>,
) -> String {
    if !wraps_named_events {
        return data.to_string();
    }
    let parsed =
        serde_json::from_str::<Value>(data).unwrap_or_else(|_| Value::String(data.to_string()));
    let event_type = if event_name.is_empty() {
        parsed
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
    } else {
        event_name
    };
    if event_type == "conversation.response.started"
        && let Some(id) = parsed.get("conversation_id").and_then(Value::as_str)
        && let Ok(mut state) = observed.lock()
    {
        state.conversation_id = Some(id.to_string());
    }
    if event_name.is_empty() {
        parsed.to_string()
    } else {
        json!({ "event": event_name, "data": parsed }).to_string()
    }
}

fn normalize_parse_result(
    result: StreamParseResult,
    observed: &Arc<Mutex<ObservedTurn>>,
) -> Result<Vec<ProviderEvent>, ProviderError> {
    let mut state = observed.lock().map_err(|_| {
        ProviderError::Transport("native provider stream state is unavailable".to_string())
    })?;
    let events = match result {
        StreamParseResult::None => Vec::new(),
        StreamParseResult::MessageStart {
            request_id,
            input_tokens,
            output_tokens,
            cached_input_tokens,
            cache_creation_tokens,
        } => {
            state.response_id = (!request_id.is_empty()).then_some(request_id.clone());
            let mut events = vec![ProviderEvent::MessageStart {
                request_id: (!request_id.is_empty()).then(|| RequestId::new(request_id)),
            }];
            if input_tokens.is_some()
                || output_tokens.is_some()
                || cached_input_tokens.is_some()
                || cache_creation_tokens.is_some()
            {
                events.push(ProviderEvent::Usage {
                    usage: TokenUsage {
                        input_tokens,
                        output_tokens,
                        total_tokens: None,
                        cached_input_tokens,
                        cache_creation_tokens,
                    },
                });
            }
            events
        }
        StreamParseResult::TextDelta { text, .. } => {
            state.assistant_text.push_str(&text);
            vec![ProviderEvent::TextDelta { text }]
        }
        StreamParseResult::ToolInput(delta) => {
            if delta.is_web_search() {
                vec![ProviderEvent::ServerToolQueryDelta {
                    id: delta.tool_id.map(ToolCallId::new),
                    name: delta.tool_name.unwrap_or_else(|| "web_search".to_string()),
                    fragment: delta.partial_json,
                }]
            } else {
                let (id, index) = state.resolve_call_id(delta.tool_id, delta.index);
                let name = delta.tool_name.unwrap_or_else(|| "tool_call".to_string());
                let is_new = !state.calls.contains_key(&id);
                let call = state.calls.entry(id.clone()).or_default();
                if call.name.is_empty() {
                    call.name = name.clone();
                }
                let fragment = if !call.arguments_text.is_empty()
                    && delta.partial_json.starts_with(&call.arguments_text)
                {
                    delta.partial_json[call.arguments_text.len()..].to_string()
                } else {
                    delta.partial_json
                };
                call.arguments_text.push_str(&fragment);
                let mut events = Vec::new();
                if is_new {
                    events.push(ProviderEvent::ToolCallStart {
                        id: ToolCallId::new(id.clone()),
                        name,
                        index,
                    });
                }
                events.push(ProviderEvent::ToolArgumentsDelta {
                    id: ToolCallId::new(id),
                    fragment,
                });
                events
            }
        }
        StreamParseResult::ToolCall(call) if call.is_web_search() => {
            let action = call.web_search.or_else(|| {
                call.input
                    .as_ref()
                    .and_then(ToolInput::as_json)
                    .and_then(crate::handlers::llm::parse_web_search_action)
            });
            vec![ProviderEvent::ServerToolStart {
                id: call.tool_id.map(ToolCallId::new),
                name: call.tool_name,
                query: action.as_ref().and_then(|value| value.query.clone()),
                queries: action.and_then(|value| value.queries).unwrap_or_default(),
            }]
        }
        StreamParseResult::ToolCall(call) => {
            let (id, index) = state.resolve_call_id(call.tool_id, call.index);
            let arguments = tool_input_value(call.input.as_ref());
            let serialized = serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string());
            let is_partial_start =
                call.raw.is_none() && arguments.as_object().is_some_and(serde_json::Map::is_empty);
            let is_new = !state.calls.contains_key(&id);
            let entry = state.calls.entry(id.clone()).or_default();
            if entry.name.is_empty() {
                entry.name = call.tool_name.clone();
            }
            entry.arguments = arguments;
            entry.thought_signature = call.raw.as_ref().and_then(|raw| {
                raw.get("thoughtSignature")
                    .cloned()
                    .or_else(|| raw.get("thought_signature").cloned())
            });
            let needs_arguments = !is_partial_start && entry.arguments_text.trim().is_empty();
            if needs_arguments {
                entry.arguments_text = serialized.clone();
            }
            entry.ended = !is_partial_start;
            let mut events = Vec::new();
            if is_new {
                events.push(ProviderEvent::ToolCallStart {
                    id: ToolCallId::new(id.clone()),
                    name: call.tool_name,
                    index,
                });
            }
            if needs_arguments {
                events.push(ProviderEvent::ToolArgumentsDelta {
                    id: ToolCallId::new(id.clone()),
                    fragment: serialized,
                });
            }
            if !is_partial_start {
                events.push(ProviderEvent::ToolCallEnd {
                    id: ToolCallId::new(id),
                });
            }
            events
        }
        StreamParseResult::WebSearchAction {
            tool_name,
            tool_id,
            query,
            queries,
        } => vec![ProviderEvent::ServerToolStart {
            id: tool_id.map(ToolCallId::new),
            name: tool_name,
            query,
            queries: queries.unwrap_or_default(),
        }],
        StreamParseResult::WebSearchResult {
            tool_name,
            tool_id,
            results,
        } => vec![ProviderEvent::ServerToolResult {
            id: tool_id.map(ToolCallId::new),
            name: tool_name,
            results: results
                .into_iter()
                .map(|result| ServerToolResultItem {
                    title: result.title,
                    url: result.url,
                    source: result.source,
                    page_age: result.page_age,
                    snippet: result.snippet,
                })
                .collect(),
        }],
        StreamParseResult::ToolResult(result) => vec![ProviderEvent::ProviderEvent {
            kind: "provider_tool_result".to_string(),
            data: json!({
                "name": result.tool_name,
                "id": result.tool_id,
                "output": result.output,
            }),
        }],
        StreamParseResult::TokenUsage {
            input_tokens,
            output_tokens,
            total_tokens,
            cached_input_tokens,
            cache_creation_tokens,
            ..
        } => vec![ProviderEvent::Usage {
            usage: TokenUsage {
                input_tokens,
                output_tokens,
                total_tokens,
                cached_input_tokens,
                cache_creation_tokens,
            },
        }],
        StreamParseResult::EventLog {
            event_type: _,
            message: Some(text),
            ..
        } => vec![ProviderEvent::ReasoningDelta { text }],
        StreamParseResult::EventLog {
            event_type, data, ..
        } => vec![ProviderEvent::ProviderEvent {
            kind: event_type,
            data: data.unwrap_or(Value::Null),
        }],
        StreamParseResult::Error { kind, message } => vec![ProviderEvent::Error {
            kind: match kind {
                StreamErrorKind::QuotaExhausted => ProviderEventErrorKind::QuotaExhausted,
                StreamErrorKind::ProviderError => ProviderEventErrorKind::Provider,
            },
            message,
        }],
    };
    Ok(events)
}

fn finish_pending_calls(
    observed: &Arc<Mutex<ObservedTurn>>,
) -> Result<Vec<ProviderEvent>, ProviderError> {
    let mut state = observed.lock().map_err(|_| {
        ProviderError::Transport("native provider stream state is unavailable".to_string())
    })?;
    let mut calls = state
        .ids_by_index
        .iter()
        .map(|(index, id)| (*index, id.clone()))
        .collect::<Vec<_>>();
    calls.sort_unstable_by_key(|(index, _)| *index);
    let mut events = Vec::new();
    for (_, id) in calls {
        let Some(call) = state.calls.get_mut(&id) else {
            continue;
        };
        if call.ended {
            continue;
        }
        call.arguments = if call.arguments_text.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&call.arguments_text).map_err(|_| {
                ProviderError::ResponseMapping(format!(
                    "tool arguments for {} are not valid JSON",
                    call.name
                ))
            })?
        };
        call.ended = true;
        events.push(ProviderEvent::ToolCallEnd {
            id: ToolCallId::new(id),
        });
    }
    Ok(events)
}

fn request_to_prompts(request: &ChatRequest) -> Vec<Prompt> {
    request
        .messages
        .iter()
        .map(|message| {
            let mut text = String::new();
            let mut files = Vec::new();
            for part in &message.content {
                match part {
                    ContentPart::Text { text: part } => text.push_str(part),
                    ContentPart::ImageUrl { url, .. } => {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(url);
                    }
                    ContentPart::ImageBase64 { data, media_type } => files.push(File {
                        id: Uuid::nil(),
                        size: None,
                        name: "image".to_string(),
                        content_type: media_type.clone(),
                        openai_id: None,
                        base64: Some(data.clone()),
                    }),
                    ContentPart::File {
                        name,
                        data,
                        media_type,
                    } => files.push(File {
                        id: Uuid::nil(),
                        size: None,
                        name: name.clone(),
                        content_type: media_type.clone(),
                        openai_id: None,
                        base64: Some(data.clone()),
                    }),
                    ContentPart::FileReference {
                        id,
                        name,
                        media_type,
                    } => files.push(File {
                        id: Uuid::parse_str(id).unwrap_or_else(|_| Uuid::nil()),
                        size: None,
                        name: name.clone(),
                        content_type: media_type.clone(),
                        openai_id: None,
                        base64: None,
                    }),
                }
            }
            Prompt {
                text,
                role: match message.role {
                    PluginChatRole::System => ChatRole::System,
                    PluginChatRole::User => ChatRole::User,
                    PluginChatRole::Assistant => ChatRole::Assistant,
                    PluginChatRole::Tool => ChatRole::Tool,
                },
                files,
            }
        })
        .collect()
}

fn openai_tools(web_search: bool, definitions: &[ToolDefinition]) -> Option<Vec<OpenaiTool>> {
    let mut tools = Vec::new();
    if web_search {
        tools.push(OpenaiTool::web_search());
    }
    tools.extend(definitions.iter().map(|tool| OpenaiTool::Function {
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters: tool.parameters.clone(),
        strict: None,
    }));
    (!tools.is_empty()).then_some(tools)
}

fn openai_tool_choice(choice: Option<&ToolChoice>) -> Option<OpenaiToolChoice> {
    match choice {
        Some(ToolChoice::Auto) => Some(OpenaiToolChoice::String("auto".to_string())),
        Some(ToolChoice::None) => Some(OpenaiToolChoice::String("none".to_string())),
        Some(ToolChoice::Required) => Some(OpenaiToolChoice::String("required".to_string())),
        Some(ToolChoice::Named(name)) => Some(OpenaiToolChoice::Object(OpenaiToolChoiceObject {
            tool_type: "function".to_string(),
            name: Some(name.clone()),
        })),
        None => None,
    }
}

fn anthropic_tools(
    web_search: bool,
    definitions: &[ToolDefinition],
) -> Option<Vec<AnthropicToolUnion>> {
    let mut tools = Vec::new();
    if web_search {
        tools.push(AnthropicToolUnion::WebSearchTool(
            AnthropicWebSearchTool::new(Some(5)),
        ));
    }
    tools.extend(definitions.iter().map(|tool| {
        AnthropicToolUnion::ClientTool(AnthropicTool {
            name: tool.name.clone(),
            description: tool
                .description
                .clone()
                .unwrap_or_else(|| tool.name.clone()),
            input_schema: tool.parameters.clone(),
        })
    }));
    (!tools.is_empty()).then_some(tools)
}

fn mistral_tools(
    web_search: bool,
    definitions: &[ToolDefinition],
    use_conversations: bool,
) -> Option<Vec<MistralTool>> {
    if !use_conversations {
        return None;
    }
    let mut tools = Vec::new();
    if web_search {
        tools.push(MistralTool::WebSearch);
    }
    tools.extend(definitions.iter().map(|tool| MistralTool::Function {
        function: MistralToolDefinition {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.parameters.clone(),
        },
    }));
    (!tools.is_empty()).then_some(tools)
}

fn mistral_tool_choice(choice: Option<&ToolChoice>, tools: &[ToolDefinition]) -> Option<Value> {
    if tools.is_empty() {
        return None;
    }
    match choice {
        Some(ToolChoice::Named(name)) => {
            Some(json!({ "type": "function", "function": { "name": name } }))
        }
        Some(ToolChoice::None) => Some(json!("none")),
        Some(ToolChoice::Required) => Some(json!("required")),
        _ => Some(json!("auto")),
    }
}

fn mistral_completion_args(temperature: Option<f32>, choice: Option<&ToolChoice>) -> Option<Value> {
    let mut args = serde_json::Map::new();
    if let Some(temperature) = temperature {
        args.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(ToolChoice::Named(name)) = choice {
        args.insert(
            "tool_choice".to_string(),
            json!({ "type": "function", "function": { "name": name } }),
        );
    }
    (!args.is_empty()).then_some(Value::Object(args))
}

fn mistral_agent_inputs(prompts: &[Prompt]) -> (String, Value) {
    let mut instructions = Vec::new();
    let entries = prompts
        .iter()
        .filter_map(|prompt| match prompt.role {
            ChatRole::System => {
                if !prompt.text.trim().is_empty() {
                    instructions.push(prompt.text.clone());
                }
                None
            }
            ChatRole::User | ChatRole::Assistant => Some(json!({
                "object": "entry",
                "type": "message.input",
                "role": prompt.role,
                "content": prompt.text,
            })),
            ChatRole::Tool => None,
        })
        .collect::<Vec<_>>();
    (instructions.join("\n\n"), Value::Array(entries))
}

fn gemini_tools(web_search: bool, definitions: &[ToolDefinition]) -> Option<Value> {
    let mut tools = Vec::new();
    if web_search {
        tools.push(json!({ "google_search": {} }));
    }
    if !definitions.is_empty() {
        let functions = definitions
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": normalize_gemini_parameters(&tool.parameters),
                })
            })
            .collect::<Vec<_>>();
        tools.push(json!({ "function_declarations": functions }));
    }
    (!tools.is_empty()).then_some(Value::Array(tools))
}

fn gemini_tool_config(
    choice: Option<&ToolChoice>,
    definitions: &[ToolDefinition],
) -> Option<Value> {
    if definitions.is_empty() {
        return None;
    }
    let config = match choice {
        Some(ToolChoice::Named(name)) => {
            json!({ "mode": "ANY", "allowed_function_names": [name] })
        }
        Some(ToolChoice::None) => json!({ "mode": "NONE" }),
        Some(ToolChoice::Required) => json!({ "mode": "ANY" }),
        _ => json!({ "mode": "AUTO" }),
    };
    Some(json!({ "function_calling_config": config }))
}

fn gemini_payload(prompts: &[Prompt]) -> (Option<Value>, Vec<Value>) {
    let mut system = Vec::new();
    let mut contents = Vec::new();
    for prompt in prompts {
        match prompt.role {
            ChatRole::System => {
                if !prompt.text.trim().is_empty() {
                    system.push(prompt.text.clone());
                }
            }
            ChatRole::User | ChatRole::Assistant => contents.push(json!({
                "role": if prompt.role == ChatRole::Assistant { "model" } else { "user" },
                "parts": [{ "text": prompt.text }],
            })),
            ChatRole::Tool => {}
        }
    }
    let instruction =
        (!system.is_empty()).then(|| json!({ "parts": [{ "text": system.join("\n\n") }] }));
    (instruction, contents)
}

fn tool_input_value(input: Option<&ToolInput>) -> Value {
    match input {
        Some(ToolInput::Json(value)) => value.clone(),
        Some(ToolInput::Text(value)) => {
            serde_json::from_str(value).unwrap_or_else(|_| json!({ "value": value }))
        }
        None => json!({}),
    }
}

fn extract_provider_message(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body).ok().and_then(|value| {
        value
            .pointer("/error/message")
            .or_else(|| value.get("message"))
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

#[cfg(test)]
mod tests {
    use llm_plugin::{ChatMessage, ModelId};

    use super::*;

    #[test]
    fn canonical_file_references_survive_native_conversion() {
        let request = ChatRequest {
            model: ModelId::new("test"),
            messages: vec![ChatMessage {
                role: PluginChatRole::User,
                content: vec![ContentPart::FileReference {
                    id: "fb2526f6-0fc8-4c37-a4bd-06d0d7573ec9".to_string(),
                    name: "report.pdf".to_string(),
                    media_type: "application/pdf".to_string(),
                }],
                tool_calls: Vec::new(),
                tool_result: None,
            }],
            temperature: None,
            max_tokens: None,
            tools: Vec::new(),
            tool_choice: None,
            web_search: false,
            options: Value::Null,
        };
        let prompts = request_to_prompts(&request);
        assert_eq!(prompts[0].files[0].name, "report.pdf");
        assert_ne!(prompts[0].files[0].id, Uuid::nil());
    }

    #[test]
    fn native_tool_builders_preserve_canonical_schema() {
        let definitions = vec![ToolDefinition {
            name: "weather".to_string(),
            description: Some("Current weather".to_string()),
            parameters: json!({"type":"object","properties":{"city":{"type":"string"}}}),
        }];
        let tools = openai_tools(false, &definitions).expect("tool list");
        let serialized = serde_json::to_value(tools).expect("serializable tools");
        assert_eq!(serialized.pointer("/0/name"), Some(&json!("weather")));
        assert_eq!(
            serialized.pointer("/0/parameters/properties/city/type"),
            Some(&json!("string"))
        );
    }

    #[test]
    fn full_tool_call_normalizes_to_start_delta_and_end() {
        let observed = Arc::new(Mutex::new(ObservedTurn::default()));
        let events = normalize_parse_result(
            StreamParseResult::ToolCall(crate::handlers::llm::ToolCall {
                tool_name: "weather".to_string(),
                tool_id: Some("call-1".to_string()),
                input: Some(ToolInput::Json(json!({"city":"Paris"}))),
                index: Some(0),
                raw: None,
                web_search: None,
            }),
            &observed,
        )
        .expect("normalization");
        assert!(matches!(events[0], ProviderEvent::ToolCallStart { .. }));
        assert!(matches!(
            events[1],
            ProviderEvent::ToolArgumentsDelta { .. }
        ));
        assert!(matches!(events[2], ProviderEvent::ToolCallEnd { .. }));
    }

    #[test]
    fn streamed_tool_start_stays_open_until_arguments_finish() {
        let observed = Arc::new(Mutex::new(ObservedTurn::default()));
        let start = normalize_parse_result(
            StreamParseResult::ToolCall(crate::handlers::llm::ToolCall {
                tool_name: "weather".to_string(),
                tool_id: Some("call-1".to_string()),
                input: Some(ToolInput::Json(json!({}))),
                index: Some(0),
                raw: None,
                web_search: None,
            }),
            &observed,
        )
        .expect("start normalization");
        assert_eq!(start.len(), 1);
        assert!(matches!(start[0], ProviderEvent::ToolCallStart { .. }));

        let first = normalize_parse_result(
            StreamParseResult::ToolInput(crate::handlers::llm::ToolInputDelta {
                partial_json: "{\"city\"".to_string(),
                index: Some(0),
                tool_name: Some("weather".to_string()),
                tool_id: Some("call-1".to_string()),
                web_search: None,
            }),
            &observed,
        )
        .expect("first delta");
        assert!(matches!(
            &first[0],
            ProviderEvent::ToolArgumentsDelta { fragment, .. } if fragment == "{\"city\""
        ));

        let cumulative = normalize_parse_result(
            StreamParseResult::ToolInput(crate::handlers::llm::ToolInputDelta {
                partial_json: "{\"city\":\"Paris\"}".to_string(),
                index: Some(0),
                tool_name: Some("weather".to_string()),
                tool_id: Some("call-1".to_string()),
                web_search: None,
            }),
            &observed,
        )
        .expect("cumulative delta");
        assert!(matches!(
            &cumulative[0],
            ProviderEvent::ToolArgumentsDelta { fragment, .. } if fragment == ":\"Paris\"}"
        ));

        let end = finish_pending_calls(&observed).expect("finish pending call");
        assert!(matches!(end[0], ProviderEvent::ToolCallEnd { .. }));
        let state = observed.lock().expect("observed state");
        assert_eq!(state.calls["call-1"].arguments, json!({"city":"Paris"}));
    }

    #[test]
    fn tool_call_id_keeps_its_index_when_later_chunks_omit_it() {
        let mut turn = ObservedTurn::default();
        assert_eq!(
            turn.resolve_call_id(Some("call-7".to_string()), Some(7)),
            ("call-7".to_string(), 7)
        );
        assert_eq!(
            turn.resolve_call_id(Some("call-7".to_string()), None),
            ("call-7".to_string(), 7)
        );
        assert_eq!(
            turn.resolve_call_id(Some("call-8".to_string()), None),
            ("call-8".to_string(), 8)
        );
    }

    #[test]
    fn pending_tool_calls_finish_in_stream_index_order() {
        let observed = Arc::new(Mutex::new(ObservedTurn::default()));
        {
            let mut turn = observed.lock().expect("observed state");
            turn.resolve_call_id(Some("second".to_string()), Some(2));
            turn.resolve_call_id(Some("first".to_string()), Some(1));
            turn.calls.insert(
                "second".to_string(),
                ObservedCall {
                    name: "second".to_string(),
                    arguments_text: "{}".to_string(),
                    ..ObservedCall::default()
                },
            );
            turn.calls.insert(
                "first".to_string(),
                ObservedCall {
                    name: "first".to_string(),
                    arguments_text: "{}".to_string(),
                    ..ObservedCall::default()
                },
            );
        }

        let events = finish_pending_calls(&observed).expect("finish pending calls");
        assert!(matches!(
            &events[0],
            ProviderEvent::ToolCallEnd { id } if id.as_str() == "first"
        ));
        assert!(matches!(
            &events[1],
            ProviderEvent::ToolCallEnd { id } if id.as_str() == "second"
        ));
    }
}
