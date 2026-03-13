use chrono::{DateTime, Duration, Utc};
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, CallToolResult, JsonObject, Tool},
    transport::{
        StreamableHttpClientTransport, TokioChildProcess,
    },
};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use openidconnect::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet, EndpointSet,
    IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl,
    RefreshToken, Scope, TokenUrl,
    core::{CoreAuthenticationFlow, CoreClient, CoreJsonWebKeySet},
};
use oauth2::{
    basic::BasicClient,
    EndpointNotSet as OAuthEndpointNotSet,
    EndpointSet as OAuthEndpointSet,
};
use reqwest::{Client as ReqwestClient, header::ACCEPT};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::process::Command;
use tokio::sync::Mutex;
use uuid::Uuid;
use crate::models::mcp_servers::McpTransportType;
use thiserror::Error;
use serde_json::Value;


#[derive(Debug, Error)]
pub enum McpClientError {
    #[error("rmcp client error: {0}")]
    Rmcp(String),
    #[error("oauth error: {0}")]
    OAuth(String),
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
    let issuer = IssuerUrl::new(config.issuer_url.clone())
        .map_err(|e| McpClientError::OAuthConfig(e.to_string()))?;
    let auth_url = AuthUrl::new(config.auth_url.clone())
        .map_err(|e| McpClientError::OAuthConfig(e.to_string()))?;
    let token_url = TokenUrl::new(config.token_url.clone())
        .map_err(|e| McpClientError::OAuthConfig(e.to_string()))?;
    let redirect_url = RedirectUrl::new(config.redirect_url.clone())
        .map_err(|e| McpClientError::OAuthConfig(e.to_string()))?;

    let client = CoreClient::new(
        ClientId::new(config.client_id.clone()),
        issuer,
        CoreJsonWebKeySet::new(Vec::new()),
    )
    .set_auth_uri(auth_url)
    .set_token_uri(token_url)
    .set_redirect_uri(redirect_url);

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

fn tokens_from_response(
    token_response: &openidconnect::core::CoreTokenResponse,
) -> McpOAuthTokens {
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
    let payload: OAuth2TokenResponsePayload = serde_json::from_str(body).map_err(|e| {
        McpClientError::OAuth(format!("Failed to parse token response: {e}"))
    })?;

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
                request = request.set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier.to_string()));
            }
            let token_response = request
                .request_async(http_client)
                .await
                .map_err(|e| McpClientError::OAuth(e.to_string()))?;
            Ok(tokens_from_response(&token_response))
        }
        McpOAuthFlow::Oauth2 => {
            let mut form = vec![
                ("grant_type".to_string(), "authorization_code".to_string()),
                ("code".to_string(), code.to_string()),
                ("redirect_uri".to_string(), config.redirect_url.clone()),
                ("client_id".to_string(), config.client_id.clone()),
            ];
            if let Some(secret) = config.client_secret.as_ref() {
                form.push(("client_secret".to_string(), secret.clone()));
            }
            if config.use_pkce {
                form.push(("code_verifier".to_string(), pkce_verifier.to_string()));
            }
            let response = http_client
                .post(&config.token_url)
                .header(ACCEPT, "application/json")
                .form(&form)
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
            let mut form = vec![
                ("grant_type".to_string(), "refresh_token".to_string()),
                ("refresh_token".to_string(), refresh_token.to_string()),
                ("client_id".to_string(), config.client_id.clone()),
            ];
            if let Some(secret) = config.client_secret.as_ref() {
                form.push(("client_secret".to_string(), secret.clone()));
            }
            let response = http_client
                .post(&config.token_url)
                .header(ACCEPT, "application/json")
                .form(&form)
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
    Ok(McpOAuthConfig {
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
    })
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
) -> Result<rmcp::service::RunningService<rmcp::service::RoleClient, ()>, McpClientError> {
    let mut config = StreamableHttpClientTransportConfig::with_uri(base_url);
    if let Some(token) = auth_header {
        config = config.auth_header(token);
    }
    let transport = StreamableHttpClientTransport::from_config(config);
    ().serve(transport)
        .await
        .map_err(|e| McpClientError::Rmcp(format!("rmcp connect failed: {e}")))
}

pub struct McpServerClient {
    pub server_id: Uuid,
    pub name: String,
    pub transport_type: McpTransportType,
    pub url: Option<String>,
    pub connection_config: Value,
    pub db_url: Option<String>,
    client: Mutex<Option<rmcp::service::RunningService<rmcp::service::RoleClient, ()>>>,
    http_clients: Mutex<HashMap<String, rmcp::service::RunningService<rmcp::service::RoleClient, ()>>>,
    connected: Mutex<bool>,
}

#[derive(Debug, Clone, Deserialize)]
struct StdioConnectionConfig {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    transport_config: Option<TransportConfigInput>,
}

#[derive(Debug, Clone, Deserialize)]
struct TransportConfigInput {
    connect_timeout_ms: Option<u64>,
    read_timeout_ms: Option<u64>,
    write_timeout_ms: Option<u64>,
    max_message_size: Option<usize>,
    keep_alive_ms: Option<u64>,
    compression: Option<bool>,
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
                let service = connect_rmcp_http(base_url, auth_header).await?;
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

    pub async fn call_tool(
        &self,
        tool_name: &str,
        args: serde_json::Value,
        auth_token: Option<String>,
    ) -> Result<CallToolResult, McpClientError> {
        match (self.transport_type, auth_token) {
            (McpTransportType::Http | McpTransportType::Sse, Some(token)) => {
                let base_url = self.http_url_for_transport()?;
                let mut clients = self.http_clients.lock().await;
                if !clients.contains_key(&token) {
                    let service = connect_rmcp_http(base_url, Some(token.clone())).await?;
                    clients.insert(token.clone(), service);
                }
                let service = clients
                    .get_mut(&token)
                    .ok_or_else(|| McpClientError::Rmcp("rmcp client not initialized".to_string()))?;
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
            _ => {
                self.ensure_connected().await?;
                let mut client = self.client.lock().await;
                let service = client
                    .as_mut()
                    .ok_or_else(|| McpClientError::Rmcp("rmcp client not initialized".to_string()))?;
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
    ) -> Result<Vec<Tool>, McpClientError> {
        match (self.transport_type, auth_token) {
            (McpTransportType::Http | McpTransportType::Sse, Some(token)) => {
                let base_url = self.http_url_for_transport()?;
                let mut clients = self.http_clients.lock().await;
                if !clients.contains_key(&token) {
                    let service = connect_rmcp_http(base_url, Some(token.clone())).await?;
                    clients.insert(token.clone(), service);
                }
                let service = clients
                    .get_mut(&token)
                    .ok_or_else(|| McpClientError::Rmcp("rmcp client not initialized".to_string()))?;
                service
                    .list_all_tools()
                    .await
                    .map_err(|e| McpClientError::Rmcp(e.to_string()))
            }
            _ => self.list_tools().await,
        }
    }

    fn http_url_for_transport(&self) -> Result<&str, McpClientError> {
        match self.transport_type {
            McpTransportType::Http => self
                .url
                .as_deref()
                .ok_or_else(|| McpClientError::Config("mcp http url missing".to_string())),
            McpTransportType::Sse => {
                if let Some(sse_url) = self.connection_config.get("sse_url").and_then(Value::as_str) {
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
        let has_placeholder = config
            .args
            .iter()
            .any(|arg| arg.contains("{{db_url}}")
                || arg.contains("$DB_URL")
                || arg.contains("${DB_URL}"));
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
    let token = resolved.strip_prefix("Bearer ").unwrap_or(resolved.as_str());
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
    std::env::var(name).map_err(|_| {
        McpClientError::Config(format!("missing environment variable: {name}"))
    })
}
