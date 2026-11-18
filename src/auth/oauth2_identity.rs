// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::auth::{
    identity::VerifiedIdentity,
    provider_config::{OidcProviderConfiguration, ProviderConfigError, validate_provider_url},
};
use oauth2::{
    AuthType, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet,
    EndpointSet, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse, TokenUrl,
    basic::BasicClient,
};
use reqwest::{
    Client as ReqwestClient, Url,
    header::{ACCEPT, AUTHORIZATION, HeaderName, HeaderValue, USER_AGENT},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use thiserror::Error;
use utoipa::ToSchema;

// Sign-in for providers that speak plain OAuth2 rather than OpenID Connect (GitHub, for example).
// Everything provider-specific comes from the template's `oauth2` profile: endpoints, API headers,
// and JSON pointers into the profile and verified-email responses. A sign-in only completes with an
// email address the provider reports as verified.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OAuth2IdentityProfile {
    pub authorization_url: String,
    pub token_url: String,
    #[serde(default)]
    pub api_headers: BTreeMap<String, String>,
    pub profile: OAuth2ProfileEndpoint,
    pub verified_emails: OAuth2VerifiedEmailsEndpoint,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_check: Option<OAuth2ClientCheck>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OAuth2ProfileEndpoint {
    pub url: String,
    pub subject: String,
    // The first pointer with a non-empty string wins (for example name, then login).
    #[serde(default)]
    pub display_name: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OAuth2VerifiedEmailsEndpoint {
    pub url: String,
    pub address: String,
    pub verified: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<String>,
}

// The token endpoint is probed with a code that cannot be valid: accepted_error means the client
// credentials were accepted; any of rejected_errors means they were not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OAuth2ClientCheck {
    pub accepted_error: String,
    pub rejected_errors: Vec<String>,
}

impl OAuth2IdentityProfile {
    pub fn validate(&self, scopes: &[String]) -> Result<(), ProviderConfigError> {
        for url in [
            &self.authorization_url,
            &self.token_url,
            &self.profile.url,
            &self.verified_emails.url,
        ] {
            validate_provider_url(url, true)?;
        }
        let pointers = std::iter::once(&self.profile.subject)
            .chain(&self.profile.display_name)
            .chain(&self.profile.picture)
            .chain(&self.profile.email)
            .chain([
                &self.verified_emails.address,
                &self.verified_emails.verified,
            ])
            .chain(&self.verified_emails.primary);
        for pointer in pointers {
            if !pointer.starts_with('/') || pointer.len() > 256 {
                return Err(ProviderConfigError::InvalidConfiguration(format!(
                    "oauth2 pointer '{pointer}' must be a JSON pointer"
                )));
            }
        }
        if self.api_headers.len() > 16 {
            return Err(ProviderConfigError::InvalidConfiguration(
                "oauth2 apiHeaders allows at most 16 headers".into(),
            ));
        }
        for (name, value) in &self.api_headers {
            let header = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                ProviderConfigError::InvalidConfiguration(format!("invalid header name {name}"))
            })?;
            // The access token is attached per request and must not be replaced by configuration.
            if header == AUTHORIZATION || HeaderValue::from_str(value).is_err() {
                return Err(ProviderConfigError::InvalidConfiguration(format!(
                    "header {name} cannot be configured"
                )));
            }
        }
        if !self.required_scopes.is_empty() {
            let requested: HashSet<&str> = scopes.iter().map(String::as_str).collect();
            let required: HashSet<&str> = self.required_scopes.iter().map(String::as_str).collect();
            if requested != required {
                return Err(ProviderConfigError::InvalidScopes);
            }
        }
        Ok(())
    }

    // The issuer names the provider's own origin, so it must match where users sign in.
    pub fn supports_issuer(&self, issuer: &str) -> bool {
        match (Url::parse(issuer), Url::parse(&self.authorization_url)) {
            (Ok(issuer), Ok(authorization)) => {
                issuer.origin() == authorization.origin()
                    && (issuer.path().is_empty() || issuer.path() == "/")
                    && issuer.query().is_none()
                    && issuer.fragment().is_none()
            }
            _ => false,
        }
    }
}

type OAuth2Client =
    BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

#[derive(Clone)]
pub struct OAuth2IdentityAdapter {
    client: OAuth2Client,
    client_id: ClientId,
    client_secret: ClientSecret,
    redirect_url: RedirectUrl,
    profile: OAuth2IdentityProfile,
}

pub struct OAuth2Authorization {
    pub url: Url,
    pub state: String,
    pub pkce_verifier: String,
    pub nonce: String,
}

#[derive(Debug, Error)]
pub enum OAuth2AdapterError {
    #[error("OAuth2 sign-in configuration is invalid")]
    InvalidConfiguration,
    #[error("OAuth2 client credentials are invalid")]
    InvalidCredentials,
    #[error("OAuth2 request failed")]
    OAuthRequest,
    #[error("the provider did not return a verified email address")]
    MissingVerifiedEmail,
    #[error("the provider returned an invalid identity response")]
    InvalidIdentity,
}

impl OAuth2IdentityAdapter {
    pub fn new(
        profile: OAuth2IdentityProfile,
        client_id: String,
        client_secret: String,
        redirect_url: String,
    ) -> Result<Self, OAuth2AdapterError> {
        let client_id = ClientId::new(client_id);
        let client_secret = ClientSecret::new(client_secret);
        let redirect_url =
            RedirectUrl::new(redirect_url).map_err(|_| OAuth2AdapterError::InvalidConfiguration)?;
        let client = BasicClient::new(client_id.clone())
            .set_client_secret(client_secret.clone())
            .set_auth_type(AuthType::RequestBody)
            .set_auth_uri(
                AuthUrl::new(profile.authorization_url.clone())
                    .map_err(|_| OAuth2AdapterError::InvalidConfiguration)?,
            )
            .set_token_uri(
                TokenUrl::new(profile.token_url.clone())
                    .map_err(|_| OAuth2AdapterError::InvalidConfiguration)?,
            )
            .set_redirect_uri(redirect_url.clone());
        Ok(Self {
            client,
            client_id,
            client_secret,
            redirect_url,
            profile,
        })
    }

    pub fn begin(
        &self,
        configuration: &OidcProviderConfiguration,
    ) -> Result<OAuth2Authorization, OAuth2AdapterError> {
        self.profile
            .validate(&configuration.scopes)
            .map_err(|_| OAuth2AdapterError::InvalidConfiguration)?;
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
        Ok(OAuth2Authorization {
            url,
            state: state.secret().to_string(),
            pkce_verifier: pkce_verifier.secret().to_string(),
            nonce: CsrfToken::new_random().secret().to_string(),
        })
    }

    // Without a client check there is nothing to verify before a real sign-in.
    pub async fn validate_remote(
        &self,
        http_client: &ReqwestClient,
    ) -> Result<bool, OAuth2AdapterError> {
        let Some(probe) = &self.profile.client_check else {
            return Ok(false);
        };
        let response = http_client
            .post(&self.profile.token_url)
            .header(ACCEPT, "application/json")
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.secret()),
                ("code", "grengin-credential-probe"),
                ("redirect_uri", self.redirect_url.as_str()),
            ])
            .send()
            .await
            .map_err(|_| OAuth2AdapterError::OAuthRequest)?;
        let body: Value = response
            .json()
            .await
            .map_err(|_| OAuth2AdapterError::OAuthRequest)?;
        match body.get("error").and_then(Value::as_str) {
            Some(error) if error == probe.accepted_error => Ok(true),
            Some(error) if probe.rejected_errors.iter().any(|known| known == error) => {
                Err(OAuth2AdapterError::InvalidCredentials)
            }
            _ => Err(OAuth2AdapterError::OAuthRequest),
        }
    }

    pub async fn complete(
        &self,
        code: String,
        pkce_verifier: String,
        http_client: &ReqwestClient,
    ) -> Result<VerifiedIdentity, OAuth2AdapterError> {
        let token_response = self
            .client
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier))
            .request_async(http_client)
            .await
            .map_err(|_| OAuth2AdapterError::OAuthRequest)?;
        let access_token = token_response.access_token().secret();
        let profile = self
            .api_get(http_client, &self.profile.profile.url, access_token)
            .await?;
        let emails = self
            .api_get(http_client, &self.profile.verified_emails.url, access_token)
            .await?;
        let subject = match profile.pointer(&self.profile.profile.subject) {
            Some(Value::String(value)) if !value.is_empty() => value.clone(),
            Some(Value::Number(value)) => value.to_string(),
            _ => return Err(OAuth2AdapterError::InvalidIdentity),
        };
        let profile_email = self
            .profile
            .profile
            .email
            .as_deref()
            .and_then(|pointer| profile.pointer(pointer))
            .and_then(Value::as_str);
        let email = select_verified_email(
            profile_email,
            &verified_email_candidates(&emails, &self.profile.verified_emails)?,
        )
        .ok_or(OAuth2AdapterError::MissingVerifiedEmail)?;
        let display_name = self
            .profile
            .profile
            .display_name
            .iter()
            .find_map(|pointer| {
                profile
                    .pointer(pointer)
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
            })
            .map(str::to_string);
        let picture = self
            .profile
            .profile
            .picture
            .as_deref()
            .and_then(|pointer| profile.pointer(pointer))
            .and_then(Value::as_str)
            .map(str::to_string);
        Ok(VerifiedIdentity {
            subject,
            email: Some(email),
            email_verified: true,
            display_name,
            picture,
            hosted_domain: None,
        })
    }

    async fn api_get(
        &self,
        http_client: &ReqwestClient,
        url: &str,
        access_token: &str,
    ) -> Result<Value, OAuth2AdapterError> {
        let mut request = http_client
            .get(url)
            .bearer_auth(access_token)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, "grengin-api");
        for (name, value) in &self.profile.api_headers {
            request = request.header(name, value);
        }
        let response = request
            .send()
            .await
            .map_err(|_| OAuth2AdapterError::OAuthRequest)?;
        if !response.status().is_success() {
            return Err(OAuth2AdapterError::OAuthRequest);
        }
        response
            .json()
            .await
            .map_err(|_| OAuth2AdapterError::InvalidIdentity)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct VerifiedEmailCandidate {
    email: String,
    primary: bool,
    verified: bool,
}

fn verified_email_candidates(
    response: &Value,
    endpoint: &OAuth2VerifiedEmailsEndpoint,
) -> Result<Vec<VerifiedEmailCandidate>, OAuth2AdapterError> {
    let entries = response
        .as_array()
        .ok_or(OAuth2AdapterError::InvalidIdentity)?;
    Ok(entries
        .iter()
        .filter_map(|entry| {
            Some(VerifiedEmailCandidate {
                email: entry.pointer(&endpoint.address)?.as_str()?.to_string(),
                verified: entry.pointer(&endpoint.verified)?.as_bool()?,
                primary: endpoint
                    .primary
                    .as_deref()
                    .and_then(|pointer| entry.pointer(pointer))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect())
}

fn select_verified_email(
    profile_email: Option<&str>,
    emails: &[VerifiedEmailCandidate],
) -> Option<String> {
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

    // The GitHub sign-in as the catalog template describes it, against the given base URL.
    fn github_profile(base: &str) -> OAuth2IdentityProfile {
        serde_json::from_value(serde_json::json!({
            "authorizationUrl": format!("{base}/login/oauth/authorize"),
            "tokenUrl": format!("{base}/login/oauth/access_token"),
            "apiHeaders": {"Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28"},
            "profile": {"url": format!("{base}/user"), "subject": "/id", "displayName": ["/name", "/login"],
                        "picture": "/avatar_url", "email": "/email"},
            "verifiedEmails": {"url": format!("{base}/user/emails"), "address": "/email",
                               "verified": "/verified", "primary": "/primary"},
            "requiredScopes": ["read:user", "user:email"],
            "clientCheck": {"acceptedError": "bad_verification_code",
                                "rejectedErrors": ["incorrect_client_credentials", "unauthorized_client"]}
        }))
        .expect("profile")
    }

    impl OAuth2IdentityProfile {
        fn with_api_host(mut self, api: &str) -> Self {
            self.profile.url = format!("{api}/user");
            self.verified_emails.url = format!("{api}/user/emails");
            self
        }
    }

    fn configuration(base: &str) -> OidcProviderConfiguration {
        OidcProviderConfiguration {
            scopes: vec!["read:user".to_string(), "user:email".to_string()],
            oauth2: Some(github_profile(base)),
            ..Default::default()
        }
    }

    #[test]
    fn authorization_uses_pkce_and_the_profile_scopes() {
        let adapter = OAuth2IdentityAdapter::new(
            github_profile("https://github.com"),
            "client-id".to_string(),
            "client-secret".to_string(),
            "http://localhost:5173/auth/github/callback".to_string(),
        )
        .expect("adapter");
        let authorization = adapter
            .begin(&configuration("https://github.com"))
            .expect("authorization");
        let query: HashMap<_, _> = authorization.url.query_pairs().into_owned().collect();
        assert_eq!(
            query.get("scope"),
            Some(&"read:user user:email".to_string())
        );
        assert_eq!(
            query.get("code_challenge_method"),
            Some(&"S256".to_string())
        );
        assert!(!query.contains_key("client_secret"));
        assert!(!authorization.state.is_empty());
        assert!(!authorization.pkce_verifier.is_empty());
        let mut broader = configuration("https://github.com");
        broader.scopes.push("repo".to_string());
        assert!(
            adapter.begin(&broader).is_err(),
            "only the profile's scopes may be requested"
        );
    }

    #[test]
    fn profiles_reject_bad_urls_pointers_headers_and_issuers() {
        let profile = github_profile("https://github.com");
        assert!(profile.supports_issuer("https://github.com"));
        assert!(!profile.supports_issuer("https://evil.example"));
        assert!(!profile.supports_issuer("https://github.com/path"));
        let scopes = vec!["read:user".to_string(), "user:email".to_string()];
        let mut insecure = profile.clone();
        insecure.token_url = "http://github.example/token".to_string();
        assert!(insecure.validate(&scopes).is_err());
        let mut pointer = profile.clone();
        pointer.profile.subject = "id".to_string();
        assert!(pointer.validate(&scopes).is_err());
        let mut header = profile.clone();
        header
            .api_headers
            .insert("Authorization".to_string(), "Bearer other".to_string());
        assert!(header.validate(&scopes).is_err());
    }

    #[test]
    fn the_github_profile_backfilled_by_the_migration_is_valid() {
        let source = include_str!(
            "../../migration/src/m20261118_000002_oauth2_profile_for_github_sign_in.rs"
        );
        let start = source.find("'oauth2', '").expect("profile literal") + "'oauth2', '".len();
        let end = start + source[start..].find("'::jsonb)").expect("profile end");
        let profile: OAuth2IdentityProfile =
            serde_json::from_str(&source[start..end]).expect("the backfilled profile parses");
        assert_eq!(
            profile,
            github_profile("https://github.com").with_api_host("https://api.github.com")
        );
        profile
            .validate(&["read:user".to_string(), "user:email".to_string()])
            .expect("the backfilled profile validates");
        assert!(profile.supports_issuer("https://github.com"));
    }

    #[test]
    fn verified_primary_email_wins_and_unverified_email_is_ignored() {
        let candidate = |email: &str, primary, verified| VerifiedEmailCandidate {
            email: email.to_string(),
            primary,
            verified,
        };
        let emails = vec![
            candidate("public@example.com", false, true),
            candidate("primary@example.com", true, true),
            candidate("unsafe@example.com", false, false),
        ];
        assert_eq!(
            select_verified_email(Some("public@example.com"), &emails).as_deref(),
            Some("primary@example.com")
        );
        assert_eq!(
            select_verified_email(
                Some("unsafe@example.com"),
                &[candidate("unsafe@example.com", true, false)]
            ),
            None
        );
    }

    #[tokio::test]
    async fn completes_against_oauth_and_verified_email_endpoints() {
        async fn token(Form(form): Form<HashMap<String, String>>) -> Json<Value> {
            assert_eq!(form.get("code").map(String::as_str), Some("valid-code"));
            assert!(
                form.get("code_verifier")
                    .is_some_and(|value| !value.is_empty())
            );
            Json(serde_json::json!({
                "access_token": "provider-access-token",
                "token_type": "bearer",
                "scope": "read:user,user:email"
            }))
        }
        async fn user(headers: axum::http::HeaderMap) -> Json<Value> {
            assert_eq!(headers["x-github-api-version"], "2022-11-28");
            Json(serde_json::json!({
                "id": 42,
                "login": "octocat",
                "name": null,
                "email": null,
                "avatar_url": "https://avatars.example/octocat"
            }))
        }
        async fn emails() -> Json<Value> {
            Json(serde_json::json!([
                {"email": "unverified@example.com", "primary": false, "verified": false},
                {"email": "octocat@example.com", "primary": true, "verified": true}
            ]))
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let base = format!("http://{}", listener.local_addr().expect("address"));
        let router = Router::new()
            .route("/login/oauth/access_token", post(token))
            .route("/user", get(user))
            .route("/user/emails", get(emails));
        tokio::spawn(async move { axum::serve(listener, router).await.expect("server") });

        let adapter = OAuth2IdentityAdapter::new(
            github_profile(&base),
            "client-id".to_string(),
            "client-secret".to_string(),
            "http://localhost:5173/auth/github/callback".to_string(),
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
        assert_eq!(
            identity.display_name.as_deref(),
            Some("octocat"),
            "an empty name falls back to the next pointer"
        );
        assert_eq!(
            identity.picture.as_deref(),
            Some("https://avatars.example/octocat")
        );
    }
}
