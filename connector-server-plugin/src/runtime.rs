// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{
    collections::{BTreeMap, HashSet},
    time::Duration,
};

use llm_plugin::{
    MappingContext, MappingExpression, evaluate_mapping_bounded, validate_mapping,
    validate_mapping_definitions,
};
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use url::Url;

use crate::{
    ConnectorAuth, ConnectorManifest, ConnectorPluginConfig, ConnectorTool, CredentialDelivery,
    HttpMethod, security,
};

const MAX_MANIFEST_BYTES: usize = 256 * 1024;
const MAX_ARGUMENT_BYTES: usize = 128 * 1024;
const MAX_RESULT_BYTES: usize = 1024 * 1024;
const MAX_URL_BYTES: usize = 16 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ConnectorError {
    #[error("connector plugin manifest invalid: {0}")]
    Manifest(&'static str),
    #[error("connector tool not found")]
    ToolNotFound,
    #[error("connector tool arguments invalid")]
    Arguments,
    #[error("connector user authentication required")]
    Authentication,
    #[error("connector destination not allowed")]
    Destination,
    #[error("connector mapping failed")]
    Mapping,
    #[error("connector request failed")]
    Transport,
    #[error("connector request timed out")]
    Timeout,
    #[error("connector response exceeds limit")]
    ResponseTooLarge,
    #[error("connector response invalid")]
    Response,
    #[error("connector provider rejected the request")]
    ProviderRejected,
    #[error("connector pagination invalid or limit exceeded")]
    Pagination,
    #[error("connector is busy; retry later")]
    Busy,
    #[error("connector provider returned HTTP {status}")]
    Http {
        status: u16,
        retry_after_seconds: Option<u64>,
    },
}

pub struct CallContext<'a> {
    pub user_id: &'a str,
    pub access_token: Option<&'a str>,
}

struct CompiledTool {
    definition: ConnectorTool,
    validator: jsonschema::Validator,
}

pub struct CompiledConnector {
    manifest: ConnectorManifest,
    configuration: Value,
    base_url: Url,
    configuration_validator: Option<jsonschema::Validator>,
    has_required_base_url: bool,
    tools: BTreeMap<String, CompiledTool>,
    client: reqwest::Client,
    calls: Semaphore,
}

impl CompiledConnector {
    pub fn compile(config: ConnectorPluginConfig) -> Result<Self, ConnectorError> {
        if serde_json::to_vec(&config)
            .map_err(|_| ConnectorError::Manifest("serialization"))?
            .len()
            > MAX_MANIFEST_BYTES
        {
            return Err(ConnectorError::Manifest("manifest too large"));
        }
        let manifest: ConnectorManifest = serde_json::from_value(config.manifest)
            .map_err(|_| ConnectorError::Manifest("schema"))?;
        if !config.configuration.is_null() && !config.configuration.is_object() {
            return Err(ConnectorError::Manifest("configuration must be an object"));
        }
        if manifest.schema_version != 1 {
            return Err(ConnectorError::Manifest("unsupported schema_version"));
        }
        if !identifier(&manifest.id, 64)
            || manifest.name.trim().is_empty()
            || manifest.name.len() > 128
            || semver::Version::parse(&manifest.version).is_err()
        {
            return Err(ConnectorError::Manifest("identity or version"));
        }
        let raw_base_url = config
            .base_url_override
            .as_deref()
            .unwrap_or(&manifest.base_url);
        let base_url = Url::parse(raw_base_url).map_err(|_| ConnectorError::Destination)?;
        if raw_base_url.len() > 4096 {
            return Err(ConnectorError::Manifest("base_url too large"));
        }
        security::validate_url(
            &base_url,
            config.allow_insecure_http,
            config.allow_private_network,
        )?;
        let mut has_required_base_url = true;
        if let Some(policy) = &manifest.base_url_policy {
            if policy.allowed_host_suffixes.is_empty()
                || policy.allowed_host_suffixes.len() > 8
                || policy.allowed_host_suffixes.iter().any(|suffix| {
                    !suffix.starts_with('.')
                        || suffix.len() > 253
                        || suffix[1..].split('.').any(|part| {
                            part.is_empty()
                                || !part.bytes().all(|c| {
                                    c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'
                                })
                        })
                })
            {
                return Err(ConnectorError::Manifest("invalid tenant host policy"));
            }
            has_required_base_url = !policy.required_override || config.base_url_override.is_some();
            if config.base_url_override.is_some() {
                let host = base_url.host_str().ok_or(ConnectorError::Destination)?;
                let local = config.allow_private_network
                    && config.allow_insecure_http
                    && (host == "localhost"
                        || host
                            .trim_matches(['[', ']'])
                            .parse::<std::net::IpAddr>()
                            .is_ok_and(|ip| ip.is_loopback()));
                if !local
                    && !policy
                        .allowed_host_suffixes
                        .iter()
                        .any(|suffix| host.ends_with(suffix))
                {
                    return Err(ConnectorError::Destination);
                }
            }
        }
        let configuration_validator = manifest
            .configuration_schema
            .as_ref()
            .map(|schema| {
                validate_schema(schema, 0)?;
                if schema.get("type").and_then(Value::as_str) != Some("object") {
                    return Err(ConnectorError::Manifest(
                        "configuration_schema must describe an object",
                    ));
                }
                jsonschema::validator_for(schema)
                    .map_err(|_| ConnectorError::Manifest("configuration_schema"))
            })
            .transpose()?;
        if manifest.auth == ConnectorAuth::None && manifest.credential_delivery.is_some() {
            return Err(ConnectorError::Manifest(
                "public tools cannot configure credentials",
            ));
        }
        match &manifest.credential_delivery {
            Some(CredentialDelivery::Header { name, prefix }) => {
                if !name.eq_ignore_ascii_case("authorization") {
                    llm_plugin::security::validate_header_name(name)
                        .map_err(|_| ConnectorError::Manifest("credential header not allowed"))?;
                }
                if prefix.as_ref().is_some_and(|value| {
                    value.len() > 32 || !value.is_ascii() || value.chars().any(char::is_control)
                }) {
                    return Err(ConnectorError::Manifest("credential prefix"));
                }
            }
            Some(CredentialDelivery::JsonBody { field }) if !identifier(field, 64) => {
                return Err(ConnectorError::Manifest("credential body field"));
            }
            _ => {}
        }
        if manifest.tools.is_empty() || manifest.tools.len() > 100 {
            return Err(ConnectorError::Manifest("must define 1 to 100 tools"));
        }
        validate_mapping_definitions(&manifest.mappings)
            .map_err(|_| ConnectorError::Manifest("mappings"))?;
        let mut tools = BTreeMap::new();
        for tool in &manifest.tools {
            if !identifier(&tool.name, 64)
                || tool.description.trim().is_empty()
                || tool.description.len() > 4096
            {
                return Err(ConnectorError::Manifest("tool identity"));
            }
            if tool.read_only
                && (tool.destructive
                    || matches!(
                        tool.request.method,
                        HttpMethod::Put | HttpMethod::Patch | HttpMethod::Delete
                    ))
            {
                return Err(ConnectorError::Manifest("contradictory tool annotations"));
            }
            if tool.input_schema.get("type").and_then(Value::as_str) != Some("object") {
                return Err(ConnectorError::Manifest(
                    "input_schema must describe an object",
                ));
            }
            validate_schema(&tool.input_schema, 0)?;
            let validator = jsonschema::validator_for(&tool.input_schema)
                .map_err(|_| ConnectorError::Manifest("input_schema"))?;
            let path = &tool.request.path;
            if path.len() > 4096
                || tool.request.headers.len() > 32
                || tool.request.query.len() > 64
                || tool.required_scopes.len() > 32
                || tool.required_scopes.iter().any(|scope| {
                    scope.trim().is_empty()
                        || scope.len() > 256
                        || scope.chars().any(char::is_control)
                })
            {
                return Err(ConnectorError::Manifest("tool limits"));
            }
            if path.is_empty()
                || path.starts_with("//")
                || path.contains(['?', '#', '%', '\\'])
                || path.chars().any(char::is_control)
            {
                return Err(ConnectorError::Manifest("request path"));
            }
            let mut parameters = HashSet::new();
            for segment in path.trim_start_matches('/').split('/') {
                if segment == "." || segment == ".." {
                    return Err(ConnectorError::Manifest("path traversal"));
                }
                if let Some(key) = segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                    if !identifier(key, 64) || !tool.request.path_parameters.contains_key(key) {
                        return Err(ConnectorError::Manifest("path parameter missing"));
                    }
                    parameters.insert(key.to_string());
                } else if segment.contains(['{', '}']) || segment.contains(':') {
                    return Err(ConnectorError::Manifest("request path segment"));
                }
            }
            if parameters.len() != tool.request.path_parameters.len() {
                return Err(ConnectorError::Manifest("unused path parameters"));
            }
            for header in tool.request.headers.keys() {
                llm_plugin::security::validate_header_name(header)
                    .map_err(|_| ConnectorError::Manifest("header not allowed"))?;
                if header.eq_ignore_ascii_case("authorization")
                    || header.eq_ignore_ascii_case("proxy-authenticate")
                    || header.eq_ignore_ascii_case("set-cookie")
                {
                    return Err(ConnectorError::Manifest("credential header not allowed"));
                }
                if matches!(&manifest.credential_delivery, Some(CredentialDelivery::Header { name, .. }) if name.eq_ignore_ascii_case(header))
                {
                    return Err(ConnectorError::Manifest("credential header collision"));
                }
            }
            for expression in tool
                .request
                .path_parameters
                .values()
                .chain(tool.request.query.values())
                .chain(tool.request.headers.values())
                .chain(tool.request.body.iter())
                .chain(tool.response.result.iter())
                .chain(tool.response.error_if.iter())
            {
                validate_mapping(expression, &manifest.mappings)
                    .map_err(|_| ConnectorError::Manifest("tool mapping"))?;
            }
            if tool.request.method == HttpMethod::Get && tool.request.body.is_some() {
                return Err(ConnectorError::Manifest("GET body not supported"));
            }
            if tool
                .response
                .error_array_pointer
                .as_ref()
                .is_some_and(|pointer| {
                    !pointer.starts_with('/')
                        || pointer.len() > 256
                        || pointer.chars().any(char::is_control)
                })
            {
                return Err(ConnectorError::Manifest("error array pointer"));
            }
            if tool.request.method == HttpMethod::Get
                && matches!(
                    manifest.credential_delivery,
                    Some(CredentialDelivery::JsonBody { .. })
                )
            {
                return Err(ConnectorError::Manifest(
                    "JSON credentials require a request body",
                ));
            }
            if let Some(pagination) = &tool.response.pagination
                && (tool.request.method != HttpMethod::Get
                    || tool.response.result.is_some()
                    || pagination.max_pages == 0
                    || pagination.max_pages > 10
                    || !pagination.items_pointer.starts_with('/')
                    || !pagination.next_pointer.starts_with('/'))
            {
                return Err(ConnectorError::Manifest(
                    "pagination must be bounded GET next-link pagination",
                ));
            }
            if tools
                .insert(
                    tool.name.clone(),
                    CompiledTool {
                        definition: tool.clone(),
                        validator,
                    },
                )
                .is_some()
            {
                return Err(ConnectorError::Manifest("duplicate tool name"));
            }
        }
        Ok(Self {
            client: security::http_client(config.allow_private_network)?,
            manifest,
            configuration: if config.configuration.is_null() {
                json!({})
            } else {
                config.configuration
            },
            base_url,
            configuration_validator,
            has_required_base_url,
            tools,
            calls: Semaphore::new(16),
        })
    }

    pub fn manifest(&self) -> &ConnectorManifest {
        &self.manifest
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    pub fn validate_configuration(&self) -> Result<(), ConnectorError> {
        if !self.has_required_base_url
            || self
                .configuration_validator
                .as_ref()
                .is_some_and(|v| !v.is_valid(&self.configuration))
        {
            return Err(ConnectorError::Manifest(
                "required connector configuration missing or invalid",
            ));
        }
        Ok(())
    }

    pub async fn call(
        &self,
        tool_name: &str,
        args: Value,
        context: CallContext<'_>,
    ) -> Result<Value, ConnectorError> {
        if context.user_id.trim().is_empty() {
            return Err(ConnectorError::Authentication);
        }
        self.validate_configuration()?;
        let token = if self.manifest.auth != ConnectorAuth::None {
            Some(
                context
                    .access_token
                    .filter(|t| {
                        !t.is_empty()
                            && t.len() <= 16384
                            && !t.chars().any(char::is_whitespace)
                            && !t.chars().any(char::is_control)
                    })
                    .ok_or(ConnectorError::Authentication)?,
            )
        } else {
            None
        };
        let tool = self
            .tools
            .get(tool_name)
            .ok_or(ConnectorError::ToolNotFound)?;
        if serde_json::to_vec(&args)
            .map_err(|_| ConnectorError::Arguments)?
            .len()
            > MAX_ARGUMENT_BYTES
            || !tool.validator.is_valid(&args)
        {
            return Err(ConnectorError::Arguments);
        }
        let _permit = self.calls.try_acquire().map_err(|_| ConnectorError::Busy)?;
        tokio::time::timeout(
            Duration::from_secs(30),
            self.execute(&tool.definition, args, token),
        )
        .await
        .map_err(|_| ConnectorError::Timeout)?
    }

    fn mapping(
        &self,
        expression: &MappingExpression,
        context: &MappingContext,
    ) -> Result<Value, ConnectorError> {
        evaluate_mapping_bounded(
            expression,
            context,
            &self.manifest.mappings,
            50_000,
            4 * 1024 * 1024,
        )
        .map_err(|_| ConnectorError::Mapping)
    }

    fn request_url(
        &self,
        tool: &ConnectorTool,
        context: &MappingContext,
    ) -> Result<Url, ConnectorError> {
        let mut url = self.base_url.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| ConnectorError::Destination)?;
            segments.pop_if_empty();
            for segment in tool.request.path.trim_start_matches('/').split('/') {
                let part = if let Some(key) =
                    segment.strip_prefix('{').and_then(|s| s.strip_suffix('}'))
                {
                    let value = self.mapping(
                        tool.request
                            .path_parameters
                            .get(key)
                            .ok_or(ConnectorError::Mapping)?,
                        context,
                    )?;
                    scalar(&value)?
                } else {
                    segment.to_string()
                };
                if part == "."
                    || part == ".."
                    || part.contains(['/', '\\', '%', '?', '#'])
                    || part.chars().any(char::is_control)
                {
                    return Err(ConnectorError::Arguments);
                }
                segments.push(&part);
            }
        }
        for (key, expression) in &tool.request.query {
            let value = self.mapping(expression, context)?;
            if !value.is_null() {
                let value = scalar(&value)?;
                if key.len().saturating_add(value.len()) > MAX_URL_BYTES {
                    return Err(ConnectorError::Arguments);
                }
                url.query_pairs_mut().append_pair(key, &value);
                if url.as_str().len() > MAX_URL_BYTES {
                    return Err(ConnectorError::Arguments);
                }
            }
        }
        if url.as_str().len() > MAX_URL_BYTES {
            return Err(ConnectorError::Arguments);
        }
        Ok(url)
    }

    async fn execute(
        &self,
        tool: &ConnectorTool,
        args: Value,
        token: Option<&str>,
    ) -> Result<Value, ConnectorError> {
        let context = MappingContext::new(json!({"request": args, "config": self.configuration}));
        let mut url = self.request_url(tool, &context)?;
        let mut visited = HashSet::new();
        let mut items = Vec::new();
        let mut total_bytes = 0usize;
        let max_pages = tool.response.pagination.as_ref().map_or(1, |p| p.max_pages);
        for page in 0..max_pages {
            if !visited.insert(url.to_string()) {
                return Err(ConnectorError::Pagination);
            }
            let mut request = self.client.request(
                reqwest::Method::from_bytes(tool.request.method.as_str().as_bytes())
                    .map_err(|_| ConnectorError::Mapping)?,
                url.clone(),
            );
            let mut header_bytes = 0usize;
            for (name, expression) in &tool.request.headers {
                let value = self.mapping(expression, &context)?;
                if !value.is_null() {
                    let value = scalar(&value)?;
                    header_bytes = header_bytes
                        .saturating_add(name.len())
                        .saturating_add(value.len());
                    if header_bytes > MAX_HEADER_BYTES {
                        return Err(ConnectorError::Arguments);
                    }
                    let value = reqwest::header::HeaderValue::from_str(&value)
                        .map_err(|_| ConnectorError::Mapping)?;
                    request = request.header(name, value);
                }
            }
            if let Some(token) = token {
                match &self.manifest.credential_delivery {
                    Some(CredentialDelivery::Header { name, prefix }) => {
                        request = request.header(
                            name,
                            format!("{}{token}", prefix.as_deref().unwrap_or_default()),
                        );
                    }
                    Some(CredentialDelivery::JsonBody { .. }) => {}
                    None => request = request.bearer_auth(token),
                }
            }
            let mut body = tool
                .request
                .body
                .as_ref()
                .map(|body| self.mapping(body, &context))
                .transpose()?;
            if let (Some(token), Some(CredentialDelivery::JsonBody { field })) =
                (token, &self.manifest.credential_delivery)
            {
                let object = body
                    .get_or_insert_with(|| json!({}))
                    .as_object_mut()
                    .ok_or(ConnectorError::Mapping)?;
                if object.contains_key(field) {
                    return Err(ConnectorError::Manifest("credential body collision"));
                }
                object.insert(field.clone(), Value::String(token.to_string()));
            }
            if let Some(body) = body {
                if serde_json::to_vec(&body)
                    .map_err(|_| ConnectorError::Mapping)?
                    .len()
                    > MAX_ARGUMENT_BYTES
                {
                    return Err(ConnectorError::Arguments);
                }
                request = request.json(&body);
            }
            let mut response = request.send().await.map_err(|error| {
                if error.is_timeout() {
                    ConnectorError::Timeout
                } else {
                    ConnectorError::Transport
                }
            })?;
            if !response.status().is_success() {
                let retry_after_seconds = response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|h| h.to_str().ok())
                    .and_then(|h| h.parse::<u64>().ok())
                    .map(|v| v.min(3600));
                return Err(ConnectorError::Http {
                    status: response.status().as_u16(),
                    retry_after_seconds,
                });
            }
            if response
                .content_length()
                .is_some_and(|size| size > MAX_RESULT_BYTES as u64)
            {
                return Err(ConnectorError::ResponseTooLarge);
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| ConnectorError::Transport)?
            {
                total_bytes = total_bytes.saturating_add(chunk.len());
                if bytes.len().saturating_add(chunk.len()) > MAX_RESULT_BYTES
                    || total_bytes > MAX_RESULT_BYTES * 4
                {
                    return Err(ConnectorError::ResponseTooLarge);
                }
                bytes.extend_from_slice(&chunk);
            }
            let body: Value = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).map_err(|_| ConnectorError::Response)?
            };
            let response_context = MappingContext::new(
                json!({"request": context.root()["request"], "config": self.configuration, "session": {"response": body}}),
            );
            if let Some(error_if) = &tool.response.error_if {
                match self.mapping(error_if, &response_context)? {
                    Value::Bool(true) => return Err(ConnectorError::ProviderRejected),
                    Value::Bool(false) | Value::Null => {}
                    _ => return Err(ConnectorError::Response),
                }
            }
            if let Some(pointer) = &tool.response.error_array_pointer {
                match body.pointer(pointer) {
                    Some(Value::Array(errors)) if !errors.is_empty() => {
                        return Err(ConnectorError::ProviderRejected);
                    }
                    Some(Value::Array(_)) | Some(Value::Null) | None => {}
                    _ => return Err(ConnectorError::Response),
                }
            }
            if let Some(pagination) = &tool.response.pagination {
                let page_items = body
                    .pointer(&pagination.items_pointer)
                    .and_then(Value::as_array)
                    .ok_or(ConnectorError::Response)?;
                items.extend(page_items.iter().cloned());
                if serde_json::to_vec(&items)
                    .map_err(|_| ConnectorError::Response)?
                    .len()
                    > MAX_RESULT_BYTES
                {
                    return Err(ConnectorError::ResponseTooLarge);
                }
                match body.pointer(&pagination.next_pointer) {
                    None | Some(Value::Null) => return Ok(Value::Array(items)),
                    Some(Value::String(next)) if next.is_empty() => return Ok(Value::Array(items)),
                    Some(Value::String(next)) => {
                        if page + 1 == max_pages {
                            return Err(ConnectorError::Pagination);
                        }
                        let next = url.join(next).map_err(|_| ConnectorError::Pagination)?;
                        if next.as_str().len() > MAX_URL_BYTES
                            || next.origin() != self.base_url.origin()
                            || !next.username().is_empty()
                            || next.password().is_some()
                            || next.fragment().is_some()
                            || !within_base_path(&next, &self.base_url)
                        {
                            return Err(ConnectorError::Destination);
                        }
                        url = next;
                    }
                    _ => return Err(ConnectorError::Response),
                }
            } else {
                let result = match &tool.response.result {
                    Some(mapping) => self.mapping(mapping, &response_context)?,
                    None => body,
                };
                if serde_json::to_vec(&result)
                    .map_err(|_| ConnectorError::Response)?
                    .len()
                    > MAX_RESULT_BYTES
                {
                    return Err(ConnectorError::ResponseTooLarge);
                }
                return Ok(result);
            }
        }
        Err(ConnectorError::Pagination)
    }
}

fn identifier(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn scalar(value: &Value) -> Result<String, ConnectorError> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Number(_) | Value::Bool(_) => Ok(value.to_string()),
        _ => Err(ConnectorError::Mapping),
    }
}

fn within_base_path(url: &Url, base: &Url) -> bool {
    let prefix = base.path().trim_end_matches('/');
    url.path() == prefix || url.path().starts_with(&format!("{prefix}/"))
}

fn validate_schema(value: &Value, depth: usize) -> Result<(), ConnectorError> {
    if depth > 32 {
        return Err(ConnectorError::Manifest("schema too deep"));
    }
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if matches!(key.as_str(), "$ref" | "$dynamicRef" | "$recursiveRef")
                    && !value.as_str().is_some_and(|s| s.starts_with('#'))
                {
                    return Err(ConnectorError::Manifest(
                        "external schema references forbidden",
                    ));
                }
                validate_schema(value, depth + 1)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                validate_schema(value, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}
