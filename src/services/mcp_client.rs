// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::models::mcp_servers::McpTransportType;
use crate::services::http_client::HttpTimeouts;
use chrono::{DateTime, Duration, Utc};
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
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::process::Command;
use tokio::sync::{Mutex, OwnedMutexGuard};
use uuid::Uuid;

const MAX_USER_HTTP_CLIENTS: usize = 256;

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTokenEndpointAuthMethod {
    ClientSecretPost,
    ClientSecretBasic,
    None,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpOAuthFlow {
    Oauth2,
    Oidc,
}

impl Default for McpOAuthFlow {
    fn default() -> Self {
        McpOAuthFlow::Oauth2
    }
}

fn default_use_pkce() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize)]
struct McpOAuthConfigInput {
    pub issuer_url: Option<String>,
    pub auth_url: String,
    pub token_url: String,
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

fn parse_oauth2_token_response(body: &str) -> Result<McpOAuthTokens, McpClientError> {
    let payload: OAuth2TokenResponsePayload = serde_json::from_str(body)
        .map_err(|e| McpClientError::OAuth(format!("Failed to parse token response: {e}")))?;

    let expires_at = payload
        .expires_in
        .as_ref()
        .and_then(ExpiresIn::as_seconds)
        .and_then(|seconds| Duration::from_std(std::time::Duration::from_secs(seconds as u64)).ok())
        .map(|delta| Utc::now() + delta);

    let scopes = match payload.scope {
        Some(ScopeField::Text(text)) => text
            .split_whitespace()
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
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        let error = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown_error");
        let description = value
            .get("error_description")
            .and_then(Value::as_str)
            .unwrap_or("no description");
        return format!("{error}: {description}");
    }
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "empty response body".to_string();
    }
    let snippet: String = trimmed.chars().take(200).collect();
    if trimmed.len() > 200 {
        format!("{snippet}...")
    } else {
        snippet
    }
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
                    McpClientError::OAuthUnreachable(e.to_string())
                } else {
                    McpClientError::OAuth(e.to_string())
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
                .map_err(|e| McpClientError::OAuthUnreachable(e.to_string()))?;
            let status = response.status();
            let body = response
                .text()
                .await
                .map_err(|e| McpClientError::OAuth(e.to_string()))?;
            if !status.is_success() {
                return Err(McpClientError::OAuth(format!(
                    "token endpoint {status}: {}",
                    summarize_oauth_error(&body)
                )));
            }
            parse_oauth2_token_response(&body)
        }
    }
}

pub async fn refresh_token(
    config: &McpOAuthConfig,
    refresh_token: &str,
    http_client: &ReqwestClient,
) -> Result<McpOAuthTokens, McpClientError> {
    match config.flow {
        McpOAuthFlow::Oidc => {
            let client = build_oidc_client(config)?;
            let token_response = client
                .exchange_refresh_token(&RefreshToken::new(refresh_token.to_string()))
                .request_async(http_client)
                .await
                .map_err(|e| McpClientError::OAuth(e.to_string()))?;
            Ok(tokens_from_response(&token_response))
        }
        McpOAuthFlow::Oauth2 => {
            let form = vec![
                ("grant_type".to_string(), "refresh_token".to_string()),
                ("refresh_token".to_string(), refresh_token.to_string()),
            ];
            let response = oauth2_token_request(config, http_client, form)?
                .send()
                .await
                .map_err(|e| McpClientError::OAuth(e.to_string()))?;
            let status = response.status();
            let body = response
                .text()
                .await
                .map_err(|e| McpClientError::OAuth(e.to_string()))?;
            if !status.is_success() {
                return Err(McpClientError::OAuth(format!(
                    "token endpoint {status}: {}",
                    summarize_oauth_error(&body)
                )));
            }
            parse_oauth2_token_response(&body)
        }
    }
}

fn validate_token_auth(config: &McpOAuthConfig) -> Result<(), McpClientError> {
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
        }
        method => {
            form.push(("client_id".to_string(), config.client_id.clone()));
            if method == McpTokenEndpointAuthMethod::ClientSecretPost {
                if let Some(secret) = config.client_secret.as_ref() {
                    form.push(("client_secret".to_string(), secret.clone()));
                }
            }
        }
    }
    Ok(request.form(&form))
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
        .map_err(|e| McpClientError::OAuthConfig(format!("invalid oauth config: {e}")))?;
    let issuer_url = input
        .issuer_url
        .clone()
        .unwrap_or_else(|| input.auth_url.clone());
    let config = McpOAuthConfig {
        issuer_url,
        auth_url: input.auth_url,
        token_url: input.token_url,
        redirect_url: input.redirect_url,
        client_id: client_id.to_string(),
        client_secret,
        scopes: input.scopes,
        extra_params: input.extra_params,
        flow: input.flow,
        use_pkce: input.use_pkce.unwrap_or(true),
        token_endpoint_auth_method: input.token_endpoint_auth_method,
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

async fn connect_rmcp_stdio(
    command: &str,
    args: &[String],
) -> Result<rmcp::service::RunningService<rmcp::service::RoleClient, ()>, McpClientError> {
    let mut cmd = Command::new(command);
    cmd.args(args);
    let transport = TokioChildProcess::new(cmd)
        .map_err(|e| McpClientError::Rmcp(format!("spawn stdio process failed: {e}")))?;
    ().serve(transport)
        .await
        .map_err(|e| McpClientError::Rmcp(format!("rmcp connect failed: {e}")))
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
    ().serve(transport)
        .await
        .map_err(|e| McpClientError::Rmcp(format!("rmcp connect failed: {e}")))
}

fn mcp_http_client(connection_config: &Value) -> Result<ReqwestClient, McpClientError> {
    let config: TransportConfigInput = serde_json::from_value(
        connection_config
            .get("transport_config")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
    )
    .map_err(|e| McpClientError::Config(format!("invalid transport_config: {e}")))?;
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
    HttpTimeouts::LONG_LIVED_STREAM
        .apply(ReqwestClient::builder())
        .redirect(reqwest::redirect::Policy::none())
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
    http_clients: Mutex<HashMap<Uuid, Arc<Mutex<Option<UserHttpClient>>>>>,
    connected: Mutex<bool>,
}

struct UserHttpClient {
    token: String,
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
}

impl McpServerClient {
    pub fn new(
        server_id: Uuid,
        name: String,
        transport_type: McpTransportType,
        url: Option<String>,
        connection_config: Value,
        db_url: Option<String>,
    ) -> Self {
        Self {
            server_id,
            name,
            transport_type,
            url,
            connection_config,
            db_url,
            client: Mutex::new(None),
            http_clients: Mutex::new(HashMap::new()),
            connected: Mutex::new(false),
        }
    }

    pub async fn ensure_connected(&self) -> Result<(), McpClientError> {
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

    async fn http_client_slot(
        &self,
        user_id: Uuid,
    ) -> Result<Arc<Mutex<Option<UserHttpClient>>>, McpClientError> {
        let mut clients = self.http_clients.lock().await;
        if let Some(entry) = clients.get(&user_id) {
            return Ok(entry.clone());
        }
        if clients.len() >= MAX_USER_HTTP_CLIENTS {
            let idle = clients
                .iter()
                .find_map(|(id, entry)| (Arc::strong_count(entry) == 1).then_some(*id));
            let Some(idle) = idle else {
                return Err(McpClientError::Rmcp(
                    "mcp client capacity reached; retry later".into(),
                ));
            };
            clients.remove(&idle);
        }
        let entry = Arc::new(Mutex::new(None));
        clients.insert(user_id, entry.clone());
        Ok(entry)
    }

    async fn authenticated_http_client(
        &self,
        token: String,
        user_id: Uuid,
    ) -> Result<OwnedMutexGuard<Option<UserHttpClient>>, McpClientError> {
        let mut entry = self.http_client_slot(user_id).await?.lock_owned().await;
        if entry.as_ref().is_none_or(|client| client.token != token) {
            let service = connect_rmcp_http(
                self.http_url_for_transport()?,
                Some(token.clone()),
                &self.connection_config,
            )
            .await?;
            *entry = Some(UserHttpClient {
                token,
                client: service,
            });
        }
        Ok(entry)
    }

    pub async fn call_tool(
        &self,
        tool_name: &str,
        args: serde_json::Value,
        auth_token: Option<String>,
        user_id: Option<Uuid>,
    ) -> Result<CallToolResult, McpClientError> {
        match (self.transport_type, auth_token, user_id) {
            (McpTransportType::Http | McpTransportType::Sse, Some(token), Some(user_id)) => {
                let client = self.authenticated_http_client(token, user_id).await?;
                let entry = client.as_ref().ok_or_else(|| {
                    McpClientError::Rmcp("rmcp client not initialized".to_string())
                })?;
                let arguments = rmcp_args_from_value(args)?;
                entry
                    .client
                    .call_tool(CallToolRequestParams {
                        meta: None,
                        name: tool_name.to_string().into(),
                        arguments,
                        task: None,
                    })
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
                    .call_tool(CallToolRequestParams {
                        meta: None,
                        name: tool_name.to_string().into(),
                        arguments,
                        task: None,
                    })
                    .await
                    .map_err(|e| McpClientError::Rmcp(e.to_string()))
            }
        }
    }

    pub async fn list_tools(&self) -> Result<Vec<Tool>, McpClientError> {
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
                while let Some(next) = chars.next() {
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

fn resolve_env_var(name: &str) -> Result<String, McpClientError> {
    std::env::var(name)
        .map_err(|_| McpClientError::Config(format!("missing environment variable: {name}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
    use base64::Engine;
    use std::sync::Mutex as StdMutex;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn oauth2_config(token_url: String) -> McpOAuthConfig {
        McpOAuthConfig {
            issuer_url: "https://issuer.example.com".to_string(),
            auth_url: "https://issuer.example.com/authorize".to_string(),
            token_url,
            client_id: "client".to_string(),
            client_secret: None,
            redirect_url: "https://app.example.com/mcp/oauth/callback".to_string(),
            scopes: vec![],
            extra_params: HashMap::new(),
            flow: McpOAuthFlow::Oauth2,
            use_pkce: true,
            token_endpoint_auth_method: None,
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
            .call_tool(CallToolRequestParams {
                meta: None,
                name: "ping".into(),
                arguments: None,
                task: None,
            })
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
    async fn oauth_client_cache_is_bounded_and_never_evicts_an_in_use_entry() {
        let client = McpServerClient::new(
            Uuid::new_v4(),
            "capacity".into(),
            McpTransportType::Http,
            Some("https://example.invalid/mcp".into()),
            serde_json::json!({}),
            None,
        );
        let first_user = Uuid::new_v4();
        let first = client.http_client_slot(first_user).await.unwrap();
        assert!(Arc::ptr_eq(
            &first,
            &client.http_client_slot(first_user).await.unwrap()
        ));
        let mut active = vec![first];
        for _ in 1..MAX_USER_HTTP_CLIENTS {
            active.push(client.http_client_slot(Uuid::new_v4()).await.unwrap());
        }
        assert!(client.http_client_slot(Uuid::new_v4()).await.is_err());
        active.pop();
        let replacement = client.http_client_slot(Uuid::new_v4()).await.unwrap();
        assert_eq!(
            client.http_clients.lock().await.len(),
            MAX_USER_HTTP_CLIENTS
        );
        assert!(Arc::ptr_eq(
            &active[0],
            &client.http_client_slot(first_user).await.unwrap()
        ));
        drop(replacement);
    }
}
