// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::models::mcp_servers::McpTransportType;
use crate::services::http_client::HttpTimeouts;
use chrono::{DateTime, Duration, Utc};
use connector_server_plugin::{
    CallContext, CompiledConnector, SessionCache, SessionCachePolicy, SessionLease,
};
use oauth2::{
    EndpointNotSet as OAuthEndpointNotSet, EndpointSet as OAuthEndpointSet, basic::BasicClient,
};
use openidconnect::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet, EndpointSet,
    IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl,
    RefreshToken, RequestTokenError, Scope, TokenUrl,
    core::{CoreAuthenticationFlow, CoreClient, CoreJsonWebKeySet},
};
use reqwest::{
    Client as ReqwestClient,
    header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderName, HeaderValue},
};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, CallToolResult, JsonObject, Tool},
    transport::{StreamableHttpClientTransport, TokioChildProcess},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use thiserror::Error;
use tokio::process::Command;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum McpClientError {
    #[error("rmcp client error: {0}")]
    Rmcp(String),
    #[error("oauth error: {0}")]
    OAuth(String),
    #[error("oauth token endpoint unreachable: {0}")]
    OAuthUnreachable(String),
    #[error("invalid oauth configuration: {0}")]
    OAuthConfig(String),
    #[error("invalid tool args: {0}")]
    ToolArgs(String),
    #[error("mcp config error: {0}")]
    Config(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpOAuthConfig {
    /// Required by `openidconnect::CoreClient` even when using OAuth-only flows.
    pub issuer_url: String,
    pub auth_url: String,
    pub token_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token_include_grant_type: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_request_include_client_id: Option<bool>,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_url: String,
    pub scopes: Vec<String>,
    #[serde(default)]
    pub extra_params: HashMap<String, String>,
    #[serde(default)]
    pub flow: McpOAuthFlow,
    #[serde(default = "default_use_pkce")]
    pub use_pkce: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_endpoint_auth_method: Option<McpTokenEndpointAuthMethod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_response: Option<McpTokenResponsePaths>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token_response: Option<McpTokenResponsePaths>,
    #[serde(default)]
    pub token_request_format: McpTokenRequestFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token_request_format: Option<McpTokenRequestFormat>,
    #[serde(default)]
    pub scope_separator: McpScopeSeparator,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpScopeSeparator {
    #[default]
    Space,
    Comma,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTokenRequestFormat {
    #[default]
    Form,
    Json,
    Query,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpTokenResponsePaths {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<String>,
    pub token_type: Option<String>,
    pub scope: Option<String>,
    pub success: Option<String>,
    #[serde(default)]
    pub expiry_source: connector_server_plugin::OAuthExpirySource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTokenEndpointAuthMethod {
    ClientSecretPost,
    ClientSecretBasic,
    None,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpOAuthFlow {
    #[default]
    Oauth2,
    Oidc,
}

fn default_use_pkce() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize)]
struct McpOAuthConfigInput {
    pub issuer_url: Option<String>,
    pub auth_url: String,
    pub token_url: String,
    pub refresh_token_url: Option<String>,
    pub refresh_token_include_grant_type: Option<bool>,
    pub token_request_include_client_id: Option<bool>,
    pub redirect_url: String,
    pub scopes: Vec<String>,
    #[serde(default)]
    pub extra_params: HashMap<String, String>,
    #[serde(default)]
    pub flow: McpOAuthFlow,
    #[serde(default)]
    pub use_pkce: Option<bool>,
    #[serde(default)]
    pub token_endpoint_auth_method: Option<McpTokenEndpointAuthMethod>,
    #[serde(default)]
    pub token_response: Option<McpTokenResponsePaths>,
    #[serde(default)]
    pub refresh_token_response: Option<McpTokenResponsePaths>,
    #[serde(default)]
    pub token_request_format: McpTokenRequestFormat,
    pub refresh_token_request_format: Option<McpTokenRequestFormat>,
    #[serde(default)]
    pub scope_separator: McpScopeSeparator,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpOAuthAuthorization {
    pub authorization_url: String,
    pub state: String,
    pub pkce_verifier: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpOAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub scopes: Vec<String>,
}

type OAuthClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointSet,
    EndpointNotSet,
>;

type OAuth2Client = BasicClient<
    OAuthEndpointSet,
    OAuthEndpointNotSet,
    OAuthEndpointNotSet,
    OAuthEndpointNotSet,
    OAuthEndpointSet,
>;

fn build_oidc_client(config: &McpOAuthConfig) -> Result<OAuthClient, McpClientError> {
    validate_token_auth(config)?;
    let issuer = IssuerUrl::new(config.issuer_url.clone())
        .map_err(|e| McpClientError::OAuthConfig(e.to_string()))?;
    let auth_url = AuthUrl::new(config.auth_url.clone())
        .map_err(|e| McpClientError::OAuthConfig(e.to_string()))?;
    let token_url = TokenUrl::new(config.token_url.clone())
        .map_err(|e| McpClientError::OAuthConfig(e.to_string()))?;
    let redirect_url = RedirectUrl::new(config.redirect_url.clone())
        .map_err(|e| McpClientError::OAuthConfig(e.to_string()))?;

    let mut client = CoreClient::new(
        ClientId::new(config.client_id.clone()),
        issuer,
        CoreJsonWebKeySet::new(Vec::new()),
    )
    .set_auth_uri(auth_url)
    .set_token_uri(token_url)
    .set_redirect_uri(redirect_url);
    if let Some(method) = config.token_endpoint_auth_method {
        client = client.set_auth_type(match method {
            McpTokenEndpointAuthMethod::ClientSecretBasic => openidconnect::AuthType::BasicAuth,
            _ => openidconnect::AuthType::RequestBody,
        });
    }

    Ok(match config.client_secret.as_ref() {
        Some(secret) => client.set_client_secret(ClientSecret::new(secret.clone())),
        None => client,
    })
}

fn build_oauth2_client(config: &McpOAuthConfig) -> Result<OAuth2Client, McpClientError> {
    let auth_url = AuthUrl::new(config.auth_url.clone())
        .map_err(|e| McpClientError::OAuthConfig(e.to_string()))?;
    let token_url = TokenUrl::new(config.token_url.clone())
        .map_err(|e| McpClientError::OAuthConfig(e.to_string()))?;
    let redirect_url = RedirectUrl::new(config.redirect_url.clone())
        .map_err(|e| McpClientError::OAuthConfig(e.to_string()))?;

    let mut client = BasicClient::new(ClientId::new(config.client_id.clone()))
        .set_auth_uri(auth_url)
        .set_token_uri(token_url)
        .set_redirect_uri(redirect_url);
    if let Some(secret) = config.client_secret.as_ref() {
        client = client.set_client_secret(ClientSecret::new(secret.clone()));
    }

    Ok(client)
}

fn tokens_from_response(token_response: &openidconnect::core::CoreTokenResponse) -> McpOAuthTokens {
    let access_token = token_response.access_token().secret().to_string();
    let refresh_token = token_response
        .refresh_token()
        .map(|token| token.secret().to_string());
    let expires_at = token_response.expires_in().and_then(|duration| {
        Duration::from_std(duration)
            .ok()
            .map(|delta| Utc::now() + delta)
    });
    let token_type = Some(format!("{:?}", token_response.token_type()));
    let scopes = token_response
        .scopes()
        .map(|items| items.iter().map(|scope| scope.to_string()).collect())
        .unwrap_or_default();

    McpOAuthTokens {
        access_token,
        refresh_token,
        token_type,
        expires_at,
        scopes,
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ExpiresIn {
    Number(i64),
    Text(String),
}

impl ExpiresIn {
    fn as_seconds(&self) -> Option<i64> {
        match self {
            ExpiresIn::Number(value) => Some(*value),
            ExpiresIn::Text(text) => text.parse::<i64>().ok(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ScopeField {
    Text(String),
    List(Vec<String>),
}

#[derive(Debug, Deserialize)]
struct OAuth2TokenResponsePayload {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    expires_in: Option<ExpiresIn>,
    #[serde(default)]
    scope: Option<ScopeField>,
}

fn parse_oauth2_token_response(
    body: &str,
    paths: Option<&McpTokenResponsePaths>,
) -> Result<McpOAuthTokens, McpClientError> {
    let invalid = || McpClientError::OAuth("invalid token response".into());
    let mut value: Value = serde_json::from_str(body).map_err(|_| invalid())?;
    if let Some(paths) = paths {
        if paths
            .success
            .as_ref()
            .is_some_and(|pointer| value.pointer(pointer) != Some(&Value::Bool(true)))
        {
            return Err(invalid());
        }
        let mut selected = serde_json::Map::new();
        for (field, pointer) in [
            ("access_token", Some(&paths.access_token)),
            ("refresh_token", paths.refresh_token.as_ref()),
            ("expires_in", paths.expires_in.as_ref()),
            ("token_type", paths.token_type.as_ref()),
            ("scope", paths.scope.as_ref()),
        ] {
            if let Some(value) = pointer.and_then(|pointer| value.pointer(pointer)) {
                selected.insert(field.into(), value.clone());
            }
        }
        value = Value::Object(selected);
    }
    let payload: OAuth2TokenResponsePayload =
        serde_json::from_value(value).map_err(|_| invalid())?;
    let valid = |token: &str| {
        !token.is_empty()
            && token.len() <= 16384
            && !token.chars().any(|c| c.is_control() || c.is_whitespace())
    };
    if !valid(&payload.access_token)
        || payload
            .refresh_token
            .as_ref()
            .is_some_and(|token| !valid(token))
    {
        return Err(invalid());
    }
    let mut expires_at = payload
        .expires_in
        .as_ref()
        .map(|expiry| {
            let seconds = expiry
                .as_seconds()
                .filter(|seconds| *seconds >= 0)
                .ok_or_else(invalid)?;
            let duration = Duration::try_seconds(seconds).ok_or_else(invalid)?;
            Utc::now().checked_add_signed(duration).ok_or_else(invalid)
        })
        .transpose()?;
    if let Some(paths) = paths
        && let Some(timestamp) = paths
            .expiry_source
            .expiry_hint(&payload.access_token, Utc::now().timestamp())
            .map_err(|_| invalid())?
    {
        let hint = DateTime::from_timestamp(timestamp, 0).ok_or_else(invalid)?;
        expires_at = Some(expires_at.map_or(hint, |reported| reported.min(hint)));
    }

    let scopes = match payload.scope {
        Some(ScopeField::Text(text)) => text
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|item| !item.is_empty())
            .map(|item| item.to_string())
            .collect(),
        Some(ScopeField::List(items)) => items,
        None => Vec::new(),
    };

    Ok(McpOAuthTokens {
        access_token: payload.access_token,
        refresh_token: payload.refresh_token,
        token_type: payload.token_type,
        expires_at,
        scopes,
    })
}

fn summarize_oauth_error(body: &str) -> String {
    let error = serde_json::from_str::<Value>(body).ok().and_then(|value| {
        value
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    match error.as_deref() {
        Some(
            "invalid_request"
            | "invalid_client"
            | "invalid_grant"
            | "unauthorized_client"
            | "unsupported_grant_type"
            | "invalid_scope"
            | "access_denied"
            | "temporarily_unavailable"
            | "server_error",
        ) => error.unwrap_or_default(),
        _ => "oauth_error".into(),
    }
}

async fn token_response_body(mut response: reqwest::Response) -> Result<String, McpClientError> {
    const LIMIT: usize = 64 * 1024;
    let invalid = || McpClientError::OAuth("invalid or oversized token response".into());
    if response
        .content_length()
        .is_some_and(|size| size > LIMIT as u64)
    {
        return Err(invalid());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| invalid())? {
        if body.len().saturating_add(chunk.len()) > LIMIT {
            return Err(invalid());
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|_| invalid())
}

pub fn build_authorization_url(
    config: &McpOAuthConfig,
) -> Result<McpOAuthAuthorization, McpClientError> {
    match config.flow {
        McpOAuthFlow::Oidc => build_oidc_authorization_url(config),
        McpOAuthFlow::Oauth2 => build_oauth2_authorization_url(config),
    }
}

fn build_oidc_authorization_url(
    config: &McpOAuthConfig,
) -> Result<McpOAuthAuthorization, McpClientError> {
    let client = build_oidc_client(config)?;
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    let mut request = client.authorize_url(
        CoreAuthenticationFlow::AuthorizationCode,
        CsrfToken::new_random,
        Nonce::new_random,
    );

    for scope in &config.scopes {
        request = request.add_scope(Scope::new(scope.clone()));
    }
    for (key, value) in &config.extra_params {
        request = request.add_extra_param(key.clone(), value.clone());
    }

    let request = if config.use_pkce {
        request.set_pkce_challenge(pkce_challenge)
    } else {
        request
    };

    let (auth_url, csrf_token, _nonce) = request.url();

    Ok(McpOAuthAuthorization {
        authorization_url: auth_url.to_string(),
        state: csrf_token.secret().to_string(),
        pkce_verifier: pkce_verifier.secret().to_string(),
    })
}

fn build_oauth2_authorization_url(
    config: &McpOAuthConfig,
) -> Result<McpOAuthAuthorization, McpClientError> {
    let client = build_oauth2_client(config)?;
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    let mut request = client.authorize_url(CsrfToken::new_random);
    for scope in &config.scopes {
        request = request.add_scope(Scope::new(scope.clone()));
    }
    for (key, value) in &config.extra_params {
        request = request.add_extra_param(key.clone(), value.clone());
    }

    let request = if config.use_pkce {
        request.set_pkce_challenge(pkce_challenge)
    } else {
        request
    };

    let (auth_url, csrf_token) = request.url();

    let auth_url = if matches!(config.scope_separator, McpScopeSeparator::Comma) {
        let mut url = auth_url;
        let pairs: Vec<_> = url
            .query_pairs()
            .filter(|(key, _)| key != "scope")
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        url.set_query(None);
        url.query_pairs_mut().extend_pairs(pairs);
        if !config.scopes.is_empty() {
            url.query_pairs_mut()
                .append_pair("scope", &config.scopes.join(","));
        }
        url
    } else {
        auth_url
    };

    Ok(McpOAuthAuthorization {
        authorization_url: auth_url.to_string(),
        state: csrf_token.secret().to_string(),
        pkce_verifier: pkce_verifier.secret().to_string(),
    })
}

pub async fn exchange_code(
    config: &McpOAuthConfig,
    code: &str,
    pkce_verifier: &str,
    http_client: &ReqwestClient,
) -> Result<McpOAuthTokens, McpClientError> {
    match config.flow {
        McpOAuthFlow::Oidc => {
            let client = build_oidc_client(config)?;
            let mut request = client.exchange_code(AuthorizationCode::new(code.to_string()));
            if config.use_pkce {
                request =
                    request.set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier.to_string()));
            }
            let token_response = request.request_async(http_client).await.map_err(|e| {
                // Only a request that never got a response leaves the code unused.
                if matches!(e, RequestTokenError::Request(_)) {
                    McpClientError::OAuthUnreachable("request failed".into())
                } else {
                    McpClientError::OAuth("token exchange rejected".into())
                }
            })?;
            Ok(tokens_from_response(&token_response))
        }
        McpOAuthFlow::Oauth2 => {
            let mut form = vec![
                ("grant_type".to_string(), "authorization_code".to_string()),
                ("code".to_string(), code.to_string()),
                ("redirect_uri".to_string(), config.redirect_url.clone()),
            ];
            if config.use_pkce {
                form.push(("code_verifier".to_string(), pkce_verifier.to_string()));
            }
            let response = oauth2_token_request(config, http_client, form)?
                .send()
                .await
                .map_err(|_| McpClientError::OAuthUnreachable("request failed".into()))?;
            let status = response.status();
            let body = token_response_body(response).await?;
            if !status.is_success() {
                return Err(McpClientError::OAuth(format!(
                    "token endpoint {status}: {}",
                    summarize_oauth_error(&body)
                )));
            }
            parse_oauth2_token_response(&body, config.token_response.as_ref())
        }
    }
}

pub async fn refresh_token(
    config: &McpOAuthConfig,
    refresh_token: &str,
    http_client: &ReqwestClient,
) -> Result<McpOAuthTokens, McpClientError> {
    validate_token_auth(config)?;
    match config.flow {
        McpOAuthFlow::Oidc => {
            let client = build_oidc_client(config)?;
            let token_response = client
                .exchange_refresh_token(&RefreshToken::new(refresh_token.to_string()))
                .request_async(http_client)
                .await
                .map_err(|_| McpClientError::OAuth("token refresh rejected".into()))?;
            Ok(tokens_from_response(&token_response))
        }
        McpOAuthFlow::Oauth2 => {
            let mut form = vec![("refresh_token".to_string(), refresh_token.to_string())];
            if config.refresh_token_include_grant_type.unwrap_or(true) {
                form.push(("grant_type".to_string(), "refresh_token".to_string()));
            }
            let mut refresh_config = config.clone();
            if let Some(url) = &config.refresh_token_url {
                refresh_config.token_url = url.clone();
            }
            refresh_config.refresh_token_url = None;
            if let Some(format) = config.refresh_token_request_format {
                refresh_config.token_request_format = format;
            }
            let response = oauth2_token_request(&refresh_config, http_client, form)?
                .send()
                .await
                .map_err(|_| McpClientError::OAuth("token refresh request failed".into()))?;
            let status = response.status();
            let body = token_response_body(response).await?;
            if !status.is_success() {
                return Err(McpClientError::OAuth(format!(
                    "token endpoint {status}: {}",
                    summarize_oauth_error(&body)
                )));
            }
            parse_oauth2_token_response(
                &body,
                config
                    .refresh_token_response
                    .as_ref()
                    .or(config.token_response.as_ref()),
            )
        }
    }
}

fn validate_token_auth(config: &McpOAuthConfig) -> Result<(), McpClientError> {
    if matches!(config.flow, McpOAuthFlow::Oidc)
        && (config.refresh_token_include_grant_type.is_some()
            || config.token_request_include_client_id.is_some()
            || config.refresh_token_request_format.is_some()
            || config.token_request_format != McpTokenRequestFormat::Form)
    {
        return Err(McpClientError::OAuthConfig(
            "custom token request requires oauth2 flow".into(),
        ));
    }
    if let Some(endpoint) = &config.refresh_token_url {
        let invalid = || {
            McpClientError::OAuthConfig(
                "refresh endpoint must share the token endpoint origin".into(),
            )
        };
        let token = url::Url::parse(&config.token_url).map_err(|_| invalid())?;
        let refresh = url::Url::parse(endpoint).map_err(|_| invalid())?;
        if !matches!(refresh.scheme(), "https" | "http")
            || refresh.origin() != token.origin()
            || !refresh.username().is_empty()
            || refresh.password().is_some()
            || refresh.fragment().is_some()
            || matches!(config.flow, McpOAuthFlow::Oidc)
        {
            return Err(invalid());
        }
    }
    if config.token_request_format == McpTokenRequestFormat::Query
        || config.refresh_token_request_format == Some(McpTokenRequestFormat::Query)
    {
        let endpoint = url::Url::parse(&config.token_url)
            .map_err(|_| McpClientError::OAuthConfig("invalid token endpoint".into()))?;
        let local = matches!(
            endpoint.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]")
        );
        if (endpoint.scheme() != "https" && !(endpoint.scheme() == "http" && local))
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(McpClientError::OAuthConfig(
                "query token requests require TLS or loopback testing".into(),
            ));
        }
    }
    for paths in config
        .token_response
        .iter()
        .chain(config.refresh_token_response.iter())
    {
        for pointer in std::iter::once(Some(&paths.access_token))
            .chain([
                paths.refresh_token.as_ref(),
                paths.expires_in.as_ref(),
                paths.token_type.as_ref(),
                paths.scope.as_ref(),
                paths.success.as_ref(),
            ])
            .flatten()
        {
            if !pointer.starts_with('/')
                || pointer.len() > 256
                || pointer.chars().any(char::is_control)
            {
                return Err(McpClientError::OAuthConfig(
                    "invalid token response pointer".into(),
                ));
            }
        }
        if matches!(config.flow, McpOAuthFlow::Oidc) {
            return Err(McpClientError::OAuthConfig(
                "custom token paths require oauth2 flow".into(),
            ));
        }
    }
    match config.token_endpoint_auth_method {
        Some(McpTokenEndpointAuthMethod::ClientSecretBasic)
            if config.client_secret.as_deref().is_none_or(str::is_empty) =>
        {
            Err(McpClientError::OAuthConfig(
                "client_secret_basic requires a client secret".to_string(),
            ))
        }
        Some(McpTokenEndpointAuthMethod::None) if config.client_secret.is_some() => {
            Err(McpClientError::OAuthConfig(
                "public client authentication must not configure a client secret".to_string(),
            ))
        }
        _ => Ok(()),
    }
}

fn oauth2_token_request(
    config: &McpOAuthConfig,
    http_client: &ReqwestClient,
    mut form: Vec<(String, String)>,
) -> Result<reqwest::RequestBuilder, McpClientError> {
    validate_token_auth(config)?;
    let mut request = http_client
        .post(&config.token_url)
        .header(ACCEPT, "application/json");
    match config
        .token_endpoint_auth_method
        .unwrap_or(McpTokenEndpointAuthMethod::ClientSecretPost)
    {
        McpTokenEndpointAuthMethod::ClientSecretBasic => {
            let encode = |value: &str| {
                url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>()
            };
            request = request.basic_auth(
                encode(&config.client_id),
                config.client_secret.as_deref().map(encode),
            );
            if config.token_request_include_client_id.unwrap_or(false) {
                form.push(("client_id".to_string(), config.client_id.clone()));
            }
        }
        method => {
            form.push(("client_id".to_string(), config.client_id.clone()));
            if method == McpTokenEndpointAuthMethod::ClientSecretPost
                && let Some(secret) = config.client_secret.as_ref()
            {
                form.push(("client_secret".to_string(), secret.clone()));
            }
        }
    }
    Ok(match config.token_request_format {
        McpTokenRequestFormat::Form => request.form(&form),
        McpTokenRequestFormat::Json => request.json(&form.into_iter().collect::<HashMap<_, _>>()),
        McpTokenRequestFormat::Query => request.query(&form),
    })
}

pub fn oauth_config_from_connection(
    connection_config: &Value,
    client_id: &str,
    client_secret: Option<String>,
) -> Result<McpOAuthConfig, McpClientError> {
    let oauth_value = connection_config
        .get("oauth")
        .ok_or_else(|| McpClientError::OAuthConfig("missing oauth config".to_string()))?;
    let input: McpOAuthConfigInput = serde_json::from_value(oauth_value.clone())
        .map_err(|_| McpClientError::OAuthConfig("invalid oauth config".into()))?;
    let issuer_url = input
        .issuer_url
        .clone()
        .unwrap_or_else(|| input.auth_url.clone());
    let config = McpOAuthConfig {
        issuer_url,
        auth_url: input.auth_url,
        token_url: input.token_url,
        refresh_token_url: input.refresh_token_url,
        refresh_token_include_grant_type: input.refresh_token_include_grant_type,
        token_request_include_client_id: input.token_request_include_client_id,
        redirect_url: input.redirect_url,
        client_id: client_id.to_string(),
        client_secret,
        scopes: input.scopes,
        extra_params: input.extra_params,
        flow: input.flow,
        use_pkce: input.use_pkce.unwrap_or(true),
        token_endpoint_auth_method: input.token_endpoint_auth_method,
        token_response: input.token_response,
        refresh_token_response: input.refresh_token_response,
        token_request_format: input.token_request_format,
        refresh_token_request_format: input.refresh_token_request_format,
        scope_separator: input.scope_separator,
    };
    validate_token_auth(&config)?;
    Ok(config)
}

fn rmcp_args_from_value(args: Value) -> Result<Option<JsonObject>, McpClientError> {
    match args {
        Value::Object(map) => Ok(Some(map.into_iter().collect())),
        Value::Null => Ok(None),
        _ => Err(McpClientError::ToolArgs(
            "tool arguments must be a JSON object".to_string(),
        )),
    }
}

const STDIO_COMMANDS_ENV: &str = "MCP_STDIO_COMMANDS";
const DEFAULT_STDIO_COMMANDS: &str = "sqlx-mcp,/usr/local/bin/sqlx-mcp";
const MCP_SECRET_ENV_PREFIX: &str = "MCP_SECRET_";
const STDIO_INHERITED_ENV: [&str; 4] = ["PATH", "HOME", "LANG", "RUST_LOG"];

fn stdio_command_allowed(command: &str, allowlist: &str) -> bool {
    allowlist
        .split(',')
        .map(str::trim)
        .any(|allowed| !allowed.is_empty() && allowed == command)
}

// Stdio servers run on the API host, so only operator-approved binaries may be configured.
pub fn ensure_stdio_command_allowed(command: &str) -> Result<(), McpClientError> {
    let allowlist =
        std::env::var(STDIO_COMMANDS_ENV).unwrap_or_else(|_| DEFAULT_STDIO_COMMANDS.to_string());
    if stdio_command_allowed(command, &allowlist) {
        Ok(())
    } else {
        Err(McpClientError::Config(format!(
            "stdio command {command} is not allowed; add it to {STDIO_COMMANDS_ENV}"
        )))
    }
}

fn stdio_child_env() -> Vec<(String, String)> {
    std::env::vars()
        .filter(|(name, _)| STDIO_INHERITED_ENV.contains(&name.as_str()) || is_mcp_secret_env(name))
        .collect()
}

async fn connect_rmcp_stdio(
    command: &str,
    args: &[String],
) -> Result<rmcp::service::RunningService<rmcp::service::RoleClient, ()>, McpClientError> {
    ensure_stdio_command_allowed(command)?;
    let mut cmd = Command::new(command);
    cmd.args(args);
    // The API's own secrets (APP_KEY, JWT_SECRET, DATABASE_URL) must not reach the child process.
    cmd.env_clear();
    cmd.envs(stdio_child_env());
    let transport = TokioChildProcess::new(cmd)
        .map_err(|e| McpClientError::Rmcp(format!("spawn stdio process failed: {e}")))?;
    let service = ()
        .serve(transport)
        .await
        .map_err(|e| McpClientError::Rmcp(format!("rmcp connect failed: {e}")))?;
    service
        .set_response_cache_config(rmcp::ClientCacheConfig::disabled())
        .await;
    Ok(service)
}

async fn connect_rmcp_http(
    base_url: &str,
    auth_header: Option<String>,
    connection_config: &Value,
) -> Result<rmcp::service::RunningService<rmcp::service::RoleClient, ()>, McpClientError> {
    let mut config = StreamableHttpClientTransportConfig::with_uri(base_url);
    if let Some(token) = auth_header {
        config = config.auth_header(token);
    }
    let client = mcp_http_client(connection_config)?;
    let transport = StreamableHttpClientTransport::with_client(client, config);
    let service = ()
        .serve(transport)
        .await
        .map_err(|e| McpClientError::Rmcp(format!("rmcp connect failed: {e}")))?;
    service
        .set_response_cache_config(rmcp::ClientCacheConfig::disabled())
        .await;
    Ok(service)
}

fn transport_config(connection_config: &Value) -> Result<TransportConfigInput, McpClientError> {
    let config: TransportConfigInput = serde_json::from_value(
        connection_config
            .get("transport_config")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
    )
    .map_err(|_| McpClientError::Config("invalid transport_config".into()))?;
    config
        .session_cache
        .unwrap_or_default()
        .validate()
        .map_err(|_| McpClientError::Config("invalid MCP session cache policy".into()))?;
    Ok(config)
}

fn mcp_http_client(connection_config: &Value) -> Result<reqwest_mcp::Client, McpClientError> {
    let config = transport_config(connection_config)?;
    let mut headers = HeaderMap::new();
    for (name, value) in config.headers.unwrap_or_default() {
        let header = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| McpClientError::Config(format!("invalid HTTP header name: {name}")))?;
        // OAuth user tokens and static Bearer tokens are selected per connection.
        if header == AUTHORIZATION {
            continue;
        }
        let resolved = resolve_env_placeholders(&value)?;
        let value = HeaderValue::from_str(&resolved)
            .map_err(|_| McpClientError::Config(format!("invalid HTTP header value for {name}")))?;
        headers.insert(header, value);
    }
    reqwest_mcp::Client::builder()
        .connect_timeout(HttpTimeouts::LONG_LIVED_STREAM.connect)
        .redirect(reqwest_mcp::redirect::Policy::none())
        .default_headers(headers)
        .build()
        .map_err(|e| McpClientError::Config(format!("http client build failed: {e}")))
}

pub struct McpServerClient {
    pub server_id: Uuid,
    pub name: String,
    pub transport_type: McpTransportType,
    pub url: Option<String>,
    pub connection_config: Value,
    pub db_url: Option<String>,
    client: Mutex<Option<rmcp::service::RunningService<rmcp::service::RoleClient, ()>>>,
    http_clients: Result<SessionCache<Uuid, UserHttpClient>, String>,
    connected: Mutex<bool>,
    plugin: Result<Option<CompiledConnector>, String>,
}

struct UserHttpClient {
    token_digest: [u8; 32],
    client: rmcp::service::RunningService<rmcp::service::RoleClient, ()>,
}

#[derive(Debug, Clone, Deserialize)]
struct StdioConnectionConfig {
    command: String,
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct TransportConfigInput {
    headers: Option<HashMap<String, String>>,
    session_cache: Option<SessionCachePolicy>,
}

impl McpServerClient {
    pub fn requires_user_credentials(&self) -> bool {
        self.plugin
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .is_some_and(|plugin| {
                plugin.manifest().auth != connector_server_plugin::ConnectorAuth::None
            })
    }
    pub fn new(
        server_id: Uuid,
        name: String,
        transport_type: McpTransportType,
        url: Option<String>,
        connection_config: Value,
        db_url: Option<String>,
    ) -> Self {
        let plugin = crate::services::mcp_plugin_helpers::compile_plugin(
            &connection_config,
            transport_type,
            url.as_deref(),
        )
        .map_err(|error| error.to_string());
        let http_clients = transport_config(&connection_config)
            .and_then(|config| {
                SessionCache::new(config.session_cache.unwrap_or_default())
                    .map_err(|error| McpClientError::Config(error.to_string()))
            })
            .map_err(|error| error.to_string());
        Self {
            server_id,
            name,
            transport_type,
            url,
            connection_config,
            db_url,
            client: Mutex::new(None),
            http_clients,
            connected: Mutex::new(false),
            plugin,
        }
    }

    pub async fn ensure_connected(&self) -> Result<(), McpClientError> {
        if self.connector_plugin()?.is_some() {
            return Ok(());
        }
        let mut connected = self.connected.lock().await;
        if *connected {
            return Ok(());
        }

        match self.transport_type {
            McpTransportType::Http | McpTransportType::Sse => {
                let base_url = self.http_url_for_transport()?;
                if self.transport_type == McpTransportType::Http
                    && self.connection_config.get("sse_url").is_some()
                {
                    eprintln!("rmcp streamable http ignores sse_url; using base_url only");
                }
                let auth_header = extract_auth_header(&self.connection_config)?;
                let service =
                    connect_rmcp_http(base_url, auth_header, &self.connection_config).await?;
                let mut client = self.client.lock().await;
                *client = Some(service);
            }
            McpTransportType::Websocket => {
                return Err(McpClientError::Config(
                    "mcp websocket transport not supported by rmcp".to_string(),
                ));
            }
            McpTransportType::Stdio => {
                let stdio = parse_stdio_config(&self.connection_config, self.db_url.as_deref())?;
                let service = connect_rmcp_stdio(&stdio.command, &stdio.args).await?;
                let mut client = self.client.lock().await;
                *client = Some(service);
            }
        }

        *connected = true;
        Ok(())
    }

    async fn authenticated_http_client(
        &self,
        token: String,
        user_id: Uuid,
    ) -> Result<SessionLease<UserHttpClient>, McpClientError> {
        let mut entry = self
            .http_clients
            .as_ref()
            .map_err(|error| McpClientError::Config(error.clone()))?
            .lease(user_id)
            .await
            .map_err(|error| McpClientError::Rmcp(error.to_string()))?;
        let token_digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        if entry
            .as_ref()
            .is_none_or(|client| client.token_digest != token_digest || client.client.is_closed())
        {
            let service = connect_rmcp_http(
                self.http_url_for_transport()?,
                Some(token),
                &self.connection_config,
            )
            .await?;
            *entry = Some(UserHttpClient {
                token_digest,
                client: service,
            });
        }
        Ok(entry)
    }

    pub async fn invalidate_user_session(&self, user_id: Uuid) {
        if let Ok(cache) = &self.http_clients {
            cache.invalidate(&user_id).await;
        }
    }

    pub async fn call_tool(
        &self,
        tool_name: &str,
        args: serde_json::Value,
        auth_token: Option<String>,
        user_id: Option<Uuid>,
    ) -> Result<CallToolResult, McpClientError> {
        if let Some(plugin) = self.connector_plugin()? {
            let user_id = user_id.ok_or_else(|| {
                McpClientError::Config("user_id required for connector plugin calls".into())
            })?;
            let result = plugin
                .call(
                    tool_name,
                    args,
                    CallContext {
                        user_id: &user_id.to_string(),
                        access_token: auth_token.as_deref(),
                    },
                )
                .await
                .map_err(|error| McpClientError::Rmcp(error.to_string()))?;
            return Ok(CallToolResult::structured(result));
        }
        match (self.transport_type, auth_token, user_id) {
            (McpTransportType::Http | McpTransportType::Sse, Some(token), Some(user_id)) => {
                let client = self.authenticated_http_client(token, user_id).await?;
                let entry = client.as_ref().ok_or_else(|| {
                    McpClientError::Rmcp("rmcp client not initialized".to_string())
                })?;
                let arguments = rmcp_args_from_value(args)?;
                entry
                    .client
                    .call_tool(
                        CallToolRequestParams::new(tool_name.to_string())
                            .with_arguments(arguments.unwrap_or_default()),
                    )
                    .await
                    .map_err(|e| McpClientError::Rmcp(e.to_string()))
            }
            (McpTransportType::Http | McpTransportType::Sse, Some(_), None) => Err(
                McpClientError::Config("user_id required for oauth mcp calls".to_string()),
            ),
            _ => {
                self.ensure_connected().await?;
                let mut client = self.client.lock().await;
                let service = client.as_mut().ok_or_else(|| {
                    McpClientError::Rmcp("rmcp client not initialized".to_string())
                })?;
                let arguments = rmcp_args_from_value(args)?;
                service
                    .call_tool(
                        CallToolRequestParams::new(tool_name.to_string())
                            .with_arguments(arguments.unwrap_or_default()),
                    )
                    .await
                    .map_err(|e| McpClientError::Rmcp(e.to_string()))
            }
        }
    }

    pub async fn list_tools(&self) -> Result<Vec<Tool>, McpClientError> {
        if let Some(plugin) = self.connector_plugin()? {
            return Ok(crate::services::mcp_plugin_helpers::plugin_tools(plugin));
        }
        self.ensure_connected().await?;
        let mut client = self.client.lock().await;
        let service = client
            .as_mut()
            .ok_or_else(|| McpClientError::Rmcp("rmcp client not initialized".to_string()))?;
        service
            .list_all_tools()
            .await
            .map_err(|e| McpClientError::Rmcp(e.to_string()))
    }

    pub async fn list_tools_with_auth(
        &self,
        auth_token: Option<String>,
        user_id: Option<Uuid>,
    ) -> Result<Vec<Tool>, McpClientError> {
        if self.connector_plugin()?.is_some() {
            return self.list_tools().await;
        }
        match (self.transport_type, auth_token, user_id) {
            (McpTransportType::Http | McpTransportType::Sse, Some(token), Some(user_id)) => {
                let client = self.authenticated_http_client(token, user_id).await?;
                let entry = client.as_ref().ok_or_else(|| {
                    McpClientError::Rmcp("rmcp client not initialized".to_string())
                })?;
                entry
                    .client
                    .list_all_tools()
                    .await
                    .map_err(|e| McpClientError::Rmcp(e.to_string()))
            }
            (McpTransportType::Http | McpTransportType::Sse, Some(_), None) => Err(
                McpClientError::Config("user_id required for oauth mcp calls".to_string()),
            ),
            _ => self.list_tools().await,
        }
    }

    pub fn http_url_for_transport(&self) -> Result<&str, McpClientError> {
        match self.transport_type {
            McpTransportType::Http => self
                .url
                .as_deref()
                .ok_or_else(|| McpClientError::Config("mcp http url missing".to_string())),
            McpTransportType::Sse => {
                if let Some(sse_url) = self
                    .connection_config
                    .get("sse_url")
                    .and_then(Value::as_str)
                {
                    return Ok(sse_url);
                }
                self.url
                    .as_deref()
                    .ok_or_else(|| McpClientError::Config("mcp sse url missing".to_string()))
            }
            _ => Err(McpClientError::Config(
                "mcp transport does not support http".to_string(),
            )),
        }
    }

    fn connector_plugin(&self) -> Result<Option<&CompiledConnector>, McpClientError> {
        self.plugin
            .as_ref()
            .map(Option::as_ref)
            .map_err(|error| McpClientError::Config(error.clone()))
    }
}

fn parse_stdio_config(
    connection_config: &Value,
    db_url: Option<&str>,
) -> Result<StdioConnectionConfig, McpClientError> {
    let mut config: StdioConnectionConfig = serde_json::from_value(connection_config.clone())
        .map_err(|e| McpClientError::Config(format!("invalid stdio config: {e}")))?;
    if let Some(db_url) = db_url {
        let has_placeholder = config.args.iter().any(|arg| {
            arg.contains("{{db_url}}") || arg.contains("$DB_URL") || arg.contains("${DB_URL}")
        });
        if has_placeholder {
            config.args = config
                .args
                .into_iter()
                .map(|arg| {
                    arg.replace("{{db_url}}", db_url)
                        .replace("${DB_URL}", db_url)
                        .replace("$DB_URL", db_url)
                })
                .collect();
        }
    }
    Ok(config)
}

fn extract_auth_header(connection_config: &Value) -> Result<Option<String>, McpClientError> {
    let config_value = match connection_config.get("transport_config") {
        Some(value) => value,
        None => return Ok(None),
    };
    let input: TransportConfigInput = serde_json::from_value(config_value.clone())
        .map_err(|e| McpClientError::Config(format!("invalid transport_config: {e}")))?;
    let headers = match input.headers {
        Some(headers) => headers,
        None => return Ok(None),
    };
    let header_value = match headers
        .get("Authorization")
        .or_else(|| headers.get("authorization"))
    {
        Some(value) => value,
        None => return Ok(None),
    };

    let resolved = resolve_env_placeholders(header_value)?;
    let token = resolved
        .strip_prefix("Bearer ")
        .unwrap_or(resolved.as_str());
    Ok(Some(token.to_string()))
}

fn resolve_env_placeholders(value: &str) -> Result<String, McpClientError> {
    let mut output = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '$' {
            if let Some('{') = chars.peek().copied() {
                chars.next();
                let mut var = String::new();
                for next in chars.by_ref() {
                    if next == '}' {
                        break;
                    }
                    var.push(next);
                }
                output.push_str(&resolve_env_var(&var)?);
                continue;
            }
            let mut var = String::new();
            while let Some(next) = chars.peek().copied() {
                if next.is_ascii_alphanumeric() || next == '_' {
                    var.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            if var.is_empty() {
                output.push(ch);
            } else {
                output.push_str(&resolve_env_var(&var)?);
            }
            continue;
        }
        if ch == '{' && matches!(chars.peek(), Some('{')) {
            chars.next();
            let mut var = String::new();
            let mut found_end = false;
            while let Some(next) = chars.next() {
                if next == '}' && matches!(chars.peek(), Some('}')) {
                    chars.next();
                    found_end = true;
                    break;
                }
                var.push(next);
            }
            if !found_end {
                return Err(McpClientError::Config(
                    "unterminated {{ENV_VAR}} placeholder".to_string(),
                ));
            }
            output.push_str(&resolve_env_var(&var)?);
            continue;
        }
        output.push(ch);
    }
    Ok(output)
}

fn is_mcp_secret_env(name: &str) -> bool {
    name.len() > MCP_SECRET_ENV_PREFIX.len() && name.starts_with(MCP_SECRET_ENV_PREFIX)
}

// Header placeholders may only read operator-provisioned MCP secrets, never the API's own keys.
fn resolve_env_var(name: &str) -> Result<String, McpClientError> {
    if !is_mcp_secret_env(name) {
        return Err(McpClientError::Config(format!(
            "environment placeholder {name} is not allowed; use an {MCP_SECRET_ENV_PREFIX}* variable"
        )));
    }
    std::env::var(name)
        .map_err(|_| McpClientError::Config(format!("missing environment variable: {name}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
    use base64::Engine;
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn rejects_cross_origin_or_credential_bearing_refresh_endpoints() {
        for url in [
            "https://other.example/refresh",
            "http://tokens.example/refresh",
            "https://name:secret@tokens.example/refresh",
            "https://tokens.example/refresh#fragment",
        ] {
            let mut config = oauth2_config("https://tokens.example/token".into());
            config.refresh_token_url = Some(url.into());
            assert!(validate_token_auth(&config).is_err());
        }
        let mut config = oauth2_config("https://tokens.example/token".into());
        config.refresh_token_url = Some("https://tokens.example/refresh".into());
        assert!(validate_token_auth(&config).is_ok());
    }

    fn oauth2_config(token_url: String) -> McpOAuthConfig {
        McpOAuthConfig {
            issuer_url: "https://issuer.example.com".to_string(),
            auth_url: "https://issuer.example.com/authorize".to_string(),
            token_url,
            refresh_token_url: None,
            refresh_token_include_grant_type: None,
            token_request_include_client_id: None,
            client_id: "client".to_string(),
            client_secret: None,
            redirect_url: "https://app.example.com/mcp/oauth/callback".to_string(),
            scopes: vec![],
            extra_params: HashMap::new(),
            flow: McpOAuthFlow::Oauth2,
            use_pkce: true,
            token_endpoint_auth_method: None,
            token_response: None,
            refresh_token_response: None,
            token_request_format: McpTokenRequestFormat::Form,
            refresh_token_request_format: None,
            scope_separator: McpScopeSeparator::Space,
        }
    }

    async fn token_endpoint_replying(status: &'static str, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request).await;
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        });
        format!("http://{address}/token")
    }

    #[test]
    fn nested_user_tokens_never_fall_back_to_a_bot_and_invalid_expiries_are_rejected() {
        let paths: McpTokenResponsePaths = serde_json::from_value(serde_json::json!({"access_token":"/authed_user/access_token","expires_in":"/authed_user/expires_in","scope":"/authed_user/scope","success":"/ok"})).unwrap();
        let token = parse_oauth2_token_response(r#"{"ok":true,"access_token":"bot","authed_user":{"access_token":"user","scope":"a,b c","expires_in":3600}}"#,Some(&paths)).unwrap();
        assert_eq!(token.access_token, "user");
        assert_eq!(token.scopes, vec!["a", "b", "c"]);
        for body in [
            r#"{"ok":true,"access_token":"bot"}"#,
            r#"{"ok":false,"authed_user":{"access_token":"user"}}"#,
        ] {
            assert!(parse_oauth2_token_response(body, Some(&paths)).is_err());
        }
        for expiry in [
            serde_json::json!(-1),
            serde_json::json!(i64::MAX),
            serde_json::json!("bad"),
        ] {
            assert!(
                parse_oauth2_token_response(
                    &serde_json::json!({"access_token":"valid","expires_in":expiry}).to_string(),
                    None
                )
                .is_err()
            );
        }
        for token in ["", " ", "a b", "a\nb"] {
            assert!(
                parse_oauth2_token_response(
                    &serde_json::json!({"access_token":token}).to_string(),
                    None
                )
                .is_err()
            );
        }
        assert!(
            parse_oauth2_token_response(r#"{"access_token":"valid","expires_in":0}"#, None)
                .unwrap()
                .expires_at
                .unwrap()
                <= Utc::now()
        );
    }

    #[test]
    fn comma_scopes_are_encoded_once_and_errors_never_include_provider_descriptions() {
        let mut config = oauth2_config("https://example.com/token".into());
        config.scope_separator = McpScopeSeparator::Comma;
        config.scopes = vec!["read_products".into(), "read_orders".into()];
        let authorization = build_authorization_url(&config).unwrap();
        let url = url::Url::parse(&authorization.authorization_url).unwrap();
        assert_eq!(url.query_pairs().filter(|(k, _)| k == "scope").count(), 1);
        assert_eq!(
            url.query_pairs().find(|(k, _)| k == "scope").unwrap().1,
            "read_products,read_orders"
        );
        assert_eq!(
            summarize_oauth_error(r#"{"error":"invalid_grant","error_description":"secret"}"#),
            "invalid_grant"
        );
        assert_eq!(summarize_oauth_error("upstream secret"), "oauth_error");
    }

    #[test]
    fn basic_client_id_body_is_opt_in_and_never_includes_the_secret() {
        let mut config = oauth2_config("https://tokens.example/token".into());
        config.client_secret = Some("private-fixture".into());
        config.token_endpoint_auth_method = Some(McpTokenEndpointAuthMethod::ClientSecretBasic);
        for include in [false, true] {
            config.token_request_include_client_id = Some(include);
            let request = oauth2_token_request(
                &config,
                &ReqwestClient::new(),
                vec![("grant_type".into(), "authorization_code".into())],
            )
            .unwrap()
            .build()
            .unwrap();
            let form: HashMap<_, _> =
                url::form_urlencoded::parse(request.body().unwrap().as_bytes().unwrap())
                    .into_owned()
                    .collect();
            assert_eq!(form.get("client_id"), include.then_some(&config.client_id));
            assert!(!form.contains_key("client_secret"));
            assert!(request.headers().contains_key(AUTHORIZATION));
        }
        config.flow = McpOAuthFlow::Oidc;
        assert!(validate_token_auth(&config).is_err());
    }

    #[test]
    fn jwt_expiry_requires_explicit_selection_and_never_extends_reported_lifetime() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let expiry = Utc::now().timestamp() + 3600;
        let token = format!(
            "e30.{}.fixture",
            URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{expiry}}}"#))
        );
        let paths = serde_json::from_value(serde_json::json!({
            "access_token":"/access_token", "expires_in":"/expires_in", "expiry_source":"jwt_exp"
        }))
        .unwrap();
        let body = serde_json::json!({"access_token":token});
        assert!(
            parse_oauth2_token_response(&body.to_string(), None)
                .unwrap()
                .expires_at
                .is_none()
        );
        assert_eq!(
            parse_oauth2_token_response(&body.to_string(), Some(&paths))
                .unwrap()
                .expires_at
                .unwrap()
                .timestamp(),
            expiry
        );
        let mut shorter = body;
        shorter["expires_in"] = serde_json::json!(60);
        assert!(
            parse_oauth2_token_response(&shorter.to_string(), Some(&paths))
                .unwrap()
                .expires_at
                .unwrap()
                .timestamp()
                < expiry
        );
        for invalid in ["opaque", "a.e30.c"] {
            assert!(
                parse_oauth2_token_response(
                    &serde_json::json!({"access_token":invalid}).to_string(),
                    Some(&paths)
                )
                .is_err()
            );
        }
    }

    #[tokio::test]
    async fn separate_refresh_endpoint_uses_basic_auth_and_refresh_only_body() {
        let router = Router::new().route("/refresh", post(|headers: HeaderMap, axum::extract::Form(body): axum::extract::Form<HashMap<String, String>>| async move {
            assert!(headers.get("authorization").unwrap().to_str().unwrap().starts_with("Basic "));
            assert_eq!(body.len(), 1);
            assert_eq!(body["refresh_token"], "previous");
            Json(serde_json::json!({"access_token":"renewed","expires_in":3600}))
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let mut config = oauth2_config(format!("{origin}/token"));
        config.client_secret = Some("fixture".into());
        config.token_endpoint_auth_method = Some(McpTokenEndpointAuthMethod::ClientSecretBasic);
        config.refresh_token_url = Some(format!("{origin}/refresh"));
        config.refresh_token_include_grant_type = Some(false);
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let result = refresh_token(&config, "previous", &ReqwestClient::new()).await;
        server.abort();
        let tokens = result.unwrap();
        assert_eq!(tokens.access_token, "renewed");
        assert!(tokens.refresh_token.is_none());
    }

    #[tokio::test]
    async fn query_code_exchange_and_form_refresh_are_distinct_and_errors_redact_secrets() {
        use axum::{body::Bytes, extract::OriginalUri};
        let router = Router::new().route(
            "/token",
            post(|OriginalUri(uri): OriginalUri, body: Bytes| async move {
                let form: HashMap<String, String> =
                    url::form_urlencoded::parse(if let Some(query) = uri.query() {
                        query.as_bytes()
                    } else {
                        &body
                    })
                    .into_owned()
                    .collect();
                assert_eq!(form["client_secret"], "query-secret-canary");
                if form["grant_type"] == "authorization_code" {
                    assert!(uri.query().is_some());
                    assert!(body.is_empty());
                    assert_eq!(form["code"], "code-canary");
                } else {
                    assert!(uri.query().is_none());
                    assert_eq!(form["refresh_token"], "refresh-canary");
                }
                Json(serde_json::json!({"access_token":"fixture","refresh_token":"refreshed"}))
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = oauth2_config(format!("http://{}/token", listener.local_addr().unwrap()));
        config.client_secret = Some("query-secret-canary".into());
        config.token_request_format = McpTokenRequestFormat::Query;
        config.refresh_token_request_format = Some(McpTokenRequestFormat::Form);
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        assert!(
            exchange_code(&config, "code-canary", "verifier", &ReqwestClient::new())
                .await
                .is_ok()
        );
        assert!(
            refresh_token(&config, "refresh-canary", &ReqwestClient::new())
                .await
                .is_ok()
        );
        server.abort();
        let _ = server.await;
        let error = exchange_code(&config, "code-canary", "verifier", &ReqwestClient::new())
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.contains("query-secret-canary") && !error.contains("code-canary"));
        config.token_url = "http://public.example/token".into();
        assert!(validate_token_auth(&config).is_err());
    }

    #[tokio::test]
    async fn json_token_requests_use_configured_basic_auth_and_nested_response_paths() {
        let router = Router::new().route(
            "/token",
            post(|headers: HeaderMap, Json(body): Json<Value>| async move {
                assert!(
                    headers
                        .get("authorization")
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .starts_with("Basic ")
                );
                assert_eq!(body["client_secret"], Value::Null);
                if body["grant_type"] == "authorization_code" {
                    assert_eq!(
                        body["redirect_uri"],
                        "https://app.example.com/mcp/oauth/callback"
                    );
                    Json(serde_json::json!({"nested":{"token":"user-token"}}))
                } else {
                    assert_eq!(body["grant_type"], "refresh_token");
                    Json(serde_json::json!({"access_token":"refreshed-user-token"}))
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = oauth2_config(format!("http://{}/token", listener.local_addr().unwrap()));
        config.client_secret = Some("fixture".into());
        config.token_endpoint_auth_method = Some(McpTokenEndpointAuthMethod::ClientSecretBasic);
        config.token_request_format = McpTokenRequestFormat::Json;
        config.token_response = Some(
            serde_json::from_value(serde_json::json!({"access_token":"/nested/token"})).unwrap(),
        );
        config.refresh_token_response = Some(
            serde_json::from_value(serde_json::json!({"access_token":"/access_token"})).unwrap(),
        );
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let result = exchange_code(&config, "code", "verifier", &ReqwestClient::new()).await;
        let refreshed = refresh_token(&config, "refresh-fixture", &ReqwestClient::new()).await;
        server.abort();
        assert_eq!(result.unwrap().access_token, "user-token");
        assert_eq!(refreshed.unwrap().access_token, "refreshed-user-token");
    }

    #[test]
    fn api_key_plugins_require_the_same_per_user_token_lookup_as_oauth_plugins() {
        let mut manifest: Value =
            serde_json::from_str(include_str!("../../connector-server-plugin/example.json"))
                .unwrap();
        for (kind, expected) in [("api_key", true), ("none", false)] {
            manifest["auth"] = serde_json::json!(kind);
            let client = McpServerClient::new(
                Uuid::new_v4(),
                "fixture".into(),
                McpTransportType::Http,
                manifest["base_url"].as_str().map(str::to_owned),
                serde_json::json!({"connector_plugin":{"manifest":manifest}}),
                None,
            );
            assert_eq!(client.requires_user_credentials(), expected);
        }
    }

    #[tokio::test]
    async fn oversized_token_endpoint_bodies_are_rejected_without_exposing_the_body() {
        let router = Router::new().route(
            "/token",
            post(|| async { Json(serde_json::json!({"access_token":"secret".repeat(12000)})) }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = oauth2_config(format!("http://{}/token", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let result = exchange_code(&config, "code", "verifier", &ReqwestClient::new())
            .await
            .unwrap_err();
        server.abort();
        assert!(matches!(result, McpClientError::OAuth(_)));
        assert!(!result.to_string().contains("secret"));
    }

    #[tokio::test]
    async fn token_endpoint_that_never_answers_is_reported_as_unreachable() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);

        let result = exchange_code(
            &oauth2_config(format!("http://{address}/token")),
            "code",
            "verifier",
            &ReqwestClient::new(),
        )
        .await;

        assert!(matches!(result, Err(McpClientError::OAuthUnreachable(_))));
    }

    #[tokio::test]
    async fn token_endpoint_rejecting_the_code_is_not_reported_as_unreachable() {
        let token_url =
            token_endpoint_replying("400 Bad Request", r#"{"error":"invalid_grant"}"#).await;

        let result = exchange_code(
            &oauth2_config(token_url),
            "used-code",
            "verifier",
            &ReqwestClient::new(),
        )
        .await;

        assert!(matches!(result, Err(McpClientError::OAuth(_))));
    }

    #[tokio::test]
    async fn token_endpoint_returning_tokens_redeems_the_code() {
        let token_url = token_endpoint_replying(
            "200 OK",
            r#"{"access_token":"at","token_type":"Bearer","expires_in":3600,"refresh_token":"rt"}"#,
        )
        .await;

        let tokens = exchange_code(
            &oauth2_config(token_url),
            "code",
            "verifier",
            &ReqwestClient::new(),
        )
        .await
        .expect("tokens");

        assert_eq!(tokens.access_token, "at");
        assert_eq!(tokens.refresh_token.as_deref(), Some("rt"));
    }

    #[tokio::test]
    async fn code_exchange_and_refresh_follow_the_configured_client_auth_method() {
        let captured = Arc::new(StdMutex::new(Vec::new()));
        let received = captured.clone();
        let router = Router::new().route(
            "/token",
            post(move |headers: HeaderMap, body: String| {
                let received = received.clone();
                async move {
                    received.lock().unwrap().push((headers, body));
                    Json(serde_json::json!({"access_token":"at", "refresh_token":"rt", "token_type":"Bearer"}))
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/token", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = ReqwestClient::new();
        let cases = [
            (McpOAuthFlow::Oauth2, None, false, false),
            (
                McpOAuthFlow::Oauth2,
                Some(McpTokenEndpointAuthMethod::ClientSecretPost),
                false,
                false,
            ),
            (
                McpOAuthFlow::Oauth2,
                Some(McpTokenEndpointAuthMethod::ClientSecretBasic),
                true,
                false,
            ),
            (
                McpOAuthFlow::Oauth2,
                Some(McpTokenEndpointAuthMethod::None),
                false,
                true,
            ),
            (McpOAuthFlow::Oauth2, None, false, true),
            (McpOAuthFlow::Oidc, None, true, false),
            (
                McpOAuthFlow::Oidc,
                Some(McpTokenEndpointAuthMethod::ClientSecretPost),
                false,
                false,
            ),
            (
                McpOAuthFlow::Oidc,
                Some(McpTokenEndpointAuthMethod::ClientSecretBasic),
                true,
                false,
            ),
            (
                McpOAuthFlow::Oidc,
                Some(McpTokenEndpointAuthMethod::None),
                false,
                true,
            ),
        ];
        for (flow, method, basic, public) in cases {
            let mut config = oauth2_config(url.clone());
            config.flow = flow;
            config.client_id = "client: id".to_string();
            config.client_secret = (!public).then(|| "s+e&cret".to_string());
            config.token_endpoint_auth_method = method;
            exchange_code(&config, "code", "verifier", &client)
                .await
                .unwrap();
            refresh_token(&config, "refresh", &client).await.unwrap();
            let requests = std::mem::take(&mut *captured.lock().unwrap());
            assert_eq!(requests.len(), 2);
            for (index, (headers, body)) in requests.into_iter().enumerate() {
                let form: HashMap<_, _> = url::form_urlencoded::parse(body.as_bytes())
                    .into_owned()
                    .collect();
                assert_eq!(
                    form.get("grant_type").map(String::as_str),
                    Some(if index == 0 {
                        "authorization_code"
                    } else {
                        "refresh_token"
                    })
                );
                if basic {
                    let expected = format!(
                        "Basic {}",
                        base64::engine::general_purpose::STANDARD
                            .encode(b"client%3A+id:s%2Be%26cret")
                    );
                    assert_eq!(headers.get(AUTHORIZATION).unwrap(), expected.as_str());
                    assert!(!form.contains_key("client_secret"));
                } else {
                    assert!(!headers.contains_key(AUTHORIZATION));
                    assert_eq!(
                        form.get("client_id").map(String::as_str),
                        Some("client: id")
                    );
                    assert_eq!(
                        form.get("client_secret").map(String::as_str),
                        if public { None } else { Some("s+e&cret") }
                    );
                }
            }
        }
        server.abort();
    }

    #[test]
    fn invalid_client_auth_configuration_is_rejected() {
        let mut config = oauth2_config("https://example.test/token".to_string());
        config.token_endpoint_auth_method = Some(McpTokenEndpointAuthMethod::ClientSecretBasic);
        assert!(validate_token_auth(&config).is_err());
        config.client_secret = Some("secret".to_string());
        assert!(validate_token_auth(&config).is_ok());
        config.token_endpoint_auth_method = Some(McpTokenEndpointAuthMethod::None);
        assert!(validate_token_auth(&config).is_err());
    }

    #[tokio::test]
    async fn custom_headers_reach_initialize_listing_and_calls_with_the_user_token() {
        let captured = Arc::new(StdMutex::new(Vec::new()));
        let received = captured.clone();
        let router = Router::new().route(
            "/mcp",
            post(move |headers: HeaderMap, Json(message): Json<Value>| {
                let received = received.clone();
                async move {
                    received.lock().unwrap().push((message["method"].clone(), headers));
                    if message.get("id").is_none() {
                        return StatusCode::ACCEPTED.into_response();
                    }
                    let result = match message["method"].as_str().unwrap() {
                        "initialize" => serde_json::json!({
                            "protocolVersion":message["params"]["protocolVersion"],
                            "capabilities":{"tools":{}}, "serverInfo":{"name":"test", "version":"1"}
                        }),
                        "tools/list" => serde_json::json!({"tools":[{"name":"ping", "description":"Ping", "inputSchema":{"type":"object"}}]}),
                        "tools/call" => serde_json::json!({"content":[{"type":"text", "text":"pong"}]}),
                        _ => panic!("unexpected method"),
                    };
                    Json(serde_json::json!({"jsonrpc":"2.0", "id":message["id"], "result":result})).into_response()
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let config = serde_json::json!({"transport_config":{"headers":{
            "User-Agent":"partner_app_Grengin", "Authorization":"Bearer wrong-token"
        }}});
        let service = connect_rmcp_http(&url, Some("per-user-token".to_string()), &config)
            .await
            .unwrap();
        assert_eq!(service.list_all_tools().await.unwrap().len(), 1);
        service
            .call_tool(CallToolRequestParams::new("ping"))
            .await
            .unwrap();
        for method in ["initialize", "tools/list", "tools/call"] {
            let requests = captured.lock().unwrap();
            let (_, headers) = requests.iter().find(|(name, _)| name == method).unwrap();
            assert_eq!(headers["user-agent"], "partner_app_Grengin");
            assert_eq!(headers[AUTHORIZATION], "Bearer per-user-token");
        }
        service.cancel().await.unwrap();
        server.abort();
    }

    #[test]
    fn header_placeholders_cannot_read_the_api_secrets() {
        for value in [
            "{{APP_KEY}}",
            "Bearer ${JWT_SECRET}",
            "$DATABASE_URL",
            "{{MCP_SECRET_}}",
        ] {
            let error = resolve_env_placeholders(value).unwrap_err();
            assert!(error.to_string().contains("not allowed"), "{value}");
        }
        assert!(
            mcp_http_client(
                &serde_json::json!({"transport_config":{"headers":{"X-Key":"{{APP_KEY}}"}}})
            )
            .is_err()
        );
        assert!(
            extract_auth_header(
                &serde_json::json!({"transport_config":{"headers":{"Authorization":"Bearer {{JWT_SECRET}}"}}})
            )
            .is_err()
        );
        let error = resolve_env_placeholders("{{MCP_SECRET_GRENGIN_TEST_UNSET}}").unwrap_err();
        assert!(error.to_string().contains("missing environment variable"));
        assert_eq!(
            resolve_env_placeholders("plain value").unwrap(),
            "plain value"
        );
    }

    #[test]
    fn stdio_commands_must_be_on_the_operator_allowlist() {
        assert!(stdio_command_allowed("sqlx-mcp", DEFAULT_STDIO_COMMANDS));
        assert!(stdio_command_allowed(
            "/usr/local/bin/sqlx-mcp",
            DEFAULT_STDIO_COMMANDS
        ));
        for command in [
            "sh",
            "bash",
            "/bin/sh",
            "echo",
            "sqlx-mcp-evil",
            "",
            " sqlx-mcp",
        ] {
            assert!(
                !stdio_command_allowed(command, DEFAULT_STDIO_COMMANDS),
                "{command}"
            );
        }
        assert!(stdio_command_allowed("npx", "sqlx-mcp, npx"));
        assert!(!stdio_command_allowed("npx", "sqlx-mcp,,"));
    }

    #[test]
    fn stdio_children_receive_only_basic_and_mcp_secret_variables() {
        let names: Vec<String> = stdio_child_env()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert!(names.iter().all(|name| STDIO_INHERITED_ENV.contains(&name.as_str())
            || is_mcp_secret_env(name)));
        for secret in ["APP_KEY", "JWT_SECRET", "DATABASE_URL"] {
            assert!(!names.iter().any(|name| name == secret));
        }
    }

    #[test]
    fn invalid_custom_headers_are_configuration_errors() {
        for headers in [
            serde_json::json!({"Bad\nName":"value"}),
            serde_json::json!({"User-Agent":"bad\r\nvalue"}),
        ] {
            assert!(
                mcp_http_client(&serde_json::json!({"transport_config":{"headers":headers}}))
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn slow_oauth_user_does_not_block_other_users_on_same_mcp_server() {
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let router = Router::new().route(
            "/mcp",
            post({
                let entered = entered.clone();
                let release = release.clone();
                move |headers: HeaderMap, Json(message): Json<Value>| {
                    let entered = entered.clone();
                    let release = release.clone();
                    async move {
                        if message.get("id").is_none() {
                            return StatusCode::ACCEPTED.into_response();
                        }
                        let result = match message["method"].as_str().unwrap() {
                            "initialize" => serde_json::json!({
                                "protocolVersion":message["params"]["protocolVersion"],
                                "capabilities":{"tools":{}}, "serverInfo":{"name":"concurrency", "version":"1"}
                            }),
                            "tools/list" => serde_json::json!({"tools":[]}),
                            "tools/call" => {
                                if headers.get(AUTHORIZATION).unwrap() == "Bearer slow-user" {
                                    entered.notify_one();
                                    release.notified().await;
                                }
                                serde_json::json!({"content":[{"type":"text","text":"ok"}]})
                            }
                            _ => panic!("unexpected MCP method"),
                        };
                        Json(serde_json::json!({"jsonrpc":"2.0", "id":message["id"], "result":result})).into_response()
                    }
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = Arc::new(McpServerClient::new(
            Uuid::new_v4(),
            "concurrency".into(),
            McpTransportType::Http,
            Some(url),
            serde_json::json!({}),
            None,
        ));
        let slow = tokio::spawn({
            let client = client.clone();
            async move {
                client
                    .call_tool(
                        "ping",
                        serde_json::json!({}),
                        Some("slow-user".into()),
                        Some(Uuid::new_v4()),
                    )
                    .await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        let fast_user = Uuid::new_v4();
        let fast = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            client
                .list_tools_with_auth(Some("fast-user".into()), Some(fast_user))
                .await?;
            client
                .call_tool(
                    "ping",
                    serde_json::json!({}),
                    Some("rotated-fast-user".into()),
                    Some(fast_user),
                )
                .await
        })
        .await;
        release.notify_one();
        slow.await.unwrap().unwrap();
        server.abort();
        assert!(
            fast.is_ok(),
            "another user's network call held the entire client-cache lock"
        );
        fast.unwrap().unwrap();
    }

    #[tokio::test]
    async fn closed_invalidated_and_rotated_sessions_reconnect_without_replaying_calls() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let initialized = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let router = Router::new().route("/mcp", post({
            let initialized = initialized.clone();
            let calls = calls.clone();
            move |headers: HeaderMap, Json(message): Json<Value>| {
                let initialized = initialized.clone();
                let calls = calls.clone();
                async move {
                    if message.get("id").is_none() {
                        return StatusCode::ACCEPTED.into_response();
                    }
                    let result = match message["method"].as_str().unwrap() {
                        "initialize" => {
                            initialized.fetch_add(1, Ordering::SeqCst);
                            serde_json::json!({"protocolVersion":message["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"recovery","version":"1"}})
                        }
                        "tools/call" => {
                            calls.fetch_add(1, Ordering::SeqCst);
                            serde_json::json!({"content":[],"structuredContent":{"authorization":headers[AUTHORIZATION].to_str().unwrap()}})
                        }
                        _ => return StatusCode::BAD_REQUEST.into_response(),
                    };
                    Json(serde_json::json!({"jsonrpc":"2.0","id":message["id"],"result":result})).into_response()
                }
            }
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = McpServerClient::new(
            Uuid::new_v4(),
            "recovery".into(),
            McpTransportType::Http,
            Some(url),
            serde_json::json!({}),
            None,
        );
        let user = Uuid::new_v4();
        for (index, token) in ["first", "first", "first", "rotated"].iter().enumerate() {
            if index == 1 {
                let lease = client
                    .http_clients
                    .as_ref()
                    .unwrap()
                    .lease(user)
                    .await
                    .unwrap();
                lease.as_ref().unwrap().client.cancellation_token().cancel();
            } else if index == 2 {
                client.invalidate_user_session(user).await;
            }
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                client.call_tool(
                    "ping",
                    serde_json::json!({}),
                    Some((*token).into()),
                    Some(user),
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(
                result.structured_content.unwrap()["authorization"],
                format!("Bearer {token}")
            );
            assert_eq!(initialized.load(Ordering::SeqCst), index + 1);
            assert_eq!(calls.load(Ordering::SeqCst), index + 1);
        }
        server.abort();
    }

    #[tokio::test]
    async fn oauth_client_cache_policy_is_configurable_and_rejects_invalid_limits() {
        let client = McpServerClient::new(
            Uuid::new_v4(),
            "capacity".into(),
            McpTransportType::Http,
            Some("https://example.invalid/mcp".into()),
            serde_json::json!({"transport_config":{"session_cache":{"max_entries":2}}}),
            None,
        );
        let cache = client.http_clients.as_ref().unwrap();
        for _ in 0..512 {
            drop(cache.lease(Uuid::new_v4()).await.unwrap());
        }
        assert_eq!(cache.cached_len().await, 2);
        assert!(
            transport_config(
                &serde_json::json!({"transport_config":{"session_cache":{"max_in_flight":0}}})
            )
            .is_err()
        );
        assert!(transport_config(&serde_json::json!({"transport_config":{"session_cache":{"max_entries":"unlimited"}}})).is_err());
    }
}
