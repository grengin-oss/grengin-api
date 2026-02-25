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
use reqwest::Client as ReqwestClient;
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

fn build_oauth_client(config: &McpOAuthConfig) -> Result<OAuthClient, McpClientError> {
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

pub fn build_authorization_url(
    config: &McpOAuthConfig,
) -> Result<McpOAuthAuthorization, McpClientError> {
    let client = build_oauth_client(config)?;
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

    let (auth_url, csrf_token, _nonce) = request.set_pkce_challenge(pkce_challenge).url();

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
    let client = build_oauth_client(config)?;
    let token_response = client
        .exchange_code(AuthorizationCode::new(code.to_string()))
        .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier.to_string()))
        .request_async(http_client)
        .await
        .map_err(|e| McpClientError::OAuth(e.to_string()))?;
    Ok(tokens_from_response(&token_response))
}

pub async fn refresh_token(
    config: &McpOAuthConfig,
    refresh_token: &str,
    http_client: &ReqwestClient,
) -> Result<McpOAuthTokens, McpClientError> {
    let client = build_oauth_client(config)?;
    let token_response = client
        .exchange_refresh_token(&RefreshToken::new(refresh_token.to_string()))
        .request_async(http_client)
        .await
        .map_err(|e| McpClientError::OAuth(e.to_string()))?;
    Ok(tokens_from_response(&token_response))
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
            connected: Mutex::new(false),
        }
    }

    pub async fn ensure_connected(&self) -> Result<(), McpClientError> {
        let mut connected = self.connected.lock().await;
        if *connected {
            return Ok(());
        }

        match self.transport_type {
            McpTransportType::Http => {
                let base_url = self
                    .url
                    .as_deref()
                    .ok_or_else(|| McpClientError::Config("mcp http url missing".to_string()))?;
                if self.connection_config.get("sse_url").is_some() {
                    eprintln!("rmcp streamable http ignores sse_url; using base_url only");
                }
                let auth_header = extract_auth_header(&self.connection_config);
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
    ) -> Result<CallToolResult, McpClientError> {
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

fn extract_auth_header(connection_config: &Value) -> Option<String> {
    let config_value = connection_config.get("transport_config")?;
    let input: TransportConfigInput = serde_json::from_value(config_value.clone()).ok()?;
    let headers = input.headers?;
    let header_value = headers
        .get("Authorization")
        .or_else(|| headers.get("authorization"))?;
    let token = header_value.strip_prefix("Bearer ").unwrap_or(header_value);
    Some(token.to_string())
}
