// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::auth::{identity::VerifiedIdentity, provider_config::OidcProviderConfiguration};
use oauth2::{
    AuthType, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet,
    EndpointSet, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse, TokenUrl,
    basic::BasicClient,
};
use reqwest::{Client as ReqwestClient, Url, header::ACCEPT};
use serde::Deserialize;
use thiserror::Error;

const GITHUB_AUTH_URL: &str = "https://github.com/login/oauth/authorize";
const GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const GITHUB_USER_URL: &str = "https://api.github.com/user";
const GITHUB_EMAILS_URL: &str = "https://api.github.com/user/emails";
const GITHUB_API_VERSION: &str = "2022-11-28";
const GITHUB_USER_AGENT: &str = "grengin-api";

type GitHubClient =
    BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

#[derive(Clone)]
struct GitHubEndpoints {
    authorization: String,
    token: String,
    user: String,
    emails: String,
}

impl Default for GitHubEndpoints {
    fn default() -> Self {
        Self {
            authorization: GITHUB_AUTH_URL.to_string(),
            token: GITHUB_TOKEN_URL.to_string(),
            user: GITHUB_USER_URL.to_string(),
            emails: GITHUB_EMAILS_URL.to_string(),
        }
    }
}

#[derive(Clone)]
pub struct GitHubOAuthAdapter {
    client: GitHubClient,
    client_id: ClientId,
    client_secret: ClientSecret,
    redirect_url: RedirectUrl,
    endpoints: GitHubEndpoints,
}

pub struct GitHubAuthorization {
    pub url: Url,
    pub state: String,
    pub pkce_verifier: String,
    pub nonce: String,
}

#[derive(Debug, Error)]
pub enum GitHubAdapterError {
    #[error("GitHub OAuth configuration is invalid")]
    InvalidConfiguration,
    #[error("GitHub OAuth client credentials are invalid")]
    InvalidCredentials,
    #[error("GitHub OAuth request failed")]
    OAuthRequest,
    #[error("GitHub did not return a verified email address")]
    MissingVerifiedEmail,
    #[error("GitHub returned an invalid identity response")]
    InvalidIdentity,
}

#[derive(Debug, Deserialize)]
struct GitHubUser {
    id: u64,
    login: String,
    name: Option<String>,
    email: Option<String>,
    avatar_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct GitHubEmail {
    email: String,
    primary: bool,
    verified: bool,
}

impl GitHubOAuthAdapter {
    pub fn supports_issuer(value: &str) -> bool {
        Url::parse(value).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str() == Some("github.com")
                && (url.path().is_empty() || url.path() == "/")
                && url.query().is_none()
                && url.fragment().is_none()
        })
    }

    pub fn new(
        client_id: String,
        client_secret: String,
        redirect_url: String,
    ) -> Result<Self, GitHubAdapterError> {
        Self::with_endpoints(
            client_id,
            client_secret,
            redirect_url,
            GitHubEndpoints::default(),
        )
    }

    fn with_endpoints(
        client_id: String,
        client_secret: String,
        redirect_url: String,
        endpoints: GitHubEndpoints,
    ) -> Result<Self, GitHubAdapterError> {
        let client_id = ClientId::new(client_id);
        let client_secret = ClientSecret::new(client_secret);
        let redirect_url =
            RedirectUrl::new(redirect_url).map_err(|_| GitHubAdapterError::InvalidConfiguration)?;
        let client = BasicClient::new(client_id.clone())
            .set_client_secret(client_secret.clone())
            .set_auth_type(AuthType::RequestBody)
            .set_auth_uri(
                AuthUrl::new(endpoints.authorization.clone())
                    .map_err(|_| GitHubAdapterError::InvalidConfiguration)?,
            )
            .set_token_uri(
                TokenUrl::new(endpoints.token.clone())
                    .map_err(|_| GitHubAdapterError::InvalidConfiguration)?,
            )
            .set_redirect_uri(redirect_url.clone());

        Ok(Self {
            client,
            client_id,
            client_secret,
            redirect_url,
            endpoints,
        })
    }

    pub fn begin(
        &self,
        configuration: &OidcProviderConfiguration,
    ) -> Result<GitHubAuthorization, GitHubAdapterError> {
        configuration
            .validate_for_provider("github")
            .map_err(|_| GitHubAdapterError::InvalidConfiguration)?;
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
        let mut request = self
            .client
            .authorize_url(CsrfToken::new_random)
            .set_pkce_challenge(pkce_challenge);
        for scope in &configuration.scopes {
            request = request.add_scope(Scope::new(scope.clone()));
        }
        for (key, value) in &configuration.authorization_params {
            request = request.add_extra_param(key.clone(), value.clone());
        }
        let (url, state) = request.url();
        Ok(GitHubAuthorization {
            url,
            state: state.secret().to_string(),
            pkce_verifier: pkce_verifier.secret().to_string(),
            nonce: CsrfToken::new_random().secret().to_string(),
        })
    }

    pub async fn validate_remote(
        &self,
        http_client: &ReqwestClient,
    ) -> Result<(), GitHubAdapterError> {
        let response = http_client
            .post(&self.endpoints.token)
            .header(ACCEPT, "application/json")
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.secret()),
                ("code", "grengin-github-validation-probe"),
                ("redirect_uri", self.redirect_url.as_str()),
            ])
            .send()
            .await
            .map_err(|_| GitHubAdapterError::OAuthRequest)?;
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|_| GitHubAdapterError::OAuthRequest)?;
        match body.get("error").and_then(|value| value.as_str()) {
            Some("bad_verification_code") => Ok(()),
            Some("incorrect_client_credentials" | "unauthorized_client") => {
                Err(GitHubAdapterError::InvalidCredentials)
            }
            _ => Err(GitHubAdapterError::OAuthRequest),
        }
    }

    pub async fn complete(
        &self,
        code: String,
        pkce_verifier: String,
        http_client: &ReqwestClient,
    ) -> Result<VerifiedIdentity, GitHubAdapterError> {
        let token_response = self
            .client
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier))
            .request_async(http_client)
            .await
            .map_err(|_| GitHubAdapterError::OAuthRequest)?;
        let access_token = token_response.access_token().secret();
        let user: GitHubUser = self
            .api_get(http_client, &self.endpoints.user, access_token)
            .await?;
        let emails: Vec<GitHubEmail> = self
            .api_get(http_client, &self.endpoints.emails, access_token)
            .await?;
        let email = select_verified_email(user.email.as_deref(), &emails)
            .ok_or(GitHubAdapterError::MissingVerifiedEmail)?;

        Ok(VerifiedIdentity {
            subject: user.id.to_string(),
            email: Some(email),
            email_verified: true,
            display_name: Some(user.name.unwrap_or(user.login)),
            picture: user.avatar_url,
            hosted_domain: None,
        })
    }

    async fn api_get<T: for<'de> Deserialize<'de>>(
        &self,
        http_client: &ReqwestClient,
        url: &str,
        access_token: &str,
    ) -> Result<T, GitHubAdapterError> {
        let response = http_client
            .get(url)
            .bearer_auth(access_token)
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .header("User-Agent", GITHUB_USER_AGENT)
            .send()
            .await
            .map_err(|_| GitHubAdapterError::OAuthRequest)?;
        if !response.status().is_success() {
            return Err(GitHubAdapterError::OAuthRequest);
        }
        response
            .json()
            .await
            .map_err(|_| GitHubAdapterError::InvalidIdentity)
    }
}

fn select_verified_email(profile_email: Option<&str>, emails: &[GitHubEmail]) -> Option<String> {
    emails
        .iter()
        .find(|email| email.verified && email.primary)
        .or_else(|| {
            profile_email.and_then(|profile| {
                emails
                    .iter()
                    .find(|email| email.verified && email.email.eq_ignore_ascii_case(profile))
            })
        })
        .or_else(|| emails.iter().find(|email| email.verified))
        .map(|email| email.email.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::Form, routing::get, routing::post};
    use std::collections::HashMap;
    use tokio::net::TcpListener;

    fn configuration() -> OidcProviderConfiguration {
        OidcProviderConfiguration {
            scopes: vec!["read:user".to_string(), "user:email".to_string()],
            ..Default::default()
        }
    }

    #[test]
    fn authorization_uses_pkce_and_least_privilege_scopes() {
        let adapter = GitHubOAuthAdapter::new(
            "client-id".to_string(),
            "client-secret".to_string(),
            "http://localhost:5173/auth/github/callback".to_string(),
        )
        .expect("adapter");
        let authorization = adapter.begin(&configuration()).expect("authorization");
        let query: HashMap<_, _> = authorization.url.query_pairs().into_owned().collect();

        assert_eq!(
            query.get("scope"),
            Some(&"read:user user:email".to_string())
        );
        assert_eq!(
            query.get("code_challenge_method"),
            Some(&"S256".to_string())
        );
        assert_eq!(
            query.get("redirect_uri"),
            Some(&"http://localhost:5173/auth/github/callback".to_string())
        );
        assert!(!query.contains_key("client_secret"));
        assert!(!authorization.state.is_empty());
        assert!(!authorization.pkce_verifier.is_empty());
    }

    #[test]
    fn verified_primary_email_wins_and_unverified_email_is_ignored() {
        let emails = vec![
            GitHubEmail {
                email: "public@example.com".to_string(),
                primary: false,
                verified: true,
            },
            GitHubEmail {
                email: "primary@example.com".to_string(),
                primary: true,
                verified: true,
            },
            GitHubEmail {
                email: "unsafe@example.com".to_string(),
                primary: false,
                verified: false,
            },
        ];
        assert_eq!(
            select_verified_email(Some("public@example.com"), &emails).as_deref(),
            Some("primary@example.com")
        );
        assert_eq!(
            select_verified_email(
                Some("unsafe@example.com"),
                &[GitHubEmail {
                    email: "unsafe@example.com".to_string(),
                    primary: true,
                    verified: false,
                }]
            ),
            None
        );
    }

    #[tokio::test]
    async fn completes_against_oauth_and_verified_email_endpoints() {
        async fn token(Form(form): Form<HashMap<String, String>>) -> Json<serde_json::Value> {
            assert_eq!(form.get("code").map(String::as_str), Some("valid-code"));
            assert!(
                form.get("code_verifier")
                    .is_some_and(|value| !value.is_empty())
            );
            Json(serde_json::json!({
                "access_token": "github-access-token",
                "token_type": "bearer",
                "scope": "read:user,user:email"
            }))
        }
        async fn user() -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "id": 42,
                "login": "octocat",
                "name": "The Octocat",
                "email": null,
                "avatar_url": "https://avatars.example/octocat"
            }))
        }
        async fn emails() -> Json<serde_json::Value> {
            Json(serde_json::json!([{
                "email": "octocat@example.com",
                "primary": true,
                "verified": true
            }]))
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let base = format!("http://{}", listener.local_addr().expect("address"));
        let router = Router::new()
            .route("/login/oauth/access_token", post(token))
            .route("/user", get(user))
            .route("/user/emails", get(emails));
        tokio::spawn(async move { axum::serve(listener, router).await.expect("server") });

        let adapter = GitHubOAuthAdapter::with_endpoints(
            "client-id".to_string(),
            "client-secret".to_string(),
            "http://localhost:5173/auth/github/callback".to_string(),
            GitHubEndpoints {
                authorization: format!("{base}/login/oauth/authorize"),
                token: format!("{base}/login/oauth/access_token"),
                user: format!("{base}/user"),
                emails: format!("{base}/user/emails"),
            },
        )
        .expect("adapter");
        let identity = adapter
            .complete(
                "valid-code".to_string(),
                "pkce-verifier".to_string(),
                &ReqwestClient::new(),
            )
            .await
            .expect("identity");

        assert_eq!(identity.subject, "42");
        assert_eq!(identity.email.as_deref(), Some("octocat@example.com"));
        assert!(identity.email_verified);
        assert_eq!(identity.display_name.as_deref(), Some("The Octocat"));
    }
}
