// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{collections::BTreeMap, fmt, net::IpAddr, sync::Arc, time::Duration};

use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::StreamExt;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
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
    CredentialType, EmbeddingOperation, EmbeddingProvider, EmbeddingRequest, EmbeddingResult,
    GeneratedImage, HeaderValueSpec, HttpMethod, ImageBodyEncoding, ImageOperation, ImageProvider,
    ImageRequest, ImageResult, ManifestCapabilities, MappingContext, ModelId, ModelListOperation,
    ModelProvider, ProviderCapabilities, ProviderDescriptor, ProviderError, ProviderEvent,
    ProviderEventStream, ProviderManifestV1, ProviderModel, ProviderPlugin, RequestSpec,
    SseDecoder, SseEventMapper, StructuredBodyEncoding, TokenUsage, ToolCall, ToolCallId,
    ToolResult, UsageMapping, capture_values, evaluate_mapping, resolve_path,
    security::{validate_destination_ip, validate_header_name, validate_provider_url},
};

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const MAX_ERROR_BODY_BYTES: usize = 8 * 1024;
const MAX_SSE_EVENT_BYTES: usize = 2 * 1024 * 1024;
const MAX_CONFIGURATION_BYTES: usize = 1024 * 1024;

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
    runtime: Arc<ProviderRuntimeConfig>,
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
        let descriptor = manifest.descriptor();

        Ok(Self {
            manifest: Arc::new(manifest),
            descriptor,
            base_url,
            client,
            runtime: Arc::new(runtime),
        })
    }

    pub fn manifest(&self) -> &ProviderManifestV1 {
        &self.manifest
    }

    async fn prepare_request<T: Serialize>(
        &self,
        spec: &RequestSpec,
        request: &T,
        session: &Value,
    ) -> Result<RequestBuilder, ProviderError> {
        let context = MappingContext::for_request(request, &self.runtime.configuration, session)?;
        let path = render_template(&spec.path, &context)?;
        let mut url = self
            .base_url
            .join(path.trim_start_matches('/'))
            .map_err(|error| {
                ProviderError::UrlNotAllowed(format!("invalid operation path: {error}"))
            })?;
        ensure_same_origin(&self.base_url, &url)?;

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
        let timeout_ms = spec
            .timeout_ms
            .unwrap_or(self.runtime.default_timeout_ms)
            .max(1);
        let mut builder = self
            .client
            .request(method, url)
            .timeout(Duration::from_millis(timeout_ms));

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
            HeaderValueSpec::Text(value) | HeaderValueSpec::Literal { literal: value } => {
                Ok(value.clone())
            }
            HeaderValueSpec::Secret {
                secret,
                prefix,
                suffix,
            } => {
                let value = self
                    .runtime
                    .credentials
                    .get(secret)
                    .ok_or_else(|| ProviderError::MissingCredential(secret.clone()))?;
                Ok(format!("{prefix}{value}{suffix}"))
            }
            HeaderValueSpec::Mapping { mapping } => {
                let value = evaluate_mapping(mapping, context, &self.manifest.mappings)?;
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
            .prepare_request(spec, request, session)
            .await?
            .send()
            .await?;
        let response = ensure_success(response).await?;
        let headers = response.headers().clone();
        let limit = spec
            .max_response_bytes
            .unwrap_or(self.runtime.max_response_bytes);
        let bytes = read_limited(response, limit).await?;
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
            .prepare_request(&operation.request, &request, &session)
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

        let response_limit = operation
            .request
            .max_response_bytes
            .unwrap_or(self.runtime.max_response_bytes);
        let mut mapper = SseEventMapper::new(operation.response.clone());
        let captures = operation.continuation.captures.clone();
        let stream = async_stream::try_stream! {
            let mut body = response.bytes_stream();
            let mut decoder = SseDecoder::new(MAX_SSE_EVENT_BYTES.min(response_limit));
            let mut total = 0usize;
            let mut completed = false;
            while let Some(chunk) = body.next().await {
                let chunk = chunk.map_err(ProviderError::from)?;
                total = total.saturating_add(chunk.len());
                if total > response_limit {
                    Err(ProviderError::ResponseTooLarge)?;
                }
                for event in decoder.push(&chunk)? {
                    if operation.response.done_data.as_deref() != Some(event.data.trim()) {
                        let data = mapper.decode_data(&event)?;
                        observed.lock().await.captures.extend(capture_values(&captures, &data));
                    }
                    for mapped in mapper.map(&event)? {
                        observed.lock().await.record(&mapped)?;
                        completed |= matches!(mapped, ProviderEvent::Completed { .. });
                        yield mapped;
                    }
                }
            }
            for event in decoder.finish()? {
                if operation.response.done_data.as_deref() != Some(event.data.trim()) {
                    let data = mapper.decode_data(&event)?;
                    observed.lock().await.captures.extend(capture_values(&captures, &data));
                }
                for mapped in mapper.map(&event)? {
                    observed.lock().await.record(&mapped)?;
                    completed |= matches!(mapped, ProviderEvent::Completed { .. });
                    yield mapped;
                }
            }
            if !completed {
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
        self.manifest
            .operations
            .image_generation
            .as_ref()
            .map(|_| self as _)
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
        let calls = self.observed.lock().await.take_calls()?;
        if calls.is_empty() && !results.is_empty() {
            return Err(ProviderError::ResponseMapping(
                "tool results were supplied before the provider emitted tool calls".to_string(),
            ));
        }
        if !calls.is_empty() {
            self.request.messages.push(ChatMessage {
                role: ChatRole::Assistant,
                content: Vec::new(),
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
        let operation = self
            .manifest
            .operations
            .image_generation
            .as_ref()
            .ok_or(ProviderError::UnsupportedCapability("image_generation"))?;
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
                let response = ensure_success(self.client.get(url).send().await?).await?;
                let headers = response.headers().clone();
                let bytes = read_limited(response, self.runtime.max_response_bytes).await?;
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
            return Err(ProviderError::ResponseMapping(
                "image item did not contain configured base64 or URL data".to_string(),
            ));
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
                .map(|usage| map_usage(&root, usage)),
        })
    }
}

#[async_trait]
impl ModelProvider for DeclarativeProvider {
    async fn list_models(&self) -> Result<Vec<ProviderModel>, ProviderError> {
        if let Some(operation) = self.manifest.operations.list_models.as_ref() {
            let (_, bytes) = self
                .send_buffered(&operation.request, &Value::Null, &Value::Null)
                .await?;
            decode_models(self.descriptor.capabilities.clone(), operation, &bytes)
        } else {
            Ok(self
                .manifest
                .models
                .iter()
                .map(|model| ProviderModel {
                    id: ModelId::new(model.id.clone()),
                    name: model.name.clone(),
                    capabilities: manifest_capabilities(&model.capabilities),
                    metadata: model.metadata.clone(),
                })
                .collect())
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

fn ensure_same_origin(base: &Url, operation: &Url) -> Result<(), ProviderError> {
    if base.scheme() != operation.scheme()
        || base.host_str() != operation.host_str()
        || base.port_or_known_default() != operation.port_or_known_default()
    {
        return Err(ProviderError::UrlNotAllowed(
            "operation path escaped the provider base origin".to_string(),
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
        output.push_str(&utf8_percent_encode(&value, NON_ALPHANUMERIC).to_string());
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
        let part = if let Some(value) = value.as_str() {
            Part::text(value.to_string())
        } else {
            let object = value.as_object().ok_or_else(|| {
                ProviderError::PayloadMapping(format!(
                    "multipart field {name} must be a string or file object"
                ))
            })?;
            let encoded = object.get("data").and_then(Value::as_str).ok_or_else(|| {
                ProviderError::PayloadMapping(format!(
                    "multipart file field {name} requires base64 data"
                ))
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
            part
        };
        form = form.part(name.clone(), part);
    }
    Ok(form)
}

async fn ensure_success(response: Response) -> Result<Response, ProviderError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let _ = read_limited(response, MAX_ERROR_BODY_BYTES).await?;
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
            .map(|usage| map_usage(&root, usage)),
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
    capabilities: crate::ProviderCapabilities,
    operation: &ModelListOperation,
    bytes: &[u8],
) -> Result<Vec<ProviderModel>, ProviderError> {
    let root = decode_structured(operation.response.body_encoding, bytes)?;
    let items = select(&root, &operation.response.models_pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ProviderError::ResponseMapping("model list pointer is not an array".to_string())
        })?;
    items
        .iter()
        .map(|item| {
            let id = select(item, &operation.response.id_pointer)
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
            Ok(ProviderModel {
                id: ModelId::new(id),
                name: name.to_string(),
                capabilities: capabilities.clone(),
                metadata: item.clone(),
            })
        })
        .collect()
}

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

fn map_usage(root: &Value, mapping: &UsageMapping) -> TokenUsage {
    TokenUsage {
        input_tokens: mapped_u32(root, mapping.input_tokens.as_deref()),
        output_tokens: mapped_u32(root, mapping.output_tokens.as_deref()),
        total_tokens: mapped_u32(root, mapping.total_tokens.as_deref()),
        cached_input_tokens: mapped_u32(root, mapping.cached_input_tokens.as_deref()),
        cache_creation_tokens: mapped_u32(root, mapping.cache_creation_tokens.as_deref()),
    }
}

fn mapped_u32(root: &Value, pointer: Option<&str>) -> Option<u32> {
    pointer
        .and_then(|pointer| select(root, pointer))
        .and_then(|value| {
            value
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .or_else(|| value.as_str()?.parse().ok())
        })
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::{
        EmbeddingOperation, EmbeddingRequest, ModelId, ProviderEvent, ToolCallId,
        mapping::MappingContext,
    };

    use super::{
        DeclarativeProvider, ObservedToolCalls, ProviderRuntimeConfig, decode_embeddings,
        render_template, validate_image_media_type,
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
                "version": "1.0.0",
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
