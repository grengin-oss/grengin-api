use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use mcp_protocol_sdk::{
    McpError,
    client::McpClient as SdkMcpClient,
    protocol::{
        InitializeResult,
        messages::ListToolsResult,
        types::{CallToolResult, ToolInfo},
    },
    transport::{TransportConfig, http::HttpClientTransport, websocket::WebSocketClientTransport},
};
use openidconnect::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet, EndpointSet,
    IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl,
    RefreshToken, Scope, TokenUrl,
    core::{CoreAuthenticationFlow, CoreClient, CoreJsonWebKeySet},
};
use reqwest::Client as ReqwestClient;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum McpClientError {
    #[error("mcp client error: {0}")]
    Mcp(#[from] McpError),
    #[error("oauth error: {0}")]
    OAuth(String),
    #[error("invalid oauth configuration: {0}")]
    OAuthConfig(String),
    #[error("invalid tool args: {0}")]
    ToolArgs(String),
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

#[async_trait]
pub trait McpClient {
    async fn connect_http(
        &mut self,
        base_url: &str,
        sse_url: Option<&str>,
        config: Option<TransportConfig>,
    ) -> Result<InitializeResult, McpClientError>;

    async fn connect_websocket(
        &mut self,
        url: &str,
        config: Option<TransportConfig>,
    ) -> Result<InitializeResult, McpClientError>;

    async fn list_tools(&mut self) -> Result<Vec<ToolInfo>, McpClientError>;

    async fn call_tool(
        &mut self,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<CallToolResult, McpClientError>;

    fn build_authorization_url(
        &self,
        config: &McpOAuthConfig,
    ) -> Result<McpOAuthAuthorization, McpClientError>;

    async fn exchange_code(
        &self,
        config: &McpOAuthConfig,
        code: &str,
        pkce_verifier: &str,
        http_client: &ReqwestClient,
    ) -> Result<McpOAuthTokens, McpClientError>;

    async fn refresh_token(
        &self,
        config: &McpOAuthConfig,
        refresh_token: &str,
        http_client: &ReqwestClient,
    ) -> Result<McpOAuthTokens, McpClientError>;
}

pub struct McpSdkClient {
    client: SdkMcpClient,
}

type OAuthClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointSet,
    EndpointNotSet,
>;

impl McpSdkClient {
    pub fn new<S: Into<String>>(name: S, version: S) -> Self {
        Self {
            client: SdkMcpClient::new(name.into(), version.into()),
        }
    }

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
}

#[async_trait]
impl McpClient for McpSdkClient {
    async fn connect_http(
        &mut self,
        base_url: &str,
        sse_url: Option<&str>,
        config: Option<TransportConfig>,
    ) -> Result<InitializeResult, McpClientError> {
        let transport = if let Some(config) = config {
            HttpClientTransport::with_config(base_url, sse_url, config).await?
        } else {
            HttpClientTransport::new(base_url, sse_url).await?
        };
        Ok(self.client.connect(transport).await?)
    }

    async fn connect_websocket(
        &mut self,
        url: &str,
        config: Option<TransportConfig>,
    ) -> Result<InitializeResult, McpClientError> {
        let transport = if let Some(config) = config {
            WebSocketClientTransport::with_config(url, config).await?
        } else {
            WebSocketClientTransport::new(url).await?
        };
        Ok(self.client.connect(transport).await?)
    }

    async fn list_tools(&mut self) -> Result<Vec<ToolInfo>, McpClientError> {
        let ListToolsResult { tools, .. } = self.client.list_tools(None).await?;
        Ok(tools)
    }

    async fn call_tool(
        &mut self,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<CallToolResult, McpClientError> {
        let arguments = match args {
            serde_json::Value::Object(map) => Some(
                map.into_iter()
                    .collect::<HashMap<String, serde_json::Value>>(),
            ),
            serde_json::Value::Null => None,
            _ => {
                return Err(McpClientError::ToolArgs(
                    "tool arguments must be a JSON object".to_string(),
                ));
            }
        };
        let result = self
            .client
            .call_tool(tool_name.to_string(), arguments)
            .await?;
        Ok(result)
    }

    fn build_authorization_url(
        &self,
        config: &McpOAuthConfig,
    ) -> Result<McpOAuthAuthorization, McpClientError> {
        let client = Self::build_oauth_client(config)?;
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

    async fn exchange_code(
        &self,
        config: &McpOAuthConfig,
        code: &str,
        pkce_verifier: &str,
        http_client: &ReqwestClient,
    ) -> Result<McpOAuthTokens, McpClientError> {
        let client = Self::build_oauth_client(config)?;
        let token_response = client
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier.to_string()))
            .request_async(http_client)
            .await
            .map_err(|e| McpClientError::OAuth(e.to_string()))?;
        Ok(Self::tokens_from_response(&token_response))
    }

    async fn refresh_token(
        &self,
        config: &McpOAuthConfig,
        refresh_token: &str,
        http_client: &ReqwestClient,
    ) -> Result<McpOAuthTokens, McpClientError> {
        let client = Self::build_oauth_client(config)?;
        let token_response = client
            .exchange_refresh_token(&RefreshToken::new(refresh_token.to_string()))
            .request_async(http_client)
            .await
            .map_err(|e| McpClientError::OAuth(e.to_string()))?;
        Ok(Self::tokens_from_response(&token_response))
    }
}
