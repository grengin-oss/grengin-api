// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

//! A provider plugin composing trusted native serializers with workspace authentication.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use futures_util::{StreamExt, TryStreamExt};
use llm_plugin::*;
use reqwest::Url;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    catalog_download::{MAX_CATALOG_DOCUMENT_BYTES, read_body_capped},
    discovery_catalog::DiscoveryCatalog,
    grengin_grant::GrenginGrant,
};

pub struct GrenginProvider {
    descriptor: ProviderDescriptor,
    upstreams: BTreeSet<String>,
    catalog: DiscoveryCatalog,
    manifests: BTreeMap<String, tokio::sync::OnceCell<ProviderManifestV1>>,
    client: reqwest::Client,
    native: Mutex<BTreeMap<String, Arc<DeclarativeProvider>>>,
}

impl GrenginProvider {
    pub fn new(
        manifest: &ProviderManifestV1,
        catalog: DiscoveryCatalog,
    ) -> Result<Self, ProviderError> {
        let upstreams: BTreeSet<_> = manifest.native_providers.iter().cloned().collect();
        if manifest.id != "grengin"
            || manifest.base_url != "https://service.grengin.com/v1/infer/"
            || upstreams.is_empty()
            || upstreams.len() != manifest.native_providers.len()
            || upstreams.iter().any(|id| !supported_provider(id))
        {
            return Err(ProviderError::InvalidManifest(
                "invalid Grengin nativeProviders".into(),
            ));
        }
        Ok(Self {
            descriptor: manifest.descriptor(),
            manifests: upstreams
                .iter()
                .map(|id| (id.clone(), tokio::sync::OnceCell::new()))
                .collect(),
            upstreams,
            catalog,
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(5))
                .build()?,
            native: Mutex::new(BTreeMap::new()),
        })
    }

    fn resolve(&self, model: &str) -> Result<(&str, String), ProviderError> {
        let (provider, native) = split_model(model)?;
        let declared = self.upstreams.get(provider).ok_or_else(|| {
            ProviderError::Configuration("provider is not declared by the Grengin plugin".into())
        })?;
        Ok((declared, native.to_owned()))
    }

    async fn manifest(&self, provider: &str) -> Result<&ProviderManifestV1, ProviderError> {
        let slot = self
            .manifests
            .get(provider)
            .ok_or_else(|| ProviderError::Configuration("undeclared Grengin provider".into()))?;
        slot.get_or_try_init(|| async {
            // Distribution compatibility, artifact identity and SHA-256 are checked by the
            // catalog. Installed engines and administrator overrides never supply this package.
            let package = self
                .catalog
                .ai_provider(provider, None)
                .await
                .map_err(|_| {
                    ProviderError::Configuration(
                        "Grengin native provider catalog unavailable".into(),
                    )
                })?;
            let manifest = ProviderManifestV1::from_json(&serde_json::to_vec(&package.plugin)?)?;
            prepare_native_manifest(provider, manifest)
        })
        .await
    }

    async fn native(&self, provider: &str) -> Result<Arc<DeclarativeProvider>, ProviderError> {
        let manifest = self.manifest(provider).await?;
        let mut cache = self
            .native
            .lock()
            .map_err(|_| ProviderError::Configuration("provider cache unavailable".into()))?;
        if let Some(native) = cache.get(provider) {
            return Ok(native.clone());
        }
        let grant = GrenginGrant::configured()?;
        let authorization = Arc::new(GatewayAuthorization {
            grant,
            provider: provider.to_owned(),
        });
        let native = Arc::new(compile_native(
            manifest.clone(),
            grant.origin(),
            authorization,
            false,
        )?);
        cache.insert(provider.to_owned(), native.clone());
        Ok(native)
    }
}

impl ProviderPlugin for GrenginProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }
    fn chat(&self) -> Option<&dyn ChatProvider> {
        self.descriptor
            .capabilities
            .chat
            .as_ref()
            .map(|_| self as _)
    }
    fn embeddings(&self) -> Option<&dyn EmbeddingProvider> {
        self.descriptor.capabilities.embeddings.then_some(self)
    }
    fn images(&self) -> Option<&dyn ImageProvider> {
        self.descriptor
            .capabilities
            .image_generation
            .then_some(self)
    }
    fn models(&self) -> Option<&dyn ModelProvider> {
        self.descriptor.capabilities.model_listing.then_some(self)
    }
}

#[async_trait]
impl ChatProvider for GrenginProvider {
    async fn start(&self, mut request: ChatRequest) -> Result<Box<dyn ChatSession>, ProviderError> {
        if self.descriptor.capabilities.chat.is_none() {
            return Err(ProviderError::UnsupportedCapability("chat"));
        }
        let (provider, native) = self.resolve(request.model.as_str())?;
        sanitize_chat(provider, &native, &mut request)?;
        let native = self.native(provider).await?;
        if text_only_mistral(native.manifest())
            && request
                .messages
                .iter()
                .flat_map(|message| &message.content)
                .any(|part| !matches!(part, ContentPart::Text { .. }))
        {
            return Err(ProviderError::UnsupportedCapability(
                "Mistral inline multimodal chat",
            ));
        }
        native.start(request).await
    }
}

#[async_trait]
impl EmbeddingProvider for GrenginProvider {
    async fn embed(&self, mut request: EmbeddingRequest) -> Result<EmbeddingResult, ProviderError> {
        if !self.descriptor.capabilities.embeddings {
            return Err(ProviderError::UnsupportedCapability("embeddings"));
        }
        let (provider, native) = self.resolve(request.model.as_str())?;
        request.model = ModelId::new(native);
        request.options = Value::Null;
        self.native(provider).await?.embed(request).await
    }
}

#[async_trait]
impl ImageProvider for GrenginProvider {
    async fn generate(&self, mut request: ImageRequest) -> Result<ImageResult, ProviderError> {
        if !self.descriptor.capabilities.image_generation {
            return Err(ProviderError::UnsupportedCapability("image generation"));
        }
        let (provider, native) = self.resolve(request.model.as_str())?;
        if provider == "gemini" {
            validate_gemini_model(&native)?;
        }
        request.model = ModelId::new(native);
        request.options = Value::Null;
        for image in &mut request.input_images {
            image.filename = Some(inline_filename(&image.media_type).into());
        }
        self.native(provider).await?.generate(request).await
    }
}

#[async_trait]
impl ModelProvider for GrenginProvider {
    async fn list_models(&self) -> Result<Vec<ProviderModel>, ProviderError> {
        if !self.descriptor.capabilities.model_listing {
            return Err(ProviderError::UnsupportedCapability("model listing"));
        }
        let origin = GrenginGrant::configured()?.origin();
        // The catalog is public: no bearer, upstream credential or browser cookie.
        let response = self
            .client
            .get(format!("{origin}/v1/infer/catalog"))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(ProviderError::HttpStatus {
                status: response.status().as_u16(),
                message: "Grengin catalog unavailable".into(),
            });
        }
        let bytes = read_body_capped(response, MAX_CATALOG_DOCUMENT_BYTES)
            .await
            .map_err(|_| ProviderError::ResponseMapping("Grengin catalog unavailable".into()))?;
        let catalog: Catalog = serde_json::from_slice(&bytes)
            .map_err(|_| ProviderError::ResponseMapping("invalid Grengin catalog".into()))?;
        let ids: BTreeSet<_> = catalog
            .providers
            .iter()
            .filter(|provider| provider.status == "available")
            .map(|provider| canonical_provider(&provider.id).to_owned())
            .filter(|provider| self.upstreams.contains(provider))
            .collect();
        let manifests: BTreeMap<_, _> = futures_util::stream::iter(ids)
            .map(|id| async move { self.manifest(&id).await.map(|manifest| (id, manifest)) })
            .buffer_unordered(4)
            .try_collect()
            .await?;
        catalog_models(catalog, &manifests, &self.descriptor.capabilities)
    }
}

struct GatewayAuthorization {
    grant: &'static GrenginGrant,
    provider: String,
}
#[async_trait]
impl RequestAuthorization for GatewayAuthorization {
    async fn authorize(&self, url: &Url) -> Result<String, ProviderError> {
        validate_gateway_url(self.grant.origin(), &self.provider, url)?;
        self.grant.bearer().await
    }

    async fn reject(&self, bearer: &str) {
        self.grant.reject(bearer).await;
    }
}

pub fn split_model(model: &str) -> Result<(&str, &str), ProviderError> {
    let (provider, native) = model.split_once('/').ok_or_else(|| {
        ProviderError::Configuration("use provider/native_model_id for Grengin".into())
    })?;
    let provider = canonical_provider(provider);
    if !supported_provider(provider)
        || native.is_empty()
        || native.trim() != native
        || native.chars().any(char::is_control)
    {
        return Err(ProviderError::Configuration(
            "unsupported Grengin model selection".into(),
        ));
    }
    Ok((provider, native))
}

fn canonical_provider(provider: &str) -> &str {
    if provider == "google" {
        "gemini"
    } else {
        provider
    }
}

fn supported_provider(provider: &str) -> bool {
    matches!(
        provider,
        "openai"
            | "anthropic"
            | "mistral"
            | "gemini"
            | "qwen"
            | "xai"
            | "deepseek"
            | "kimi"
            | "groq"
            | "cerebras"
            | "glm"
            | "minimax"
            | "sarvam"
            | "sakana"
            | "tinker"
    )
}

fn validate_gemini_model(model: &str) -> Result<(), ProviderError> {
    if !model.is_empty()
        && model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
    {
        Ok(())
    } else {
        Err(ProviderError::Configuration(
            "unsupported Gemini native model ID".into(),
        ))
    }
}

fn validate_gateway_url(origin: &str, provider: &str, url: &Url) -> Result<(), ProviderError> {
    let origin = Url::parse(origin)
        .map_err(|_| ProviderError::UrlNotAllowed("invalid Grengin origin".into()))?;
    let prefix = format!("/v1/infer/{provider}/");
    let operation = url.path().strip_prefix(&prefix).unwrap_or_default();
    let allowed = match provider {
        "openai" => matches!(
            operation,
            "responses" | "chat/completions" | "embeddings" | "images/generations" | "images/edits"
        ),
        "anthropic" => operation == "messages",
        "qwen" | "xai" => matches!(operation, "responses" | "chat/completions"),
        "mistral" => matches!(
            operation,
            "conversations" | "chat/completions" | "embeddings"
        ),
        "gemini" => {
            operation == "openai/embeddings"
                || operation
                    .strip_prefix("models/")
                    .and_then(|value| value.split_once(':'))
                    .is_some_and(|(model, operation)| {
                        validate_gemini_model(model).is_ok()
                            && matches!(operation, "generateContent" | "streamGenerateContent")
                    })
        }
        other if supported_provider(other) => operation == "chat/completions",
        _ => false,
    };
    let query_ok = url.query().is_none()
        || (provider == "gemini"
            && operation.ends_with(":streamGenerateContent")
            && url.query() == Some("alt=sse"));
    if url.origin() != origin.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !allowed
        || !query_ok
        || url.path().contains('%')
    {
        return Err(ProviderError::UrlNotAllowed(
            "unsupported Grengin operation".into(),
        ));
    }
    Ok(())
}

fn prepare_native_manifest(
    provider: &str,
    mut manifest: ProviderManifestV1,
) -> Result<ProviderManifestV1, ProviderError> {
    if manifest.id != provider || !manifest.native_providers.is_empty() {
        return Err(ProviderError::InvalidManifest(
            "invalid Grengin native package".into(),
        ));
    }
    manifest.operations.list_models = None;
    manifest.capabilities.model_listing = !manifest.models.is_empty();
    if text_only_mistral(&manifest) {
        if let Some(chat) = manifest.capabilities.chat.as_mut() {
            chat.vision = false;
        }
        for model in &mut manifest.models {
            if let Some(chat) = model.capabilities.chat.as_mut() {
                chat.vision = false;
            }
        }
    }
    let operations = &mut manifest.operations;
    for request in [
        operations
            .chat_stream
            .as_mut()
            .map(|operation| &mut operation.request),
        operations
            .embeddings
            .as_mut()
            .map(|operation| &mut operation.request),
        operations
            .image_generation
            .as_mut()
            .map(|operation| &mut operation.request),
        operations
            .image_edit
            .as_mut()
            .map(|operation| &mut operation.request),
    ]
    .into_iter()
    .flatten()
    {
        let path = request.path.replace("${request.model}", "model");
        let origin = "https://service.grengin.com";
        let url = Url::parse(&format!(
            "{origin}/v1/infer/{provider}/{}",
            path.trim_start_matches('/')
        ))
        .map_err(|_| ProviderError::InvalidManifest("invalid Grengin native path".into()))?;
        validate_gateway_url(origin, provider, &url)?;
        // A catalog update may introduce new optional fields. Only opt in the inputs
        // used by this operation on the workspace-funded route.
        let Some(MappingExpression::ObjectValue(body)) = request.body.as_mut() else {
            return Err(ProviderError::InvalidManifest(
                "native request body is not an object".into(),
            ));
        };
        let fields: &[&str] = match request.path.as_str() {
            "responses" => &[
                "model",
                "input",
                "max_output_tokens",
                "temperature",
                "tools",
                "tool_choice",
                "stream",
            ],
            "conversations" => &[
                "model",
                "inputs",
                "instructions",
                "completion_args",
                "tools",
                "stream",
            ],
            "messages" => &[
                "model",
                "messages",
                "system",
                "max_tokens",
                "temperature",
                "tools",
                "tool_choice",
                "stream",
            ],
            "chat/completions" => &[
                "model",
                "messages",
                "max_tokens",
                "max_completion_tokens",
                "temperature",
                "tools",
                "tool_choice",
                "stream",
                "stream_options",
            ],
            "embeddings" | "openai/embeddings" => &["model", "input", "dimensions"],
            "images/generations" => &["model", "prompt", "n", "size", "quality"],
            "images/edits" => &["model", "prompt", "n", "size", "quality", "image[]"],
            "models/${request.model}:streamGenerateContent" => &[
                "contents",
                "system_instruction",
                "generationConfig",
                "tools",
                "toolConfig",
            ],
            "models/${request.model}:generateContent" => &["contents", "generationConfig"],
            _ => {
                return Err(ProviderError::InvalidManifest(
                    "unsupported Grengin native request".into(),
                ));
            }
        };
        body.retain(|key, _| {
            fields.contains(&key.as_str())
                || (provider == "groq"
                    && request.path == "chat/completions"
                    && key == "compound_custom")
        });
        if matches!(request.path.as_str(), "responses" | "conversations") {
            body.insert(
                "store".into(),
                serde_json::from_value(json!({"$literal":false}))?,
            );
        }
    }
    // Native continuations replay inline messages. Gemini's signature is the one protocol
    // value needed to replay its tool calls; response/conversation IDs never leave the API.
    if let Some(chat) = manifest.operations.chat_stream.as_mut() {
        chat.continuation
            .captures
            .retain(|name, _| provider == "gemini" && name == "thoughtSignature");
    }
    Ok(manifest)
}

fn text_only_mistral(manifest: &ProviderManifestV1) -> bool {
    if manifest.id != "mistral" {
        return false;
    }
    let Some(mapping) = manifest.mappings.get("mistral_input_entries") else {
        return false;
    };
    let Ok(mapping) = serde_json::to_string(mapping) else {
        return true;
    };
    // Released Conversations templates read only the first text part despite advertising
    // vision. Stop silently dropping attachments; a package mapping full content lifts this.
    mapping.contains("\"/item/content/0/text\"")
        && !mapping.contains("\"item.content\"")
        && !mapping.contains("\"/item/content\"")
}

fn compile_native(
    manifest: ProviderManifestV1,
    origin: &str,
    authorization: Arc<dyn RequestAuthorization>,
    allow_test_http: bool,
) -> Result<DeclarativeProvider, ProviderError> {
    let runtime = ProviderRuntimeConfig {
        base_url_override: Some(format!("{origin}/v1/infer/{}/", manifest.id)),
        default_timeout_ms: 240_000,
        max_response_bytes: 16 * 1024 * 1024,
        // This argument is never true in production; only loopback unit fixtures.
        allow_insecure_http: cfg!(test) && allow_test_http,
        allow_private_network: cfg!(test) && allow_test_http,
        ..Default::default()
    };
    DeclarativeProvider::new_authorized(manifest, runtime, authorization)
}

fn inline_filename(media_type: &str) -> &'static str {
    match media_type {
        "application/pdf" => "attachment.pdf",
        "text/plain" => "attachment.txt",
        "image/png" => "attachment.png",
        "image/jpeg" => "attachment.jpg",
        "image/webp" => "attachment.webp",
        "image/gif" => "attachment.gif",
        _ => "attachment.bin",
    }
}

fn sanitize_chat(
    provider: &str,
    native: &str,
    request: &mut ChatRequest,
) -> Result<(), ProviderError> {
    use base64::Engine;
    if provider == "gemini" {
        validate_gemini_model(native)?;
    }
    for part in request
        .messages
        .iter_mut()
        .flat_map(|message| &mut message.content)
    {
        match part {
            ContentPart::File {
                name, media_type, ..
            } => {
                *name = inline_filename(media_type).into();
            }
            ContentPart::FileReference { .. } => {
                return Err(ProviderError::Configuration(
                    "Grengin requires inline file content".into(),
                ));
            }
            ContentPart::ImageUrl { url, .. } => {
                let valid_inline = url
                    .strip_prefix("data:image/")
                    .and_then(|url| url.split_once(";base64,"))
                    .is_some_and(|(kind, data)| {
                        !kind.is_empty()
                            && kind.bytes().all(|byte| {
                                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'+')
                            })
                            && base64::engine::general_purpose::STANDARD
                                .decode(data)
                                .is_ok()
                    });
                if !valid_inline {
                    return Err(ProviderError::Configuration(
                        "Grengin requires inline image content".into(),
                    ));
                }
            }
            _ => {}
        }
    }
    request.model = ModelId::new(native);
    // Arbitrary options never carry metadata, identifiers or saved provider state.
    request.options = Value::Null;
    Ok(())
}

#[derive(Deserialize)]
struct Catalog {
    providers: Vec<CatalogProvider>,
}
#[derive(Deserialize)]
struct CatalogProvider {
    id: String,
    status: String,
    models: Vec<CatalogModel>,
}
#[derive(Deserialize)]
struct CatalogModel {
    id: String,
    native_model_id: String,
    name: String,
    capability: String,
    status: String,
    input_per_million_microdollars: i64,
    output_per_million_microdollars: i64,
}

fn model_capabilities(
    manifest: &ProviderManifestV1,
    model: Option<&ManifestModel>,
) -> ProviderCapabilities {
    let Some(model) = model else {
        return manifest.descriptor().capabilities;
    };
    let capabilities = &model.capabilities;
    ProviderCapabilities {
        chat: capabilities.chat.as_ref().map(|chat| ChatCapabilities {
            vision: chat.vision,
            reasoning: chat.reasoning,
            streaming: chat.streaming,
            tools: chat.tools,
        }),
        embeddings: capabilities.embeddings,
        image_generation: capabilities.image_generation,
        model_listing: false,
    }
}

fn catalog_models(
    catalog: Catalog,
    manifests: &BTreeMap<String, &ProviderManifestV1>,
    declared: &ProviderCapabilities,
) -> Result<Vec<ProviderModel>, ProviderError> {
    let mut models = BTreeMap::new();
    for provider in catalog.providers {
        let provider_id = canonical_provider(&provider.id);
        let Some(native) = manifests.get(provider_id) else {
            continue;
        };
        if provider.status != "available" {
            continue;
        }
        for model in provider.models {
            if model.status != "available"
                || model.input_per_million_microdollars < 0
                || model.output_per_million_microdollars < 0
            {
                continue;
            }
            let Ok((id_provider, id_native)) = split_model(&model.id) else {
                continue;
            };
            if id_provider != provider_id || id_native != model.native_model_id {
                continue;
            }
            let native_model = native.models.iter().find(|entry| entry.id == id_native);
            let native_capabilities = model_capabilities(native, native_model);
            let mut capabilities = ProviderCapabilities::default();
            let model_type = match model.capability.as_str() {
                "text" if declared.chat.is_some() && native.operations.chat_stream.is_some() => {
                    let (Some(mut chat), Some(advertised)) =
                        (native_capabilities.chat, declared.chat.as_ref())
                    else {
                        continue;
                    };
                    chat.vision &= advertised.vision;
                    chat.reasoning &= advertised.reasoning;
                    chat.streaming &= advertised.streaming;
                    chat.tools &= advertised.tools;
                    capabilities.chat = Some(chat);
                    ProviderModelType::TextGenerator
                }
                "image"
                    if declared.image_generation
                        && native_capabilities.image_generation
                        && native.operations.image_generation.is_some() =>
                {
                    capabilities.image_generation = true;
                    ProviderModelType::ImageGenerator
                }
                "embed"
                    if declared.embeddings
                        && native_capabilities.embeddings
                        && native.operations.embeddings.is_some() =>
                {
                    capabilities.embeddings = true;
                    ProviderModelType::TextEmbedder
                }
                _ => continue,
            };
            if provider_id == "gemini"
                && model_type != ProviderModelType::TextEmbedder
                && validate_gemini_model(id_native).is_err()
            {
                continue;
            }
            let id = format!("{provider_id}/{}", model.native_model_id);
            let input_rate = model.input_per_million_microdollars as f64 / 1_000_000.0;
            let output_rate = model.output_per_million_microdollars as f64 / 1_000_000.0;
            let mut metadata = json!({
                "inputTokenRate": input_rate,
                "outputTokenRate": output_rate,
                "inputPerMillionMicrodollars": model.input_per_million_microdollars,
                "outputPerMillionMicrodollars": model.output_per_million_microdollars,
            });
            if model_type == ProviderModelType::ImageGenerator {
                // The gateway publishes base tariffs only. Do not price image inputs as free
                // or substitute a native provider's tariff for the workspace tariff.
                metadata["imageInputTokenRate"] = json!(input_rate);
                metadata["imageOutputTokenRate"] = json!(output_rate);
            }
            if let Some(native_model) = native_model {
                for field in [
                    "supportsWebSearch",
                    "supportsPdfNative",
                    "supportsAudio",
                    "supportsMultipleImages",
                    "maxImages",
                    "maxInputTokens",
                    "maxOutputTokens",
                    "dimensions",
                ] {
                    if let Some(value) = native_model.metadata.get(field) {
                        metadata[field] = value.clone();
                    }
                }
            }
            models.insert(
                id.clone(),
                ProviderModel {
                    id: ModelId::new(id),
                    name: model.name,
                    model_type,
                    capabilities,
                    metadata,
                },
            );
        }
    }
    Ok(models.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::State,
        http::{HeaderMap, Uri},
        response::IntoResponse,
        routing::post,
    };
    use futures_util::StreamExt;

    struct TestAuthorization {
        origin: String,
        provider: String,
    }
    #[async_trait]
    impl RequestAuthorization for TestAuthorization {
        async fn authorize(&self, url: &Url) -> Result<String, ProviderError> {
            validate_gateway_url(&self.origin, &self.provider, url)?;
            Ok("test-workspace-grant".into())
        }
    }
    fn request() -> ChatRequest {
        ChatRequest {
            model: ModelId::new("openai/Org/Native-Case"),
            messages: vec![ChatMessage {
                role: ChatRole::User,
                content: vec![ContentPart::Text {
                    text: "hello".into(),
                }],
                tool_calls: vec![],
                tool_result: None,
            }],
            temperature: Some(0.2),
            max_tokens: Some(30),
            tools: vec![ToolDefinition {
                name: "lookup".into(),
                description: None,
                parameters: json!({"type":"object","properties":{}}),
            }],
            tool_choice: None,
            web_search: false,
            options: json!({"metadata":{"email":"private@example.invalid"},"previous_response_id":"saved-provider-id","user":"private-user","store":true}),
        }
    }
    fn declared() -> ProviderCapabilities {
        ProviderCapabilities {
            chat: Some(ChatCapabilities {
                streaming: true,
                tools: true,
                vision: true,
                reasoning: true,
            }),
            embeddings: true,
            image_generation: true,
            model_listing: true,
        }
    }

    #[test]
    fn canonical_ids_and_routes_are_narrow_without_changing_native_ids() {
        assert_eq!(
            split_model("qwen/Org/Native-Case").unwrap(),
            ("qwen", "Org/Native-Case")
        );
        assert_eq!(
            split_model("google/Gemini-Case").unwrap(),
            ("gemini", "Gemini-Case")
        );
        for model in [
            "openrouter/model",
            "bedrock/model",
            "azure/model",
            "openai/",
            "OpenAI/model",
        ] {
            assert!(split_model(model).is_err());
        }
        let origin = "https://service.grengin.com";
        for path in [
            "/v1/infer/gemini/models/Model-1:streamGenerateContent?alt=sse",
            "/v1/infer/gemini/models/Model-1:generateContent",
            "/v1/infer/gemini/openai/embeddings",
        ] {
            assert!(
                validate_gateway_url(
                    origin,
                    "gemini",
                    &Url::parse(&format!("{origin}{path}")).unwrap()
                )
                .is_ok()
            );
        }
        for url in [
            "https://evil.invalid/v1/infer/openai/responses",
            "https://service.grengin.com/v1/infer/openai/responses?api_key=secret",
            "https://service.grengin.com/v1/infer/openai/files",
            "https://service.grengin.com/v1/infer/openai/responses%2fother",
            "https://service.grengin.com/v1/infer/anthropic/messages",
        ] {
            assert!(validate_gateway_url(origin, "openai", &Url::parse(url).unwrap()).is_err());
        }
        let mut chat = request();
        sanitize_chat("openai", "Org/Native-Case", &mut chat).unwrap();
        assert_eq!(chat.model.as_str(), "Org/Native-Case");
        assert!(chat.options.is_null());
        chat.messages[0].content = vec![ContentPart::ImageUrl {
            url: "https://example.com/image?signature=private".into(),
            media_type: None,
        }];
        assert!(sanitize_chat("openai", "Model", &mut chat).is_err());
        chat.messages[0].content = vec![ContentPart::ImageUrl {
            url: "data:image/png;base64,aGVsbG8=".into(),
            media_type: None,
        }];
        assert!(sanitize_chat("openai", "Model", &mut chat).is_ok());
        chat.messages[0].content = vec![ContentPart::FileReference {
            id: "stored-file".into(),
            name: "file.pdf".into(),
            media_type: "application/pdf".into(),
        }];
        assert!(sanitize_chat("openai", "Model", &mut chat).is_err());
    }

    #[test]
    fn native_packages_keep_their_protocol_and_capabilities_without_saved_state() {
        for provider in ["qwen", "xai"] {
            let mut package = ProviderManifestV1::from_json(include_bytes!(
                "../../llm-plugin/examples/openai.provider.json"
            ))
            .unwrap();
            package.id = provider.into();
            let chat = package.operations.chat_stream.as_mut().unwrap();
            chat.continuation
                .captures
                .insert("responseId".into(), "/id".into());
            let Some(MappingExpression::ObjectValue(body)) = chat.request.body.as_mut() else {
                panic!()
            };
            body.insert(
                "previous_response_id".into(),
                serde_json::from_value(json!({"$get":"session.responseId"})).unwrap(),
            );
            body.insert(
                "metadata".into(),
                serde_json::from_value(json!({"$literal":{"user":"private"}})).unwrap(),
            );
            body.insert(
                "future_tracking_field".into(),
                serde_json::from_value(json!({"$literal":"private"})).unwrap(),
            );
            let package = prepare_native_manifest(provider, package).unwrap();
            let chat = package.operations.chat_stream.unwrap();
            assert_eq!(chat.request.path, "responses");
            let Some(MappingExpression::ObjectValue(body)) = chat.request.body else {
                panic!()
            };
            assert!(body.contains_key("max_output_tokens") && body.contains_key("input"));
            assert!(!body.contains_key("metadata") && !body.contains_key("previous_response_id"));
            assert!(!body.contains_key("future_tracking_field"));
            assert!(chat.continuation.captures.is_empty());
        }
        for (provider, token_field, tools) in [
            ("deepseek", "max_tokens", true),
            ("kimi", "max_completion_tokens", true),
            ("sarvam", "max_tokens", false),
        ] {
            let mut package = ProviderManifestV1::from_json(include_bytes!(
                "../../llm-plugin/examples/openai-compatible.provider.json"
            ))
            .unwrap();
            package.id = provider.into();
            package.operations.embeddings = None;
            package.operations.image_generation = None;
            package.operations.image_edit = None;
            package.capabilities.embeddings = false;
            package.capabilities.image_generation = false;
            package.capabilities.chat.as_mut().unwrap().tools = tools;
            let chat = package.operations.chat_stream.as_mut().unwrap();
            let Some(MappingExpression::ObjectValue(body)) = chat.request.body.as_mut() else {
                panic!()
            };
            body.remove("max_tokens");
            body.remove("max_completion_tokens");
            body.insert(
                token_field.into(),
                serde_json::from_value(json!({"$get":"request.maxTokens"})).unwrap(),
            );
            if provider == "kimi" {
                body.remove("temperature");
            }
            let before = serde_json::to_value(&chat.request.body).unwrap();
            let package = prepare_native_manifest(provider, package).unwrap();
            assert_eq!(
                serde_json::to_value(&package.operations.chat_stream.unwrap().request.body)
                    .unwrap(),
                before
            );
            assert_eq!(package.capabilities.chat.unwrap().tools, tools);
        }
        let mut mistral = ProviderManifestV1::from_json(include_bytes!(
            "../../llm-plugin/examples/mistral.provider.json"
        ))
        .unwrap();
        assert!(text_only_mistral(&mistral));
        assert!(
            !prepare_native_manifest("mistral", mistral.clone())
                .unwrap()
                .capabilities
                .chat
                .unwrap()
                .vision
        );
        mistral.mappings.insert(
            "mistral_input_entries".into(),
            serde_json::from_value(json!({"$get":"item.content"})).unwrap(),
        );
        assert!(!text_only_mistral(&mistral));
    }

    #[tokio::test]
    async fn native_response_body_contains_only_inputs_and_required_generation_controls() {
        type Observed = Arc<Mutex<Option<(String, HeaderMap, Value)>>>;
        async fn receive(
            State(observed): State<Observed>,
            uri: Uri,
            headers: HeaderMap,
            Json(body): Json<Value>,
        ) -> impl IntoResponse {
            *observed.lock().unwrap() = Some((uri.to_string(), headers, body));
            (
                [("content-type", "text/event-stream")],
                "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
            )
        }
        let observed = Observed::default();
        let app = Router::new()
            .route("/{*path}", post(receive))
            .with_state(observed.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let authorization = Arc::new(TestAuthorization {
            origin: origin.clone(),
            provider: "openai".into(),
        });
        let manifest = prepare_native_manifest(
            "openai",
            ProviderManifestV1::from_json(include_bytes!(
                "../../llm-plugin/examples/openai.provider.json"
            ))
            .unwrap(),
        )
        .unwrap();
        let native = compile_native(manifest, &origin, authorization, true).unwrap();
        let mut chat = request();
        chat.messages[0].content.push(ContentPart::File {
            name: "tenant-private-invoice.pdf".into(),
            data: "aGVsbG8=".into(),
            media_type: "application/pdf".into(),
        });
        sanitize_chat("openai", "Org/Native-Case", &mut chat).unwrap();
        let mut session = native.start(chat).await.unwrap();
        let mut stream = session.stream().await.unwrap();
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        server.abort();
        let (path, headers, body) = observed.lock().unwrap().take().unwrap();
        assert_eq!(path, "/v1/infer/openai/responses");
        assert_eq!(headers["authorization"], "Bearer test-workspace-grant");
        assert_eq!(body["model"], "Org/Native-Case");
        assert_eq!(body["store"], false);
        assert_eq!(body["input"][0]["content"][1]["filename"], "attachment.pdf");
        assert_eq!(
            body["input"][0]["content"][1]["file_data"],
            "data:application/pdf;base64,aGVsbG8="
        );
        assert_eq!(
            body.as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "model",
                "input",
                "temperature",
                "max_output_tokens",
                "tools",
                "stream",
                "store"
            ])
        );
        for private in [
            "tenant-private",
            "private@example.invalid",
            "private-user",
            "saved-provider-id",
        ] {
            assert!(!body.to_string().contains(private));
        }
    }

    #[test]
    fn catalog_exposes_only_declared_and_implemented_native_capabilities() {
        let model = |provider: &str, id: &str, capability: &str| {
            json!({"id":format!("{provider}/{id}"),"native_model_id":id,"name":id,"capability":capability,"status":"available",
            "input_per_million_microdollars":1_250_000,"output_per_million_microdollars":2_000_000,"free":false,"fetched_at":"2026-11-17T00:00:00Z"})
        };
        // Shape emitted by the website's handleInferCatalog (including nullable residency).
        let catalog = json!({"enabled":true,"providers":[
            {"id":"openai","name":"OpenAI","processing_region":null,"status":"available","models":[model("openai","Org/Text-Case","text"),model("openai","embed","embed"),model("openai","image","image")]},
            {"id":"qwen","name":"Qwen","processing_region":null,"status":"available","models":[model("qwen","text","text"),model("qwen","no-image","image"),model("qwen","no-embed","embed")]},
            {"id":"anthropic","name":"Anthropic","processing_region":null,"status":"available","models":[model("anthropic","not-declared","text")]}
        ]});
        let mut openai = ProviderManifestV1::from_json(include_bytes!(
            "../../llm-plugin/examples/openai-compatible.provider.json"
        ))
        .unwrap();
        openai.id = "openai".into();
        openai.models =
            vec![serde_json::from_value(json!({
            "id":"image", "name":"Image", "modelType":"image_generator",
            "capabilities":{"imageGeneration":true},
            "metadata":{"supportsMultipleImages":true,"maxImages":4,"imageInputTokenRate":999.0}
        })).unwrap()];
        let openai = prepare_native_manifest("openai", openai).unwrap();
        let mut qwen = ProviderManifestV1::from_json(include_bytes!(
            "../../llm-plugin/examples/openai.provider.json"
        ))
        .unwrap();
        qwen.id = "qwen".into();
        qwen.models = vec![serde_json::from_value(json!({
            "id":"text", "name":"Text", "modelType":"text_generator",
            "capabilities":{"chat":{"streaming":true,"tools":false,"reasoning":false,"vision":false}},
            "metadata":{"supportsWebSearch":true,"maxOutputTokens":4096,"inputTokenRate":999.0}
        })).unwrap()];
        let qwen = prepare_native_manifest("qwen", qwen).unwrap();
        let manifests = BTreeMap::from([("openai".into(), &openai), ("qwen".into(), &qwen)]);
        let models = catalog_models(
            serde_json::from_value(catalog.clone()).unwrap(),
            &manifests,
            &declared(),
        )
        .unwrap();
        assert_eq!(models.len(), 4);
        let text = models
            .iter()
            .find(|model| model.id.as_str() == "openai/Org/Text-Case")
            .unwrap();
        assert_eq!(text.metadata["inputTokenRate"], 1.25);
        assert_eq!(text.metadata["inputPerMillionMicrodollars"], 1_250_000);
        assert!(!text.capabilities.embeddings);
        assert!(!text.capabilities.image_generation);
        let qwen_chat = models
            .iter()
            .find(|model| model.id.as_str() == "qwen/text")
            .unwrap()
            .capabilities
            .chat
            .as_ref()
            .unwrap();
        assert!(!qwen_chat.tools && !qwen_chat.reasoning && !qwen_chat.vision);
        let info = |id| {
            crate::services::provider_models::to_model_info(
                "grengin",
                models
                    .iter()
                    .find(|model| model.id.as_str() == id)
                    .unwrap()
                    .clone(),
            )
        };
        let qwen_info = info("qwen/text");
        assert!(qwen_info.supports_web_search);
        assert_eq!(qwen_info.max_output_tokens, Some(4096));
        assert_eq!(qwen_info.input_token_rate, Some(1.25));
        let image_info = info("openai/image");
        assert!(image_info.supports_multiple_images);
        assert_eq!(image_info.max_images, Some(4));
        assert_eq!(image_info.image_input_token_rate, Some(1.25));
        assert_eq!(image_info.image_output_token_rate, Some(2.0));
        assert!(
            crate::utils::cost_calc::calculate_cost_decimal(
                1_000,
                0,
                image_info.image_input_token_rate,
                None
            ) > rust_decimal::Decimal::ZERO
        );
        let chat_only = ProviderCapabilities {
            chat: declared().chat,
            ..Default::default()
        };
        assert_eq!(
            catalog_models(
                serde_json::from_value(catalog).unwrap(),
                &manifests,
                &chat_only
            )
            .unwrap()
            .len(),
            2
        );
    }
}
