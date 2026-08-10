// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::{
    ChatCapabilities, MappingExpression, ProviderCapabilities, ProviderDescriptor, ProviderError,
    ProviderId, validate_mapping, validate_mapping_definitions,
};

pub const SUPPORTED_MANIFEST_VERSION: &str = "1.0";
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderManifestV1 {
    #[serde(rename = "$schema", default)]
    pub schema: Option<String>,
    pub manifest_version: String,
    pub id: String,
    pub version: String,
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub credentials: Vec<CredentialDefinition>,
    pub capabilities: ManifestCapabilities,
    #[serde(default)]
    pub configuration_schema: Option<Value>,
    #[serde(default)]
    pub mappings: BTreeMap<String, MappingExpression>,
    #[serde(default)]
    pub models: Vec<ManifestModel>,
    pub operations: ProviderOperations,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialDefinition {
    pub id: String,
    #[serde(rename = "type")]
    pub credential_type: CredentialType,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialType {
    Secret,
    Text,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestCapabilities {
    #[serde(default)]
    pub chat: Option<ManifestChatCapabilities>,
    #[serde(default)]
    pub embeddings: bool,
    #[serde(default)]
    pub image_generation: bool,
    #[serde(default)]
    pub model_listing: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestChatCapabilities {
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub reasoning: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestModel {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub capabilities: ManifestCapabilities,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderOperations {
    #[serde(default)]
    pub chat_stream: Option<ChatOperation>,
    #[serde(default)]
    pub embeddings: Option<EmbeddingOperation>,
    #[serde(default)]
    pub image_generation: Option<ImageOperation>,
    #[serde(default)]
    pub list_models: Option<ModelListOperation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatOperation {
    #[serde(flatten)]
    pub request: RequestSpec,
    pub response: ChatResponseSpec,
    #[serde(default)]
    pub continuation: ContinuationSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmbeddingOperation {
    #[serde(flatten)]
    pub request: RequestSpec,
    #[serde(default = "default_max_embedding_batch_size")]
    pub max_batch_size: usize,
    pub response: EmbeddingResponseSpec,
}

fn default_max_embedding_batch_size() -> usize {
    2048
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageOperation {
    #[serde(flatten)]
    pub request: RequestSpec,
    pub response: ImageResponseSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelListOperation {
    #[serde(flatten)]
    pub request: RequestSpec,
    pub response: ModelListResponseSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestSpec {
    #[serde(default)]
    pub method: HttpMethod,
    pub path: String,
    #[serde(default)]
    pub headers: BTreeMap<String, HeaderValueSpec>,
    #[serde(default)]
    pub query: BTreeMap<String, MappingExpression>,
    #[serde(default)]
    pub body_encoding: BodyEncoding,
    #[serde(default)]
    pub body: Option<MappingExpression>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub max_response_bytes: Option<usize>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    #[default]
    Post,
    Put,
    Patch,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BodyEncoding {
    #[default]
    Json,
    Form,
    Multipart,
    Text,
    Binary,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum HeaderValueSpec {
    Text(String),
    Literal {
        literal: String,
    },
    Secret {
        secret: String,
        #[serde(default)]
        prefix: String,
        #[serde(default)]
        suffix: String,
    },
    Mapping {
        mapping: MappingExpression,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatResponseSpec {
    #[serde(default)]
    pub body_encoding: ChatBodyEncoding,
    #[serde(default)]
    pub event_data_encoding: EventDataEncoding,
    #[serde(default)]
    pub done_data: Option<String>,
    pub rules: Vec<ResponseRule>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatBodyEncoding {
    #[default]
    Sse,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventDataEncoding {
    #[default]
    Json,
    Text,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResponseRule {
    pub id: String,
    #[serde(default)]
    pub for_each: Option<String>,
    #[serde(default)]
    pub when: Option<MatchCondition>,
    pub emit: EventKind,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    #[serde(default)]
    pub constants: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MatchCondition {
    pub pointer: String,
    #[serde(default)]
    pub equals: Option<Value>,
    #[serde(default)]
    pub exists: Option<bool>,
    #[serde(default)]
    pub not_null: Option<bool>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum EventKind {
    MessageStart,
    TextDelta,
    ReasoningDelta,
    ToolCallStart,
    ToolArgumentsDelta,
    ToolCallEnd,
    Usage,
    ProviderEvent,
    Error,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContinuationSpec {
    #[serde(default)]
    pub mode: ContinuationMode,
    #[serde(default = "default_max_tool_rounds")]
    pub max_tool_rounds: u8,
    #[serde(default)]
    pub captures: BTreeMap<String, String>,
}

impl Default for ContinuationSpec {
    fn default() -> Self {
        Self {
            mode: ContinuationMode::default(),
            max_tool_rounds: default_max_tool_rounds(),
            captures: BTreeMap::new(),
        }
    }
}

fn default_max_tool_rounds() -> u8 {
    3
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationMode {
    #[default]
    ReplayMessages,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmbeddingResponseSpec {
    #[serde(default)]
    pub body_encoding: StructuredBodyEncoding,
    pub items_pointer: String,
    pub vector_pointer: String,
    #[serde(default)]
    pub index_pointer: Option<String>,
    #[serde(default)]
    pub usage: Option<UsageMapping>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageResponseSpec {
    #[serde(default)]
    pub body_encoding: ImageBodyEncoding,
    #[serde(default)]
    pub images_pointer: Option<String>,
    #[serde(default)]
    pub base64_pointer: Option<String>,
    #[serde(default)]
    pub url_pointer: Option<String>,
    #[serde(default)]
    pub media_type_pointer: Option<String>,
    #[serde(default)]
    pub default_media_type: Option<String>,
    #[serde(default)]
    pub usage: Option<UsageMapping>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImageBodyEncoding {
    #[default]
    Json,
    Binary,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StructuredBodyEncoding {
    #[default]
    Json,
    TextJson,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelListResponseSpec {
    #[serde(default)]
    pub body_encoding: StructuredBodyEncoding,
    pub models_pointer: String,
    pub id_pointer: String,
    #[serde(default)]
    pub name_pointer: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageMapping {
    #[serde(default)]
    pub input_tokens: Option<String>,
    #[serde(default)]
    pub output_tokens: Option<String>,
    #[serde(default)]
    pub total_tokens: Option<String>,
    #[serde(default)]
    pub cached_input_tokens: Option<String>,
    #[serde(default)]
    pub cache_creation_tokens: Option<String>,
}

impl ProviderManifestV1 {
    pub fn from_json(bytes: &[u8]) -> Result<Self, ProviderError> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(ProviderError::InvalidManifest(format!(
                "provider manifest exceeds the {MAX_MANIFEST_BYTES}-byte limit"
            )));
        }
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|error| ProviderError::InvalidManifest(error.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), ProviderError> {
        if self.manifest_version != SUPPORTED_MANIFEST_VERSION {
            return Err(ProviderError::InvalidManifest(format!(
                "unsupported manifestVersion {}; expected {SUPPORTED_MANIFEST_VERSION}",
                self.manifest_version
            )));
        }
        validate_identifier("provider id", &self.id)?;
        validate_non_empty("provider version", &self.version)?;
        validate_non_empty("provider name", &self.name)?;

        let url = Url::parse(&self.base_url)
            .map_err(|error| ProviderError::InvalidManifest(format!("invalid baseUrl: {error}")))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(ProviderError::InvalidManifest(
                "baseUrl must be an absolute HTTP or HTTPS URL".to_string(),
            ));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(ProviderError::InvalidManifest(
                "baseUrl must not contain a query or fragment".to_string(),
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(ProviderError::InvalidManifest(
                "baseUrl must not contain embedded credentials".to_string(),
            ));
        }

        let mut credential_ids = BTreeSet::new();
        for credential in &self.credentials {
            validate_identifier("credential id", &credential.id)?;
            if !credential_ids.insert(credential.id.as_str()) {
                return Err(ProviderError::InvalidManifest(format!(
                    "duplicate credential id {}",
                    credential.id
                )));
            }
        }

        validate_mapping_definitions(&self.mappings)?;

        validate_capability(
            "chat",
            self.capabilities.chat.is_some(),
            self.operations.chat_stream.is_some(),
        )?;
        validate_capability(
            "embeddings",
            self.capabilities.embeddings,
            self.operations.embeddings.is_some(),
        )?;
        validate_capability(
            "imageGeneration",
            self.capabilities.image_generation,
            self.operations.image_generation.is_some(),
        )?;
        validate_capability(
            "modelListing",
            self.capabilities.model_listing,
            self.operations.list_models.is_some(),
        )?;

        if let Some(chat) = &self.capabilities.chat
            && !chat.streaming
        {
            return Err(ProviderError::InvalidManifest(
                "chat capability must declare streaming=true".to_string(),
            ));
        }

        for request in self.request_specs() {
            validate_request(request, &credential_ids, &self.mappings)?;
        }

        if let Some(chat) = &self.operations.chat_stream {
            if chat.response.rules.is_empty() {
                return Err(ProviderError::InvalidManifest(
                    "chatStream response requires at least one rule".to_string(),
                ));
            }
            let mut rule_ids = BTreeSet::new();
            for rule in &chat.response.rules {
                validate_identifier("response rule id", &rule.id)?;
                if !rule_ids.insert(rule.id.as_str()) {
                    return Err(ProviderError::InvalidManifest(format!(
                        "duplicate response rule id {}",
                        rule.id
                    )));
                }
                validate_rule(rule)?;
            }
            if !(1..=16).contains(&chat.continuation.max_tool_rounds) {
                return Err(ProviderError::InvalidManifest(
                    "continuation maxToolRounds must be between 1 and 16".to_string(),
                ));
            }
        }

        if let Some(embedding) = &self.operations.embeddings {
            if embedding.max_batch_size == 0 {
                return Err(ProviderError::InvalidManifest(
                    "embedding maxBatchSize must be greater than zero".to_string(),
                ));
            }
            validate_pointer(&embedding.response.items_pointer)?;
            validate_pointer(&embedding.response.vector_pointer)?;
            if let Some(pointer) = &embedding.response.index_pointer {
                validate_pointer(pointer)?;
            }
            validate_usage_mapping(embedding.response.usage.as_ref())?;
        }

        if let Some(image) = &self.operations.image_generation {
            match image.response.body_encoding {
                ImageBodyEncoding::Json => {
                    let images_pointer =
                        image.response.images_pointer.as_deref().ok_or_else(|| {
                            ProviderError::InvalidManifest(
                                "JSON image responses require imagesPointer".to_string(),
                            )
                        })?;
                    validate_pointer(images_pointer)?;
                    if image.response.base64_pointer.is_none()
                        && image.response.url_pointer.is_none()
                    {
                        return Err(ProviderError::InvalidManifest(
                            "JSON image responses require base64Pointer or urlPointer".to_string(),
                        ));
                    }
                    for pointer in [
                        image.response.base64_pointer.as_deref(),
                        image.response.url_pointer.as_deref(),
                        image.response.media_type_pointer.as_deref(),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        validate_pointer(pointer)?;
                    }
                }
                ImageBodyEncoding::Binary => {
                    if image.response.images_pointer.is_some()
                        || image.response.base64_pointer.is_some()
                        || image.response.url_pointer.is_some()
                        || image.response.media_type_pointer.is_some()
                    {
                        return Err(ProviderError::InvalidManifest(
                            "binary image responses must not declare JSON pointers".to_string(),
                        ));
                    }
                }
            }
            validate_usage_mapping(image.response.usage.as_ref())?;
        }

        if let Some(models) = &self.operations.list_models {
            validate_pointer(&models.response.models_pointer)?;
            validate_pointer(&models.response.id_pointer)?;
            if let Some(pointer) = &models.response.name_pointer {
                validate_pointer(pointer)?;
            }
        }

        for model in &self.models {
            validate_non_empty("model id", &model.id)?;
            validate_non_empty("model name", &model.name)?;
        }
        Ok(())
    }

    pub fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId::new(self.id.clone()),
            version: self.version.clone(),
            name: self.name.clone(),
            capabilities: ProviderCapabilities {
                chat: self
                    .capabilities
                    .chat
                    .as_ref()
                    .map(|chat| ChatCapabilities {
                        streaming: chat.streaming,
                        tools: chat.tools,
                        vision: chat.vision,
                        reasoning: chat.reasoning,
                    }),
                embeddings: self.capabilities.embeddings,
                image_generation: self.capabilities.image_generation,
                model_listing: self.capabilities.model_listing,
            },
        }
    }

    fn request_specs(&self) -> Vec<&RequestSpec> {
        let mut requests = Vec::new();
        if let Some(operation) = &self.operations.chat_stream {
            requests.push(&operation.request);
        }
        if let Some(operation) = &self.operations.embeddings {
            requests.push(&operation.request);
        }
        if let Some(operation) = &self.operations.image_generation {
            requests.push(&operation.request);
        }
        if let Some(operation) = &self.operations.list_models {
            requests.push(&operation.request);
        }
        requests
    }
}

fn validate_request(
    request: &RequestSpec,
    credentials: &BTreeSet<&str>,
    definitions: &BTreeMap<String, MappingExpression>,
) -> Result<(), ProviderError> {
    if request.path.trim().is_empty() {
        return Err(ProviderError::InvalidManifest(
            "operation path must not be empty".to_string(),
        ));
    }
    if Url::parse(&request.path).is_ok() || request.path.starts_with("//") {
        return Err(ProviderError::InvalidManifest(
            "operation path must be relative to baseUrl".to_string(),
        ));
    }
    if request.path.contains(['\r', '\n', '\\'])
        || request.path.split('/').any(|segment| segment == "..")
    {
        return Err(ProviderError::InvalidManifest(
            "operation path must not contain control characters, backslashes, or '..' segments"
                .to_string(),
        ));
    }
    if matches!(request.method, HttpMethod::Get) && request.body.is_some() {
        return Err(ProviderError::InvalidManifest(
            "GET operations must not declare a request body".to_string(),
        ));
    }
    if request.body.is_none() && request.body_encoding != BodyEncoding::None {
        return Err(ProviderError::InvalidManifest(
            "bodyEncoding must be none when an operation has no body".to_string(),
        ));
    }
    if request.body.is_some() && request.body_encoding == BodyEncoding::None {
        return Err(ProviderError::InvalidManifest(
            "an operation with a body must declare a bodyEncoding".to_string(),
        ));
    }
    if request.timeout_ms == Some(0) {
        return Err(ProviderError::InvalidManifest(
            "operation timeoutMs must be greater than zero".to_string(),
        ));
    }
    if request.max_response_bytes == Some(0) {
        return Err(ProviderError::InvalidManifest(
            "operation maxResponseBytes must be greater than zero".to_string(),
        ));
    }
    for (name, value) in &request.headers {
        crate::security::validate_header_name(name)?;
        if let HeaderValueSpec::Secret { secret, .. } = value
            && !credentials.contains(secret.as_str())
        {
            return Err(ProviderError::InvalidManifest(format!(
                "header {name} references undeclared credential {secret}"
            )));
        }
        if let HeaderValueSpec::Mapping { mapping } = value {
            validate_mapping(mapping, definitions)?;
        }
    }
    for mapping in request.query.values() {
        validate_mapping(mapping, definitions)?;
    }
    if let Some(mapping) = &request.body {
        validate_mapping(mapping, definitions)?;
    }
    Ok(())
}

fn validate_rule(rule: &ResponseRule) -> Result<(), ProviderError> {
    if let Some(pointer) = &rule.for_each {
        validate_pointer(pointer)?;
    }
    if let Some(condition) = &rule.when {
        validate_pointer(&condition.pointer)?;
        if condition.equals.is_none() && condition.exists.is_none() && condition.not_null.is_none()
        {
            return Err(ProviderError::InvalidManifest(format!(
                "response rule {} condition requires equals, exists, or notNull",
                rule.id
            )));
        }
    }
    if let Some(pointer) = &rule.value {
        validate_pointer(pointer)?;
    }
    for pointer in rule.fields.values() {
        validate_pointer(pointer)?;
    }
    Ok(())
}

fn validate_usage_mapping(mapping: Option<&UsageMapping>) -> Result<(), ProviderError> {
    let Some(mapping) = mapping else {
        return Ok(());
    };
    for pointer in [
        mapping.input_tokens.as_deref(),
        mapping.output_tokens.as_deref(),
        mapping.total_tokens.as_deref(),
        mapping.cached_input_tokens.as_deref(),
        mapping.cache_creation_tokens.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        validate_pointer(pointer)?;
    }
    Ok(())
}

fn validate_pointer(pointer: &str) -> Result<(), ProviderError> {
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return Err(ProviderError::InvalidManifest(format!(
            "JSON pointer must be empty or start with '/': {pointer}"
        )));
    }
    Ok(())
}

fn validate_identifier(label: &str, value: &str) -> Result<(), ProviderError> {
    if value.is_empty()
        || value.len() > 64
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
    {
        return Err(ProviderError::InvalidManifest(format!(
            "{label} must contain only lowercase ASCII letters, digits, '-' or '_'"
        )));
    }
    Ok(())
}

fn validate_non_empty(label: &str, value: &str) -> Result<(), ProviderError> {
    if value.trim().is_empty() {
        return Err(ProviderError::InvalidManifest(format!(
            "{label} must not be empty"
        )));
    }
    Ok(())
}

fn validate_capability(name: &str, capability: bool, operation: bool) -> Result<(), ProviderError> {
    if capability != operation {
        return Err(ProviderError::InvalidManifest(format!(
            "capability {name} and its operation must either both be present or both be absent"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::ProviderManifestV1;

    fn chat_manifest() -> Value {
        json!({
            "manifestVersion": "1.0",
            "id": "example",
            "version": "1.2.3",
            "name": "Example",
            "baseUrl": "https://api.example.com/v1/",
            "credentials": [{"id": "api_key", "type": "secret", "required": true}],
            "capabilities": {
                "chat": {"streaming": true, "tools": true}
            },
            "operations": {
                "chatStream": {
                    "method": "POST",
                    "path": "chat/${request.model}",
                    "headers": {
                        "Authorization": {"secret": "api_key", "prefix": "Bearer "}
                    },
                    "bodyEncoding": "json",
                    "body": {"messages": {"$get": "request.messages"}},
                    "response": {
                        "bodyEncoding": "sse",
                        "eventDataEncoding": "json",
                        "doneData": "[DONE]",
                        "rules": [{"id": "text", "emit": "textDelta", "value": "/delta"}]
                    }
                }
            }
        })
    }

    #[test]
    fn accepts_strict_versioned_manifest() {
        ProviderManifestV1::from_json(&serde_json::to_vec(&chat_manifest()).unwrap()).unwrap();
    }

    #[test]
    fn rejects_unknown_fields_and_undeclared_credentials() {
        let mut manifest = chat_manifest();
        manifest["surprise"] = json!(true);
        assert!(ProviderManifestV1::from_json(&serde_json::to_vec(&manifest).unwrap()).is_err());

        let mut manifest = chat_manifest();
        manifest["operations"]["chatStream"]["headers"]["Authorization"]["secret"] =
            json!("missing");
        assert!(ProviderManifestV1::from_json(&serde_json::to_vec(&manifest).unwrap()).is_err());
    }

    #[test]
    fn rejects_path_escape_and_capability_mismatch() {
        let mut manifest = chat_manifest();
        manifest["operations"]["chatStream"]["path"] = json!("../admin");
        assert!(ProviderManifestV1::from_json(&serde_json::to_vec(&manifest).unwrap()).is_err());

        let mut manifest = chat_manifest();
        manifest["capabilities"]["chat"] = Value::Null;
        assert!(ProviderManifestV1::from_json(&serde_json::to_vec(&manifest).unwrap()).is_err());
    }

    #[test]
    fn rejects_oversized_manifests_before_parsing() {
        let oversized = vec![b' '; super::MAX_MANIFEST_BYTES + 1];
        assert!(ProviderManifestV1::from_json(&oversized).is_err());
    }
}
