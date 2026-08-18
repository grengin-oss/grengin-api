// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    net::IpAddr,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::StreamExt;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::{
    Client, Method, RequestBuilder, Response, StatusCode,
    header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue},
    multipart::{Form, Part},
};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::{net::lookup_host, sync::Mutex};
use url::Url;

use crate::{
    BodyEncoding, ChatMessage, ChatOperation, ChatProvider, ChatRequest, ChatRole, ChatSession,
    ContentPart, CredentialType, EmbeddingOperation, EmbeddingProvider, EmbeddingRequest,
    EmbeddingResult, GeneratedImage, HeaderValueSpec, HttpMethod, ImageBodyEncoding,
    ImageOperation, ImageProvider, ImageRequest, ImageResult, ManifestCapabilities, ManifestModel,
    MappingContext, ModelId, ModelListOperation, ModelProvider, ProviderCapabilities,
    ProviderDescriptor, ProviderError, ProviderEvent, ProviderEventStream, ProviderManifestV1,
    ProviderModel, ProviderPlugin, RequestSpec, SseDecoder, SseEventMapper, StructuredBodyEncoding,
    TokenUsage, ToolCall, ToolCallId, ToolResult, UsageMapping, capture_values, evaluate_mapping,
    resolve_path,
    security::{validate_destination_ip, validate_header_name, validate_provider_url},
};

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const MAX_ERROR_BODY_BYTES: usize = 8 * 1024;
const MAX_SSE_EVENT_BYTES: usize = 2 * 1024 * 1024;
const MAX_CONFIGURATION_BYTES: usize = 1024 * 1024;

/// Percent-encodes everything except the RFC 3986 unreserved set, so a templated value can never
/// introduce URL structure while ordinary model ids such as `gpt-4.1-mini` survive intact.
const PATH_VALUE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

#[derive(Clone)]
pub struct ProviderRuntimeConfig {
    pub base_url_override: Option<String>,
    pub configuration: Value,
    pub credentials: BTreeMap<String, String>,
    pub allow_insecure_http: bool,
    pub allow_private_network: bool,
    pub default_timeout_ms: u64,
    pub max_response_bytes: usize,
}

impl Default for ProviderRuntimeConfig {
    fn default() -> Self {
        Self {
            base_url_override: None,
            configuration: Value::Object(Default::default()),
            credentials: BTreeMap::new(),
            allow_insecure_http: false,
            allow_private_network: false,
            default_timeout_ms: DEFAULT_TIMEOUT_MS,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        }
    }
}

impl fmt::Debug for ProviderRuntimeConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderRuntimeConfig")
            .field("base_url_override", &self.base_url_override)
            .field(
                "configuration_keys",
                &self
                    .configuration
                    .as_object()
                    .map(|configuration| configuration.keys().collect::<Vec<_>>()),
            )
            .field(
                "credential_slots",
                &self.credentials.keys().collect::<Vec<_>>(),
            )
            .field("allow_insecure_http", &self.allow_insecure_http)
            .field("allow_private_network", &self.allow_private_network)
            .field("default_timeout_ms", &self.default_timeout_ms)
            .field("max_response_bytes", &self.max_response_bytes)
            .finish()
    }
}

#[derive(Clone)]
pub struct DeclarativeProvider {
    manifest: Arc<ProviderManifestV1>,
    descriptor: ProviderDescriptor,
    base_url: Url,
    client: Client,
    stream_client: Client,
    runtime: Arc<ProviderRuntimeConfig>,
}

/// Chooses how an operation's `timeoutMs` is enforced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestMode {
    /// Total deadline for the whole exchange; correct for responses read into memory.
    Buffered,
    /// Inactivity deadline only, so a long-running stream is not truncated mid-answer.
    Streaming,
}

impl DeclarativeProvider {
    pub fn new(
        manifest: ProviderManifestV1,
        runtime: ProviderRuntimeConfig,
    ) -> Result<Self, ProviderError> {
        manifest.validate()?;
        validate_credentials(&manifest, &runtime)?;
        validate_configuration(&manifest, &runtime.configuration)?;

        let base_url = runtime
            .base_url_override
            .as_deref()
            .unwrap_or(&manifest.base_url);
        let mut base_url = Url::parse(base_url)
            .map_err(|error| ProviderError::Configuration(format!("invalid base URL: {error}")))?;
        validate_provider_url(
            &base_url,
            runtime.allow_insecure_http,
            runtime.allow_private_network,
        )?;
        if !base_url.path().ends_with('/') {
            let path = format!("{}/", base_url.path());
            base_url.set_path(&path);
        }

        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(ProviderError::from)?;
        // A chat stream stays open for as long as the model keeps talking, so its timeout has to
        // bound silence between chunks instead of the total exchange.
        let stream_timeout = Duration::from_millis(clamp_timeout(
            manifest
                .operations
                .chat_stream
                .as_ref()
                .and_then(|operation| operation.request.timeout_ms),
            runtime.default_timeout_ms,
        ));
        let stream_client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(stream_timeout)
            .read_timeout(stream_timeout)
            .build()
            .map_err(ProviderError::from)?;
        let descriptor = manifest.descriptor();

        Ok(Self {
            manifest: Arc::new(manifest),
            descriptor,
            base_url,
            client,
            stream_client,
            runtime: Arc::new(runtime),
        })
    }

    pub fn manifest(&self) -> &ProviderManifestV1 {
        &self.manifest
    }

    /// Resolves an operation's timeout, treating the operator's runtime setting as a **ceiling**
    /// rather than a default. A provider manifest is operator-supplied but not necessarily
    /// operator-audited, so it may ask for a shorter deadline and never a longer one.
    fn timeout_for(&self, spec: &RequestSpec) -> Duration {
        Duration::from_millis(clamp_timeout(
            spec.timeout_ms,
            self.runtime.default_timeout_ms,
        ))
    }

    /// Resolves an operation's response cap, again bounded by the operator's setting so a manifest
    /// cannot raise its own memory budget.
    fn response_limit_for(&self, spec: &RequestSpec) -> usize {
        spec.max_response_bytes
            .unwrap_or(self.runtime.max_response_bytes)
            .min(self.runtime.max_response_bytes)
    }

    async fn prepare_request<T: Serialize>(
        &self,
        spec: &RequestSpec,
        request: &T,
        session: &Value,
        mode: RequestMode,
    ) -> Result<RequestBuilder, ProviderError> {
        let context = MappingContext::for_request(request, &self.runtime.configuration, session)?;
        let path = render_template(&spec.path, &context)?;
        let mut url = self
            .base_url
            .join(path.trim_start_matches('/'))
            .map_err(|error| {
                ProviderError::UrlNotAllowed(format!("invalid operation path: {error}"))
            })?;
        ensure_within_base(&self.base_url, &url)?;

        let mut mapped_query = Vec::new();
        for (name, mapping) in &spec.query {
            let value = evaluate_mapping(mapping, &context, &self.manifest.mappings)?;
            append_query_value(&mut mapped_query, name, &value)?;
        }
        if !mapped_query.is_empty() {
            let mut query = url.query_pairs_mut();
            for (name, value) in mapped_query {
                query.append_pair(&name, &value);
            }
        }
        self.validate_destination(&url).await?;

        let method = match spec.method {
            HttpMethod::Get => Method::GET,
            HttpMethod::Post => Method::POST,
            HttpMethod::Put => Method::PUT,
            HttpMethod::Patch => Method::PATCH,
        };
        let mut builder = match mode {
            RequestMode::Buffered => self
                .client
                .request(method, url)
                .timeout(self.timeout_for(spec)),
            RequestMode::Streaming => self.stream_client.request(method, url),
        };

        for (name, value) in &spec.headers {
            validate_header_name(name)?;
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| ProviderError::HeaderNotAllowed(name.clone()))?;
            let value = self.resolve_header(value, &context)?;
            let value = HeaderValue::from_str(&value).map_err(|_| {
                ProviderError::HeaderNotAllowed(
                    "provider header contains invalid bytes".to_string(),
                )
            })?;
            builder = builder.header(name, value);
        }

        let body = spec
            .body
            .as_ref()
            .map(|mapping| evaluate_mapping(mapping, &context, &self.manifest.mappings))
            .transpose()?;
        builder = apply_body(builder, spec.body_encoding, body)?;
        Ok(builder)
    }

    fn resolve_header(
        &self,
        spec: &HeaderValueSpec,
        context: &MappingContext,
    ) -> Result<String, ProviderError> {
        match spec {
            HeaderValueSpec::Text(value) => Ok(value.clone()),
            HeaderValueSpec::Literal(spec) => Ok(spec.literal.clone()),
            HeaderValueSpec::Secret(spec) => {
                let value = self
                    .runtime
                    .credentials
                    .get(&spec.secret)
                    .ok_or_else(|| ProviderError::MissingCredential(spec.secret.clone()))?;
                Ok(format!("{}{value}{}", spec.prefix, spec.suffix))
            }
            HeaderValueSpec::Mapping(spec) => {
                let value = evaluate_mapping(&spec.mapping, context, &self.manifest.mappings)?;
                scalar_to_string(&value, "header mapping")
            }
        }
    }

    async fn validate_destination(&self, url: &Url) -> Result<(), ProviderError> {
        validate_provider_url(
            url,
            self.runtime.allow_insecure_http,
            self.runtime.allow_private_network,
        )?;
        let host = url
            .host_str()
            .ok_or_else(|| ProviderError::UrlNotAllowed("provider URL has no host".to_string()))?;
        if host.parse::<IpAddr>().is_ok() {
            return Ok(());
        }
        let port = url.port_or_known_default().ok_or_else(|| {
            ProviderError::UrlNotAllowed("provider URL has no destination port".to_string())
        })?;
        let addresses = lookup_host((host, port)).await.map_err(|error| {
            ProviderError::Transport(format!("could not resolve provider host: {error}"))
        })?;
        let mut resolved = false;
        for address in addresses {
            resolved = true;
            validate_destination_ip(address.ip(), self.runtime.allow_private_network)?;
        }
        if !resolved {
            return Err(ProviderError::Transport(
                "provider host resolved to no addresses".to_string(),
            ));
        }
        Ok(())
    }

    async fn send_buffered<T: Serialize>(
        &self,
        spec: &RequestSpec,
        request: &T,
        session: &Value,
    ) -> Result<(HeaderMap, Vec<u8>), ProviderError> {
        let response = self
            .prepare_request(spec, request, session, RequestMode::Buffered)
            .await?
            .send()
            .await?;
        let response = ensure_success(response).await?;
        let headers = response.headers().clone();
        let bytes = read_limited(response, self.response_limit_for(spec)).await?;
        Ok((headers, bytes))
    }

    async fn stream_chat_response(
        &self,
        operation: ChatOperation,
        request: ChatRequest,
        session: Value,
        observed: Arc<Mutex<ObservedToolCalls>>,
    ) -> Result<ProviderEventStream, ProviderError> {
        let response = self
            .prepare_request(
                &operation.request,
                &request,
                &session,
                RequestMode::Streaming,
            )
            .await?
            .send()
            .await?;
        let response = ensure_success(response).await?;
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if !content_type
            .to_ascii_lowercase()
            .starts_with("text/event-stream")
        {
            return Err(ProviderError::ResponseMapping(format!(
                "chat response must be text/event-stream, got {content_type}"
            )));
        }

        let response_limit = self.response_limit_for(&operation.request);
        let mut mapper = SseEventMapper::new(operation.response.clone());
        let captures = operation.continuation.captures.clone();
        let stream = async_stream::try_stream! {
            let mut body = response.bytes_stream();
            let mut decoder = SseDecoder::new(MAX_SSE_EVENT_BYTES.min(response_limit));
            let mut total = 0usize;
            let mut drained = false;
            while !drained {
                let events = match body.next().await {
                    Some(chunk) => {
                        let chunk = chunk.map_err(ProviderError::from)?;
                        total = total.saturating_add(chunk.len());
                        if total > response_limit {
                            Err(ProviderError::ResponseTooLarge)?;
                        }
                        decoder.push(&chunk)?
                    }
                    None => {
                        drained = true;
                        decoder.finish()?
                    }
                };
                for event in events {
                    // Trailing events are still decoded after completion: the mapper decides which
                    // kinds remain meaningful (usage arrives last on OpenAI-compatible providers)
                    // and drops the rest.
                    if operation.response.done_data.as_deref() != Some(event.data.trim()) {
                        let data = mapper.decode_data(&event)?;
                        observed.lock().await.captures.extend(capture_values(&captures, &data));
                    }
                    for mapped in mapper.map(&event)? {
                        observed.lock().await.record(&mapped)?;
                        yield mapped;
                    }
                }
            }
            if !mapper.is_completed() {
                Err(ProviderError::StreamEnded)?;
            }
        };
        Ok(Box::pin(stream))
    }
}

impl ProviderPlugin for DeclarativeProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn chat(&self) -> Option<&dyn ChatProvider> {
        self.manifest
            .operations
            .chat_stream
            .as_ref()
            .map(|_| self as _)
    }

    fn embeddings(&self) -> Option<&dyn EmbeddingProvider> {
        self.manifest
            .operations
            .embeddings
            .as_ref()
            .map(|_| self as _)
    }

    fn images(&self) -> Option<&dyn ImageProvider> {
        (self.manifest.operations.image_generation.is_some()
            || self.manifest.operations.image_edit.is_some())
        .then_some(self as _)
    }

    fn models(&self) -> Option<&dyn ModelProvider> {
        if self.manifest.operations.list_models.is_some() || !self.manifest.models.is_empty() {
            Some(self)
        } else {
            None
        }
    }
}

#[async_trait]
impl ChatProvider for DeclarativeProvider {
    async fn start(&self, request: ChatRequest) -> Result<Box<dyn ChatSession>, ProviderError> {
        if self.manifest.operations.chat_stream.is_none() {
            return Err(ProviderError::UnsupportedCapability("chat"));
        }
        Ok(Box::new(DeclarativeChatSession {
            provider: self.clone(),
            request,
            tool_round: 0,
            observed: Arc::new(Mutex::new(ObservedToolCalls::default())),
        }))
    }
}

struct DeclarativeChatSession {
    provider: DeclarativeProvider,
    request: ChatRequest,
    tool_round: u8,
    observed: Arc<Mutex<ObservedToolCalls>>,
}

#[async_trait]
impl ChatSession for DeclarativeChatSession {
    async fn stream(&mut self) -> Result<ProviderEventStream, ProviderError> {
        self.send_stream().await
    }

    async fn continue_with_tools(
        &mut self,
        results: Vec<ToolResult>,
    ) -> Result<ProviderEventStream, ProviderError> {
        let operation = self
            .provider
            .manifest
            .operations
            .chat_stream
            .as_ref()
            .ok_or(ProviderError::UnsupportedCapability("chat"))?;
        if self.tool_round >= operation.continuation.max_tool_rounds {
            return Err(ProviderError::Configuration(format!(
                "provider exceeded {} tool continuation rounds",
                operation.continuation.max_tool_rounds
            )));
        }
        let (calls, text) = {
            let mut observed = self.observed.lock().await;
            (observed.take_calls()?, observed.take_text())
        };
        if calls.is_empty() && !results.is_empty() {
            return Err(ProviderError::ResponseMapping(
                "tool results were supplied before the provider emitted tool calls".to_string(),
            ));
        }
        if !calls.is_empty() {
            // The assistant's own words belong in the replayed turn: providers that validate
            // history (Anthropic in particular) reject a turn whose content was silently dropped,
            // and models that narrate before calling a tool otherwise lose that context.
            self.request.messages.push(ChatMessage {
                role: ChatRole::Assistant,
                content: if text.is_empty() {
                    Vec::new()
                } else {
                    vec![ContentPart::Text { text }]
                },
                tool_calls: calls,
                tool_result: None,
            });
        }
        for result in results {
            self.request.messages.push(ChatMessage {
                role: ChatRole::Tool,
                content: Vec::new(),
                tool_calls: Vec::new(),
                tool_result: Some(result),
            });
        }
        self.tool_round += 1;
        self.send_stream().await
    }
}

impl DeclarativeChatSession {
    async fn send_stream(&self) -> Result<ProviderEventStream, ProviderError> {
        let operation = self
            .provider
            .manifest
            .operations
            .chat_stream
            .clone()
            .ok_or(ProviderError::UnsupportedCapability("chat"))?;
        let observed = self.observed.lock().await;
        let session = json!({
            "toolRound": self.tool_round,
            "captures": observed.captures.clone(),
        });
        drop(observed);
        self.provider
            .stream_chat_response(
                operation,
                self.request.clone(),
                session,
                self.observed.clone(),
            )
            .await
    }
}

#[derive(Default)]
struct ObservedToolCalls {
    calls: BTreeMap<ToolCallId, PartialToolCall>,
    captures: BTreeMap<String, Value>,
    /// Assistant text seen in the current round, replayed with the tool calls it accompanied.
    text: String,
}

#[derive(Default)]
struct PartialToolCall {
    name: String,
    index: u32,
    arguments: String,
    completed: bool,
}

impl ObservedToolCalls {
    fn record(&mut self, event: &ProviderEvent) -> Result<(), ProviderError> {
        match event {
            ProviderEvent::TextDelta { text } => self.text.push_str(text),
            ProviderEvent::ToolCallStart { id, name, index } => {
                if self.calls.contains_key(id) {
                    return Err(ProviderError::ResponseMapping(format!(
                        "provider emitted duplicate tool call id {id}"
                    )));
                }
                self.calls.insert(
                    id.clone(),
                    PartialToolCall {
                        name: name.clone(),
                        index: *index,
                        arguments: String::new(),
                        completed: false,
                    },
                );
            }
            ProviderEvent::ToolArgumentsDelta { id, fragment } => {
                let call = self.calls.get_mut(id).ok_or_else(|| {
                    ProviderError::ResponseMapping(format!(
                        "provider emitted arguments before tool call {id} started"
                    ))
                })?;
                if call.completed {
                    return Err(ProviderError::ResponseMapping(format!(
                        "provider emitted arguments after tool call {id} completed"
                    )));
                }
                call.arguments.push_str(fragment);
            }
            ProviderEvent::ToolCallEnd { id } => {
                let call = self.calls.get_mut(id).ok_or_else(|| {
                    ProviderError::ResponseMapping(format!(
                        "provider completed unknown tool call {id}"
                    ))
                })?;
                if call.completed {
                    return Err(ProviderError::ResponseMapping(format!(
                        "provider completed tool call {id} more than once"
                    )));
                }
                call.completed = true;
            }
            _ => {}
        }
        Ok(())
    }

    fn take_text(&mut self) -> String {
        std::mem::take(&mut self.text)
    }

    fn take_calls(&mut self) -> Result<Vec<ToolCall>, ProviderError> {
        let mut calls = std::mem::take(&mut self.calls)
            .into_iter()
            .map(|(id, call)| {
                if !call.completed {
                    return Err(ProviderError::ResponseMapping(format!(
                        "tool call {id} did not complete"
                    )));
                }
                let arguments = if call.arguments.trim().is_empty() {
                    Value::Object(Default::default())
                } else {
                    serde_json::from_str(&call.arguments).map_err(|error| {
                        ProviderError::ResponseMapping(format!(
                            "tool call {id} arguments are not valid JSON: {error}"
                        ))
                    })?
                };
                Ok(ToolCall {
                    id,
                    name: call.name,
                    arguments,
                    index: Some(call.index),
                })
            })
            .collect::<Result<Vec<_>, ProviderError>>()?;
        calls.sort_by_key(|call| call.index.unwrap_or_default());
        Ok(calls)
    }
}

#[async_trait]
impl EmbeddingProvider for DeclarativeProvider {
    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResult, ProviderError> {
        let operation = self
            .manifest
            .operations
            .embeddings
            .as_ref()
            .ok_or(ProviderError::UnsupportedCapability("embeddings"))?;
        if request.inputs.is_empty() {
            return Err(ProviderError::Configuration(
                "embedding request must contain at least one input".to_string(),
            ));
        }
        if request.inputs.len() > operation.max_batch_size {
            return Err(ProviderError::Configuration(format!(
                "embedding batch contains {} inputs; provider limit is {}",
                request.inputs.len(),
                operation.max_batch_size
            )));
        }
        let (_, bytes) = self
            .send_buffered(&operation.request, &request, &Value::Null)
            .await?;
        decode_embeddings(operation, &request, &bytes)
    }
}

#[async_trait]
impl ImageProvider for DeclarativeProvider {
    async fn generate(&self, request: ImageRequest) -> Result<ImageResult, ProviderError> {
        let operation = if request.input_images.is_empty() {
            self.manifest.operations.image_generation.as_ref().ok_or(
                ProviderError::UnsupportedCapability("text_to_image_generation"),
            )?
        } else {
            self.manifest
                .operations
                .image_edit
                .as_ref()
                .or(self.manifest.operations.image_generation.as_ref())
                .ok_or(ProviderError::UnsupportedCapability("image_edit"))?
        };
        if request.count == 0 {
            return Err(ProviderError::Configuration(
                "image request must ask for at least one image".to_string(),
            ));
        }
        let (headers, bytes) = self
            .send_buffered(&operation.request, &request, &Value::Null)
            .await?;
        match operation.response.body_encoding {
            ImageBodyEncoding::Binary => {
                let media_type = headers
                    .get(CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.split(';').next())
                    .unwrap_or("application/octet-stream")
                    .to_string();
                validate_image_media_type(&media_type)?;
                if request.count != 1 {
                    return Err(ProviderError::ResponseMapping(format!(
                        "binary image operation returned one image for requested count {}",
                        request.count
                    )));
                }
                if bytes.is_empty() {
                    return Err(ProviderError::ResponseMapping(
                        "provider returned an empty image".to_string(),
                    ));
                }
                Ok(ImageResult {
                    images: vec![GeneratedImage { bytes, media_type }],
                    usage: None,
                })
            }
            ImageBodyEncoding::Json => {
                self.decode_json_images(operation, request.count, &bytes)
                    .await
            }
        }
    }
}

impl DeclarativeProvider {
    async fn decode_json_images(
        &self,
        operation: &ImageOperation,
        expected_count: u8,
        bytes: &[u8],
    ) -> Result<ImageResult, ProviderError> {
        let root: Value = serde_json::from_slice(bytes)?;
        let items = select_array(
            &root,
            operation.response.images_pointer.as_deref(),
            "images",
        )?;
        let mut images = Vec::with_capacity(items.len());
        for item in items {
            let media_type = operation
                .response
                .media_type_pointer
                .as_deref()
                .and_then(|pointer| select(item, pointer))
                .and_then(Value::as_str)
                .or(operation.response.default_media_type.as_deref())
                .unwrap_or("image/png")
                .to_string();
            validate_image_media_type(&media_type)?;
            if let Some(pointer) = operation.response.base64_pointer.as_deref()
                && let Some(encoded) = select(item, pointer).and_then(Value::as_str)
            {
                let bytes = STANDARD.decode(encoded).map_err(|error| {
                    ProviderError::ResponseMapping(format!(
                        "provider image is not valid base64: {error}"
                    ))
                })?;
                images.push(GeneratedImage { bytes, media_type });
                continue;
            }
            if let Some(pointer) = operation.response.url_pointer.as_deref()
                && let Some(value) = select(item, pointer).and_then(Value::as_str)
            {
                let url = Url::parse(value).map_err(|error| {
                    ProviderError::ResponseMapping(format!("invalid image URL: {error}"))
                })?;
                if url.scheme() != "https" {
                    return Err(ProviderError::UrlNotAllowed(
                        "generated image downloads require HTTPS".to_string(),
                    ));
                }
                self.validate_destination(&url).await?;
                let request = self
                    .client
                    .get(url)
                    .timeout(self.timeout_for(&operation.request));
                let response = ensure_success(request.send().await?).await?;
                let headers = response.headers().clone();
                let bytes =
                    read_limited(response, self.response_limit_for(&operation.request)).await?;
                let media_type = headers
                    .get(CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.split(';').next())
                    .unwrap_or(&media_type)
                    .to_string();
                validate_image_media_type(&media_type)?;
                images.push(GeneratedImage { bytes, media_type });
                continue;
            }
            // Some providers return text and image parts in the same array (Gemini's
            // generateContent response is one example). Non-image parts are not malformed image
            // data, so ignore them and let the final count check catch missing output images.
        }
        if images.is_empty() {
            return Err(ProviderError::ResponseMapping(
                "provider returned no images".to_string(),
            ));
        }
        if images.len() != usize::from(expected_count) {
            return Err(ProviderError::ResponseMapping(format!(
                "provider returned {} images for requested count {expected_count}",
                images.len()
            )));
        }
        Ok(ImageResult {
            images,
            usage: operation
                .response
                .usage
                .as_ref()
                .map(|usage| map_usage(&root, usage))
                .transpose()?,
        })
    }
}

#[async_trait]
impl ModelProvider for DeclarativeProvider {
    fn static_models(&self) -> Vec<ProviderModel> {
        self.manifest
            .models
            .iter()
            .map(provider_model_from_manifest)
            .collect()
    }

    async fn list_models(&self) -> Result<Vec<ProviderModel>, ProviderError> {
        if let Some(operation) = self.manifest.operations.list_models.as_ref() {
            let (_, bytes) = self
                .send_buffered(&operation.request, &Value::Null, &Value::Null)
                .await?;
            decode_models(
                &self.descriptor.capabilities,
                &self.manifest.models,
                &self.manifest.mappings,
                operation,
                &bytes,
            )
        } else {
            Ok(self.static_models())
        }
    }
}

fn validate_credentials(
    manifest: &ProviderManifestV1,
    runtime: &ProviderRuntimeConfig,
) -> Result<(), ProviderError> {
    for credential in &manifest.credentials {
        if credential.required
            && runtime
                .credentials
                .get(&credential.id)
                .is_none_or(|value| value.is_empty())
        {
            return Err(ProviderError::MissingCredential(credential.id.clone()));
        }
        if credential.credential_type == CredentialType::Secret
            && runtime.configuration.get(&credential.id).is_some()
        {
            return Err(ProviderError::Configuration(format!(
                "secret credential {} must not be stored in provider configuration",
                credential.id
            )));
        }
    }
    // Credentials only ever leave the process inside a header, and a value carrying a line break
    // would be a header-injection vector. Reject it here, naming the slot, so an operator who
    // pasted a wrapped key gets a diagnosable error instead of a per-request header failure.
    for (slot, value) in &runtime.credentials {
        if HeaderValue::from_str(value).is_err() {
            return Err(ProviderError::Configuration(format!(
                "credential {slot} contains characters that cannot be sent in an HTTP header, such as a line break"
            )));
        }
    }
    Ok(())
}

fn validate_configuration(
    manifest: &ProviderManifestV1,
    configuration: &Value,
) -> Result<(), ProviderError> {
    let encoded_size = serde_json::to_vec(configuration)
        .map_err(|error| ProviderError::Configuration(error.to_string()))?
        .len();
    if encoded_size > MAX_CONFIGURATION_BYTES {
        return Err(ProviderError::Configuration(format!(
            "provider configuration exceeds the {MAX_CONFIGURATION_BYTES}-byte limit"
        )));
    }
    let Some(schema) = manifest.configuration_schema.as_ref() else {
        return Ok(());
    };
    let validator = jsonschema::validator_for(schema).map_err(|error| {
        ProviderError::InvalidManifest(format!("configurationSchema is invalid: {error}"))
    })?;
    validator.validate(configuration).map_err(|error| {
        ProviderError::Configuration(format!("configuration does not match schema: {error}"))
    })
}

fn manifest_capabilities(capabilities: &ManifestCapabilities) -> ProviderCapabilities {
    ProviderCapabilities {
        chat: capabilities
            .chat
            .as_ref()
            .map(|chat| crate::ChatCapabilities {
                streaming: chat.streaming,
                tools: chat.tools,
                vision: chat.vision,
                reasoning: chat.reasoning,
            }),
        embeddings: capabilities.embeddings,
        image_generation: capabilities.image_generation,
        model_listing: capabilities.model_listing,
    }
}

/// Confirms a joined operation URL is still inside the configured base URL.
///
/// The path prefix is checked on the *normalised* URL rather than on the manifest template, because
/// the URL standard resolves percent-encoded dot segments (`%2e%2e`) as well as literal ones, and a
/// templated value such as a caller-supplied model id can contain either.
/// Clamps a manifest-declared timeout to the operator's ceiling, never below 1ms.
fn clamp_timeout(declared: Option<u64>, ceiling: u64) -> u64 {
    declared.unwrap_or(ceiling).min(ceiling).max(1)
}

fn ensure_within_base(base: &Url, operation: &Url) -> Result<(), ProviderError> {
    if base.scheme() != operation.scheme()
        || base.host_str() != operation.host_str()
        || base.port_or_known_default() != operation.port_or_known_default()
        || !operation.path().starts_with(base.path())
    {
        return Err(ProviderError::UrlNotAllowed(
            "operation path escaped the provider base URL".to_string(),
        ));
    }
    Ok(())
}

fn render_template(template: &str, context: &MappingContext) -> Result<String, ProviderError> {
    let mut output = String::with_capacity(template.len());
    let mut remainder = template;
    while let Some(start) = remainder.find("${") {
        output.push_str(&remainder[..start]);
        let expression = &remainder[start + 2..];
        let end = expression.find('}').ok_or_else(|| {
            ProviderError::PayloadMapping(format!("unclosed path template in {template}"))
        })?;
        let path = &expression[..end];
        let value = resolve_path(context.root(), path).ok_or_else(|| {
            ProviderError::PayloadMapping(format!("template path does not exist: {path}"))
        })?;
        let value = scalar_to_string(value, "path template")?;
        output.push_str(&utf8_percent_encode(&value, PATH_VALUE).to_string());
        remainder = &expression[end + 1..];
    }
    output.push_str(remainder);
    Ok(output)
}

fn append_query_value(
    query: &mut Vec<(String, String)>,
    name: &str,
    value: &Value,
) -> Result<(), ProviderError> {
    match value {
        Value::Null => {}
        Value::Array(values) => {
            for value in values {
                query.push((name.to_string(), scalar_to_string(value, "query mapping")?));
            }
        }
        value => {
            query.push((name.to_string(), scalar_to_string(value, "query mapping")?));
        }
    }
    Ok(())
}

fn scalar_to_string(value: &Value, label: &str) -> Result<String, ProviderError> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        _ => Err(ProviderError::PayloadMapping(format!(
            "{label} must produce a string, number, or boolean"
        ))),
    }
}

fn apply_body(
    builder: RequestBuilder,
    encoding: BodyEncoding,
    body: Option<Value>,
) -> Result<RequestBuilder, ProviderError> {
    let Some(body) = body else {
        return Ok(builder);
    };
    match encoding {
        BodyEncoding::Json => Ok(builder.json(&body)),
        BodyEncoding::Form => {
            let object = body.as_object().ok_or_else(|| {
                ProviderError::PayloadMapping("form body must be an object".to_string())
            })?;
            let mut form = Vec::with_capacity(object.len());
            for (name, value) in object {
                form.push((name.clone(), scalar_to_string(value, "form field")?));
            }
            Ok(builder.form(&form))
        }
        BodyEncoding::Multipart => Ok(builder.multipart(build_multipart(body)?)),
        BodyEncoding::Text => Ok(builder.body(
            body.as_str()
                .ok_or_else(|| {
                    ProviderError::PayloadMapping("text body must be a string".to_string())
                })?
                .to_string(),
        )),
        BodyEncoding::Binary => {
            let encoded = body.as_str().ok_or_else(|| {
                ProviderError::PayloadMapping("binary body must be a base64 string".to_string())
            })?;
            let bytes = STANDARD.decode(encoded).map_err(|error| {
                ProviderError::PayloadMapping(format!("binary body is not valid base64: {error}"))
            })?;
            Ok(builder.body(bytes))
        }
        BodyEncoding::None => Err(ProviderError::PayloadMapping(
            "bodyEncoding none cannot be used with a mapped body".to_string(),
        )),
    }
}

fn build_multipart(body: Value) -> Result<Form, ProviderError> {
    let object = body.as_object().ok_or_else(|| {
        ProviderError::PayloadMapping("multipart body must be an object".to_string())
    })?;
    let mut form = Form::new();
    for (name, value) in object {
        if let Some(values) = value.as_array() {
            for value in values {
                form = form.part(name.clone(), multipart_part(name, value)?);
            }
        } else {
            form = form.part(name.clone(), multipart_part(name, value)?);
        }
    }
    Ok(form)
}

fn multipart_part(name: &str, value: &Value) -> Result<Part, ProviderError> {
    if let Some(value) = value.as_str() {
        return Ok(Part::text(value.to_string()));
    }
    if value.is_number() || value.is_boolean() {
        return Ok(Part::text(scalar_to_string(value, "multipart field")?));
    }
    let object = value.as_object().ok_or_else(|| {
        ProviderError::PayloadMapping(format!(
            "multipart field {name} must be a string, file object, or array of those values"
        ))
    })?;
    let encoded = object.get("data").and_then(Value::as_str).ok_or_else(|| {
        ProviderError::PayloadMapping(format!("multipart file field {name} requires base64 data"))
    })?;
    let bytes = STANDARD.decode(encoded).map_err(|error| {
        ProviderError::PayloadMapping(format!(
            "multipart field {name} is not valid base64: {error}"
        ))
    })?;
    let mut part = Part::bytes(bytes);
    if let Some(filename) = object.get("filename").and_then(Value::as_str) {
        part = part.file_name(filename.to_string());
    }
    if let Some(media_type) = object.get("mediaType").and_then(Value::as_str) {
        part = part.mime_str(media_type).map_err(|error| {
            ProviderError::PayloadMapping(format!(
                "multipart field {name} has invalid media type: {error}"
            ))
        })?;
    }
    Ok(part)
}

async fn ensure_success(response: Response) -> Result<Response, ProviderError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    // Drain a bounded prefix of the error body so the connection can be reused, but never let a
    // read failure or an oversized body replace the status callers key their backoff off.
    let _ = read_limited(response, MAX_ERROR_BODY_BYTES).await;
    if status == StatusCode::PAYMENT_REQUIRED {
        return Err(ProviderError::PaymentRequired);
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(ProviderError::QuotaExhausted);
    }
    Err(ProviderError::HttpStatus {
        status: status.as_u16(),
        message: status
            .canonical_reason()
            .unwrap_or("provider request failed")
            .to_string(),
    })
}

async fn read_limited(response: Response, limit: usize) -> Result<Vec<u8>, ProviderError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(ProviderError::ResponseTooLarge);
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(ProviderError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn decode_embeddings(
    operation: &EmbeddingOperation,
    request: &EmbeddingRequest,
    bytes: &[u8],
) -> Result<EmbeddingResult, ProviderError> {
    let root = decode_structured(operation.response.body_encoding, bytes)?;
    let items = select(&root, &operation.response.items_pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ProviderError::ResponseMapping("embedding items pointer is not an array".to_string())
        })?;
    let mut indexed = Vec::with_capacity(items.len());
    for (position, item) in items.iter().enumerate() {
        let vector = select(item, &operation.response.vector_pointer)
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ProviderError::ResponseMapping(format!(
                    "embedding item {position} does not contain a vector"
                ))
            })?
            .iter()
            .map(|value| {
                let value = value.as_f64().ok_or_else(|| {
                    ProviderError::ResponseMapping("embedding value is not numeric".to_string())
                })?;
                if !value.is_finite() {
                    return Err(ProviderError::ResponseMapping(
                        "embedding value must be finite".to_string(),
                    ));
                }
                let value = value as f32;
                if !value.is_finite() {
                    return Err(ProviderError::ResponseMapping(
                        "embedding value is outside the supported f32 range".to_string(),
                    ));
                }
                Ok(value)
            })
            .collect::<Result<Vec<_>, ProviderError>>()?;
        if vector.is_empty() {
            return Err(ProviderError::ResponseMapping(
                "embedding vectors must not be empty".to_string(),
            ));
        }
        let index = operation
            .response
            .index_pointer
            .as_deref()
            .and_then(|pointer| select(item, pointer))
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(position);
        indexed.push((index, vector));
    }
    if indexed.len() != request.inputs.len() {
        return Err(ProviderError::ResponseMapping(format!(
            "provider returned {} embedding vectors for {} inputs",
            indexed.len(),
            request.inputs.len()
        )));
    }
    indexed.sort_by_key(|(index, _)| *index);
    for (expected, (actual, _)) in indexed.iter().enumerate() {
        if *actual != expected {
            return Err(ProviderError::ResponseMapping(format!(
                "embedding indices must contain each value from 0 to {}; found {actual} at position {expected}",
                request.inputs.len().saturating_sub(1)
            )));
        }
    }
    let expected_dimensions = request
        .dimensions
        .map(|value| value as usize)
        .or_else(|| indexed.first().map(|(_, vector)| vector.len()))
        .unwrap_or_default();
    if indexed
        .iter()
        .any(|(_, vector)| vector.len() != expected_dimensions)
    {
        return Err(ProviderError::ResponseMapping(format!(
            "embedding vectors must all contain {expected_dimensions} dimensions"
        )));
    }
    Ok(EmbeddingResult {
        vectors: indexed.into_iter().map(|(_, vector)| vector).collect(),
        usage: operation
            .response
            .usage
            .as_ref()
            .map(|usage| map_usage(&root, usage))
            .transpose()?,
    })
}

fn validate_image_media_type(media_type: &str) -> Result<(), ProviderError> {
    if !media_type.to_ascii_lowercase().starts_with("image/") {
        return Err(ProviderError::ResponseMapping(format!(
            "provider returned non-image media type {media_type}"
        )));
    }
    Ok(())
}

fn decode_models(
    provider_capabilities: &ProviderCapabilities,
    catalog: &[ManifestModel],
    mappings: &BTreeMap<String, crate::MappingExpression>,
    operation: &ModelListOperation,
    bytes: &[u8],
) -> Result<Vec<ProviderModel>, ProviderError> {
    let root = decode_structured(operation.response.body_encoding, bytes)?;
    let items = select(&root, &operation.response.models_pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ProviderError::ResponseMapping("model list pointer is not an array".to_string())
        })?;
    let catalog = catalog
        .iter()
        .map(|model| (model.id.as_str(), model))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut models = Vec::with_capacity(items.len());
    for item in items {
        let mut model = if let Some(mapping) = &operation.response.model_mapping {
            let value = evaluate_mapping(
                mapping,
                &MappingContext::new(json!({"item": item})),
                mappings,
            )?;
            let mapped: ManifestModel = serde_json::from_value(value).map_err(|error| {
                ProviderError::ResponseMapping(format!(
                    "modelMapping did not produce a canonical model: {error}"
                ))
            })?;
            provider_model_from_manifest(&mapped)
        } else {
            let id_pointer = operation.response.id_pointer.as_deref().ok_or_else(|| {
                ProviderError::ResponseMapping(
                    "pointer-based model listing has no idPointer".to_string(),
                )
            })?;
            let id = select(item, id_pointer)
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ProviderError::ResponseMapping("model id is not a string".to_string())
                })?;
            let name = operation
                .response
                .name_pointer
                .as_deref()
                .and_then(|pointer| select(item, pointer))
                .and_then(Value::as_str)
                .unwrap_or(id);
            let model_type = operation.response.default_model_type.ok_or_else(|| {
                ProviderError::ResponseMapping(
                    "pointer-based model listing has no defaultModelType".to_string(),
                )
            })?;
            let capabilities = operation
                .response
                .default_capabilities
                .as_ref()
                .ok_or_else(|| {
                    ProviderError::ResponseMapping(
                        "pointer-based model listing has no defaultCapabilities".to_string(),
                    )
                })?;
            ProviderModel {
                id: ModelId::new(id),
                name: name.to_string(),
                model_type,
                capabilities: manifest_capabilities(capabilities),
                metadata: item.clone(),
            }
        };
        if let Some(enrichment) = catalog.get(model.id.as_str()) {
            model.name = enrichment.name.clone();
            model.model_type = enrichment.model_type;
            model.capabilities = manifest_capabilities(&enrichment.capabilities);
            model.metadata = merge_model_metadata(model.metadata, enrichment.metadata.clone());
        }
        if !model_capabilities_are_subset(&model.capabilities, provider_capabilities) {
            return Err(ProviderError::ResponseMapping(format!(
                "model {} declares a capability the provider does not support",
                model.id
            )));
        }
        if !seen.insert(model.id.to_string()) {
            return Err(ProviderError::ResponseMapping(format!(
                "provider returned duplicate model id {}",
                model.id
            )));
        }
        models.push(model);
    }
    Ok(models)
}

fn model_capabilities_are_subset(
    model: &ProviderCapabilities,
    provider: &ProviderCapabilities,
) -> bool {
    let chat_supported = match (&model.chat, &provider.chat) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(model), Some(provider)) => {
            (!model.streaming || provider.streaming)
                && (!model.tools || provider.tools)
                && (!model.vision || provider.vision)
                && (!model.reasoning || provider.reasoning)
        }
    };
    chat_supported
        && (!model.embeddings || provider.embeddings)
        && (!model.image_generation || provider.image_generation)
}

fn provider_model_from_manifest(model: &ManifestModel) -> ProviderModel {
    ProviderModel {
        id: ModelId::new(model.id.clone()),
        name: model.name.clone(),
        model_type: model.model_type,
        capabilities: manifest_capabilities(&model.capabilities),
        metadata: model.metadata.clone(),
    }
}

fn merge_model_metadata(dynamic: Value, catalog: Value) -> Value {
    let Value::Object(catalog) = catalog else {
        return dynamic;
    };
    let mut merged = dynamic.as_object().cloned().unwrap_or_default();
    merged.extend(catalog);
    Value::Object(merged)
}

/// TODO: `StructuredBodyEncoding::TextJson` is accepted by the manifest schema but is decoded
/// exactly like `Json` today, because `serde_json` already ignores the declared content type. Give
/// the variant distinct behaviour or drop it from the schema before manifests start relying on it.
fn decode_structured(
    _encoding: StructuredBodyEncoding,
    bytes: &[u8],
) -> Result<Value, ProviderError> {
    serde_json::from_slice(bytes).map_err(ProviderError::from)
}

fn select<'a>(root: &'a Value, pointer: &str) -> Option<&'a Value> {
    if pointer.is_empty() {
        Some(root)
    } else {
        root.pointer(pointer)
    }
}

fn select_array<'a>(
    root: &'a Value,
    pointer: Option<&str>,
    label: &str,
) -> Result<&'a Vec<Value>, ProviderError> {
    let selected = pointer
        .and_then(|pointer| select(root, pointer))
        .unwrap_or(root);
    selected
        .as_array()
        .ok_or_else(|| ProviderError::ResponseMapping(format!("{label} pointer is not an array")))
}

fn map_usage(root: &Value, mapping: &UsageMapping) -> Result<TokenUsage, ProviderError> {
    crate::sse::normalize_token_usage(
        TokenUsage {
            input_tokens: mapped_u32(root, mapping.input_tokens.as_deref()),
            text_input_tokens: mapped_u32(root, mapping.text_input_tokens.as_deref()),
            image_input_tokens: mapped_u32(root, mapping.image_input_tokens.as_deref()),
            output_tokens: mapped_u32(root, mapping.output_tokens.as_deref()),
            total_tokens: mapped_u32(root, mapping.total_tokens.as_deref()),
            cached_input_tokens: mapped_u32(root, mapping.cached_input_tokens.as_deref()),
            cache_creation_tokens: mapped_u32(root, mapping.cache_creation_tokens.as_deref()),
        },
        mapping.input_tokens_include_cached,
        mapping.input_tokens_include_cache_creation,
    )
}

fn mapped_u32(root: &Value, pointer: Option<&str>) -> Option<u32> {
    pointer
        .and_then(|pointer| select(root, pointer))
        .and_then(crate::sse::value_to_u32)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::{
        EmbeddingOperation, EmbeddingRequest, ModelId, ProviderEvent, ToolCallId,
        mapping::MappingContext,
    };

    use url::Url;

    use super::{
        DeclarativeProvider, ObservedToolCalls, ProviderRuntimeConfig, decode_embeddings,
        ensure_within_base, render_template, validate_image_media_type,
    };

    fn embedding_operation() -> EmbeddingOperation {
        serde_json::from_value(json!({
            "method": "POST",
            "path": "embeddings",
            "bodyEncoding": "json",
            "body": {},
            "response": {
                "itemsPointer": "/data",
                "vectorPointer": "/embedding",
                "indexPointer": "/index"
            }
        }))
        .unwrap()
    }

    fn embedding_request(inputs: usize, dimensions: Option<u32>) -> EmbeddingRequest {
        EmbeddingRequest {
            model: ModelId::new("embed"),
            inputs: (0..inputs).map(|index| index.to_string()).collect(),
            dimensions,
            options: Value::Null,
        }
    }

    #[test]
    fn rejects_missing_duplicate_and_inconsistent_embeddings() {
        let operation = embedding_operation();
        let missing = serde_json::to_vec(&json!({
            "data": [{"index": 0, "embedding": [1.0, 2.0]}]
        }))
        .unwrap();
        assert!(decode_embeddings(&operation, &embedding_request(2, None), &missing).is_err());

        let duplicate = serde_json::to_vec(&json!({
            "data": [
                {"index": 0, "embedding": [1.0, 2.0]},
                {"index": 0, "embedding": [3.0, 4.0]}
            ]
        }))
        .unwrap();
        assert!(decode_embeddings(&operation, &embedding_request(2, None), &duplicate).is_err());

        let dimensions = serde_json::to_vec(&json!({
            "data": [
                {"index": 0, "embedding": [1.0, 2.0]},
                {"index": 1, "embedding": [3.0]}
            ]
        }))
        .unwrap();
        assert!(
            decode_embeddings(&operation, &embedding_request(2, Some(2)), &dimensions).is_err()
        );
    }

    #[test]
    fn rejects_malformed_tool_call_lifecycle() {
        let mut observed = ObservedToolCalls::default();
        let id = ToolCallId::new("call-1");
        assert!(
            observed
                .record(&ProviderEvent::ToolArgumentsDelta {
                    id: id.clone(),
                    fragment: "{}".to_string(),
                })
                .is_err()
        );
        observed
            .record(&ProviderEvent::ToolCallStart {
                id: id.clone(),
                name: "lookup".to_string(),
                index: 0,
            })
            .unwrap();
        assert!(
            observed
                .record(&ProviderEvent::ToolCallStart {
                    id: id.clone(),
                    name: "lookup".to_string(),
                    index: 0,
                })
                .is_err()
        );
        assert!(observed.take_calls().is_err());
    }

    #[test]
    fn path_templates_encode_each_dynamic_value() {
        let context = MappingContext::new(json!({"request": {"model": "team/model one"}}));
        assert_eq!(
            render_template("chat/${request.model}", &context).unwrap(),
            "chat/team%2Fmodel%20one"
        );
    }

    #[test]
    fn path_templates_keep_unreserved_characters_intact() {
        let context = MappingContext::new(json!({"request": {"model": "gpt-4.1-mini_v2~beta"}}));
        // Encoding the unreserved set would send `gpt%2D4%2E1%2Dmini`, which providers 404 on.
        assert_eq!(
            render_template("models/${request.model}:stream", &context).unwrap(),
            "models/gpt-4.1-mini_v2~beta:stream"
        );
    }

    #[test]
    fn rejects_operation_urls_that_climb_out_of_the_base_path() {
        let base = Url::parse("https://api.example.com/v1/").unwrap();
        for path in ["chat", "chat/completions", ""] {
            let joined = base.join(path).unwrap();
            ensure_within_base(&base, &joined).unwrap();
        }
        // The URL standard resolves `%2e%2e` as a parent segment just like `..`, so both spellings
        // have to be caught after normalisation rather than in the manifest template alone.
        for path in [
            "../admin",
            "%2e%2e/admin",
            "%2E%2E/%2e%2e/admin",
            ".%2e/admin",
        ] {
            let joined = base.join(path).unwrap();
            assert!(
                ensure_within_base(&base, &joined).is_err(),
                "{path} joined to {joined} should be rejected"
            );
        }
        assert!(
            ensure_within_base(
                &base,
                &Url::parse("https://evil.example.com/v1/chat").unwrap()
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_embedding_vectors_that_are_not_finite_numbers() {
        let operation = embedding_operation();
        for vector in [json!(["1.0"]), json!([[1.0]]), json!([1e40]), json!([])] {
            let body =
                serde_json::to_vec(&json!({"data": [{"index": 0, "embedding": vector}]})).unwrap();
            assert!(
                decode_embeddings(&operation, &embedding_request(1, None), &body).is_err(),
                "{vector} should not decode"
            );
        }
        // f64 values outside the f32 range collapse to infinity and must not reach the caller.
        let body =
            serde_json::to_vec(&json!({"data": [{"index": 0, "embedding": [1.5, -2.5]}]})).unwrap();
        assert_eq!(
            decode_embeddings(&operation, &embedding_request(1, None), &body)
                .unwrap()
                .vectors,
            vec![vec![1.5_f32, -2.5_f32]]
        );
    }

    #[test]
    fn rejects_embedding_indices_outside_the_requested_range() {
        let operation = embedding_operation();
        let body = serde_json::to_vec(&json!({
            "data": [
                {"index": 0, "embedding": [1.0]},
                {"index": 7, "embedding": [2.0]}
            ]
        }))
        .unwrap();
        assert!(decode_embeddings(&operation, &embedding_request(2, None), &body).is_err());
    }

    #[test]
    fn rejects_embedding_dimensions_that_disagree_with_the_request() {
        let operation = embedding_operation();
        let body =
            serde_json::to_vec(&json!({"data": [{"index": 0, "embedding": [1.0, 2.0]}]})).unwrap();
        assert!(decode_embeddings(&operation, &embedding_request(1, Some(3)), &body).is_err());
        assert!(decode_embeddings(&operation, &embedding_request(1, Some(2)), &body).is_ok());
    }

    #[test]
    fn rejects_credentials_that_cannot_be_sent_as_a_header() {
        let manifest = crate::ProviderManifestV1::from_json(
            &serde_json::to_vec(&json!({
                "manifestVersion": "1.0",
                "id": "keyed",
                "version": "1.0",
                "name": "Keyed",
                "baseUrl": "https://api.example.com/v1/",
                "credentials": [{"id": "api_key", "type": "secret", "required": true}],
                "capabilities": {},
                "operations": {}
            }))
            .unwrap(),
        )
        .unwrap();
        let with_key = |key: &str| {
            DeclarativeProvider::new(
                manifest.clone(),
                ProviderRuntimeConfig {
                    credentials: std::collections::BTreeMap::from([(
                        "api_key".to_string(),
                        key.to_string(),
                    )]),
                    ..Default::default()
                },
            )
        };
        assert!(with_key("sk-proj-abc123").is_ok());
        // A key pasted with line wrapping is a header-injection vector, and the failure has to name
        // the slot instead of surfacing as an opaque per-request header error.
        let error = match with_key("sk-proj-abc\n123") {
            Ok(_) => panic!("a line-wrapped credential must be rejected"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("api_key"), "{error}");
        assert!(!error.contains("sk-proj"), "credential leaked: {error}");
        assert!(with_key("sk-proj-abc\r\nHost: evil").is_err());
    }

    #[test]
    fn only_accepts_image_media_types() {
        assert!(validate_image_media_type("image/png").is_ok());
        assert!(validate_image_media_type("text/html").is_err());
    }

    #[test]
    fn validates_configuration_against_the_declared_schema() {
        let manifest = crate::ProviderManifestV1::from_json(
            &serde_json::to_vec(&json!({
                "manifestVersion": "1.0",
                "id": "configured",
                "version": "1.0",
                "name": "Configured",
                "baseUrl": "https://api.example.com/",
                "capabilities": {},
                "configurationSchema": {
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "type": "object",
                    "properties": {"region": {"type": "string"}},
                    "required": ["region"],
                    "additionalProperties": false
                },
                "operations": {}
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(
            DeclarativeProvider::new(
                manifest.clone(),
                ProviderRuntimeConfig {
                    configuration: json!({"region": "eu-west"}),
                    ..Default::default()
                }
            )
            .is_ok()
        );
        assert!(
            DeclarativeProvider::new(
                manifest,
                ProviderRuntimeConfig {
                    configuration: json!({"region": 4}),
                    ..Default::default()
                }
            )
            .is_err()
        );
    }
}
