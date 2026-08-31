// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use openidconnect::{ClientId, ClientSecret, IssuerUrl, RedirectUrl, core::CoreProviderMetadata};
use reqwest::{Client as ReqwestClient, Url};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use thiserror::Error;
use utoipa::ToSchema;

use crate::config::setting::OidcClient;

pub const OIDC_PROVIDER_CONFIG_VERSION: &str = "1.0";

const RESERVED_AUTHORIZATION_PARAMS: &[&str] = &[
    "client_id",
    "code_challenge",
    "code_challenge_method",
    "nonce",
    "redirect_uri",
    "response_type",
    "scope",
    "state",
];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum EmailLinkingMode {
    Disabled,
    #[default]
    VerifiedEmail,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct OidcProviderConfiguration {
    pub version: String,
    pub scopes: Vec<String>,
    pub authorization_params: BTreeMap<String, String>,
    pub email_linking: EmailLinkingMode,
    pub auto_redirect: bool,
}

impl Default for OidcProviderConfiguration {
    fn default() -> Self {
        Self {
            version: OIDC_PROVIDER_CONFIG_VERSION.to_string(),
            scopes: vec![
                "openid".to_string(),
                "email".to_string(),
                "profile".to_string(),
            ],
            authorization_params: BTreeMap::new(),
            email_linking: EmailLinkingMode::VerifiedEmail,
            auto_redirect: false,
        }
    }
}

impl OidcProviderConfiguration {
    pub fn from_value(value: Option<&serde_json::Value>) -> Result<Self, ProviderConfigError> {
        let configuration = match value {
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|error| ProviderConfigError::InvalidConfiguration(error.to_string()))?,
            None => Self::default(),
        };
        configuration.validate()?;
        Ok(configuration)
    }

    pub fn validate(&self) -> Result<(), ProviderConfigError> {
        if self.version != OIDC_PROVIDER_CONFIG_VERSION {
            return Err(ProviderConfigError::UnsupportedVersion(
                self.version.clone(),
            ));
        }
        if self.scopes.is_empty() || self.scopes.len() > 32 {
            return Err(ProviderConfigError::InvalidScopes);
        }

        let mut seen = HashSet::new();
        for scope in &self.scopes {
            let scope = scope.trim();
            if scope.is_empty()
                || scope.len() > 128
                || scope.chars().any(char::is_whitespace)
                || !seen.insert(scope.to_string())
            {
                return Err(ProviderConfigError::InvalidScopes);
            }
        }
        if !seen.contains("openid") {
            return Err(ProviderConfigError::MissingOpenIdScope);
        }

        for (key, value) in &self.authorization_params {
            let normalized = key.trim().to_ascii_lowercase();
            if normalized.is_empty()
                || key.len() > 128
                || value.len() > 2048
                || RESERVED_AUTHORIZATION_PARAMS.contains(&normalized.as_str())
            {
                return Err(ProviderConfigError::InvalidAuthorizationParameter(
                    key.clone(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProviderConfigError {
    #[error("authorization parameter '{0}' is reserved or invalid")]
    InvalidAuthorizationParameter(String),
    #[error("provider configuration is invalid: {0}")]
    InvalidConfiguration(String),
    #[error("provider slug is invalid")]
    InvalidProviderSlug,
    #[error("provider URL is invalid")]
    InvalidUrl,
    #[error("OIDC scopes are invalid")]
    InvalidScopes,
    #[error("OIDC providers must request the openid scope")]
    MissingOpenIdScope,
    #[error("provider configuration version '{0}' is unsupported")]
    UnsupportedVersion(String),
}

pub fn normalize_provider_slug(value: &str) -> Result<String, ProviderConfigError> {
    let slug = value.trim().to_ascii_lowercase();
    let mut chars = slug.chars();
    let starts_with_letter = chars
        .next()
        .is_some_and(|character| character.is_ascii_lowercase());
    if !starts_with_letter
        || slug.len() > 63
        || !slug.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
        || slug.ends_with('-')
    {
        return Err(ProviderConfigError::InvalidProviderSlug);
    }
    Ok(slug)
}

pub fn validate_provider_url(
    value: &str,
    allow_loopback_http: bool,
) -> Result<Url, ProviderConfigError> {
    let url = Url::parse(value).map_err(|_| ProviderConfigError::InvalidUrl)?;
    let loopback = url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if url.scheme() != "https" && !(allow_loopback_http && url.scheme() == "http" && loopback) {
        return Err(ProviderConfigError::InvalidUrl);
    }
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(ProviderConfigError::InvalidUrl);
    }
    Ok(url)
}

pub async fn build_discovered_oidc_client(
    req_client: &ReqwestClient,
    issuer_url: &str,
    client_id: String,
    client_secret: String,
    redirect_url: String,
) -> Result<OidcClient, anyhow::Error> {
    validate_provider_url(issuer_url, true)?;
    validate_provider_url(&redirect_url, true)?;
    let metadata =
        CoreProviderMetadata::discover_async(IssuerUrl::new(issuer_url.to_string())?, req_client)
            .await?;
    Ok(OidcClient::from_provider_metadata(
        metadata,
        ClientId::new(client_id),
        Some(ClientSecret::new(client_secret)),
    )
    .set_redirect_uri(RedirectUrl::new(redirect_url)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_valid_provider_slugs() {
        assert_eq!(
            normalize_provider_slug(" Keycloak-EU ").unwrap(),
            "keycloak-eu"
        );
    }

    #[test]
    fn rejects_unsafe_provider_slugs() {
        for value in ["", "-okta", "okta-", "okta_eu", "okta/eu", "42-okta"] {
            assert!(normalize_provider_slug(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn requires_openid_and_rejects_reserved_parameters() {
        let missing_openid = OidcProviderConfiguration {
            scopes: vec!["email".to_string()],
            ..Default::default()
        };
        assert_eq!(
            missing_openid.validate(),
            Err(ProviderConfigError::MissingOpenIdScope)
        );

        let mut reserved = OidcProviderConfiguration::default();
        reserved.authorization_params.insert(
            "redirect_uri".to_string(),
            "https://attacker.example".to_string(),
        );
        assert!(matches!(
            reserved.validate(),
            Err(ProviderConfigError::InvalidAuthorizationParameter(_))
        ));
    }

    #[test]
    fn provider_urls_require_https_except_loopback() {
        assert!(validate_provider_url("https://id.example.com/realms/acme", true).is_ok());
        assert!(validate_provider_url("http://localhost:5556/dex", true).is_ok());
        assert!(validate_provider_url("http://id.example.com", true).is_err());
        assert!(validate_provider_url("https://user:pass@id.example.com", true).is_err());
        assert!(validate_provider_url("https://id.example.com/#fragment", true).is_err());
    }
}

#[cfg(test)]
mod mock_oidc_matrix_tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::{Form, Path, State},
        routing::{get, post},
    };
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use chrono::Utc;
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    use openidconnect::{
        AuthorizationCode, CsrfToken, Nonce, OAuth2TokenResponse, PkceCodeChallenge, Scope,
        TokenResponse as _,
        core::{CoreAuthenticationFlow, CoreUserInfoClaims},
    };
    use openssl::rsa::Rsa;
    use serde::Serialize;
    use std::{
        borrow::Cow,
        collections::HashMap,
        sync::{Arc, Mutex},
    };
    use tokio::net::TcpListener;

    #[derive(Clone)]
    struct ProviderCase {
        slug: &'static str,
        scopes: &'static [&'static str],
        authorization_params: &'static [(&'static str, &'static str)],
        subject: &'static str,
        display_name: &'static str,
        id_token_email: Option<&'static str>,
        userinfo_email: Option<&'static str>,
    }

    #[derive(Clone)]
    struct IssuerFixture {
        case: ProviderCase,
        issuer_url: String,
        encoding_key: Arc<EncodingKey>,
        jwks: serde_json::Value,
        nonce: Arc<Mutex<Option<String>>>,
    }

    struct MockOidcServer {
        base_url: String,
        issuers: HashMap<String, Arc<IssuerFixture>>,
    }

    impl MockOidcServer {
        async fn start(cases: Vec<ProviderCase>) -> std::io::Result<Self> {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .map_err(|error| std::io::Error::new(error.kind(), error.to_string()))?;
            let base_url = format!("http://{}", listener.local_addr().expect("mock oidc addr"));

            let mut issuers = HashMap::new();
            for case in cases {
                let rsa = Rsa::generate(2048).expect("generate rsa key");
                let private_key_pem = rsa.private_key_to_pem().expect("private pem");
                let encoding_key =
                    Arc::new(EncodingKey::from_rsa_pem(&private_key_pem).expect("encoding key"));
                let kid = format!("{}-kid", case.slug);
                let jwks = serde_json::json!({
                    "keys": [{
                        "kty": "RSA",
                        "use": "sig",
                        "alg": "RS256",
                        "kid": kid,
                        "n": URL_SAFE_NO_PAD.encode(rsa.n().to_vec()),
                        "e": URL_SAFE_NO_PAD.encode(rsa.e().to_vec()),
                    }]
                });
                let issuer_url = format!("{}/{}", base_url, case.slug);
                issuers.insert(
                    case.slug.to_string(),
                    Arc::new(IssuerFixture {
                        case,
                        issuer_url,
                        encoding_key,
                        jwks,
                        nonce: Arc::new(Mutex::new(None)),
                    }),
                );
            }

            let state = Arc::new(MockOidcServer {
                base_url: base_url.clone(),
                issuers,
            });
            let router = Router::new()
                .route(
                    "/{issuer}/.well-known/openid-configuration",
                    get(discovery_handler),
                )
                .route("/{issuer}/oauth/token", post(token_handler))
                .route("/{issuer}/oauth/jwks", get(jwks_handler))
                .route("/{issuer}/userinfo", get(userinfo_handler))
                .with_state(state.clone());

            tokio::spawn(async move {
                axum::serve(listener, router)
                    .await
                    .expect("mock oidc server");
            });

            Ok(Self {
                base_url,
                issuers: state.issuers.clone(),
            })
        }

        fn issuer_url(&self, slug: &str) -> String {
            format!("{}/{}", self.base_url, slug)
        }

        fn set_nonce(&self, slug: &str, nonce: String) {
            let fixture = self.issuers.get(slug).expect("issuer fixture");
            let mut guard = fixture.nonce.lock().expect("nonce mutex");
            *guard = Some(nonce);
        }

        fn fixture(&self, slug: &str) -> Arc<IssuerFixture> {
            self.issuers.get(slug).expect("issuer fixture").clone()
        }
    }

    #[derive(Serialize)]
    struct IdTokenClaims<'a> {
        iss: &'a str,
        sub: &'a str,
        aud: &'a str,
        exp: usize,
        iat: usize,
        nonce: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        email: Option<&'a str>,
        email_verified: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        preferred_username: Option<&'a str>,
    }

    async fn discovery_handler(
        Path(issuer): Path<String>,
        State(state): State<Arc<MockOidcServer>>,
    ) -> Json<serde_json::Value> {
        let fixture = state.fixture(&issuer);
        let issuer_url = fixture.issuer_url.clone();
        Json(serde_json::json!({
            "issuer": issuer_url,
            "authorization_endpoint": format!("{}/oauth/authorize", fixture.issuer_url),
            "token_endpoint": format!("{}/oauth/token", fixture.issuer_url),
            "jwks_uri": format!("{}/oauth/jwks", fixture.issuer_url),
            "userinfo_endpoint": format!("{}/userinfo", fixture.issuer_url),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["RS256"],
            "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post"],
            "scopes_supported": ["openid", "email", "profile", "groups", "offline_access"],
        }))
    }

    async fn jwks_handler(
        Path(issuer): Path<String>,
        State(state): State<Arc<MockOidcServer>>,
    ) -> Json<serde_json::Value> {
        Json(state.fixture(&issuer).jwks.clone())
    }

    async fn token_handler(
        Path(issuer): Path<String>,
        State(state): State<Arc<MockOidcServer>>,
        Form(_form): Form<HashMap<String, String>>,
    ) -> Json<serde_json::Value> {
        let fixture = state.fixture(&issuer);
        let nonce = fixture
            .nonce
            .lock()
            .expect("nonce mutex")
            .take()
            .expect("nonce must be set before token exchange");
        let now = Utc::now().timestamp().max(0) as usize;
        let claims = IdTokenClaims {
            iss: &fixture.issuer_url,
            sub: fixture.case.subject,
            aud: &format!("{}-client", fixture.case.slug),
            exp: now + 3600,
            iat: now,
            nonce: &nonce,
            email: fixture.case.id_token_email,
            email_verified: true,
            name: Some(fixture.case.display_name),
            preferred_username: Some(fixture.case.display_name),
        };
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(format!("{}-kid", fixture.case.slug));
        let id_token =
            jsonwebtoken::encode(&header, &claims, &fixture.encoding_key).expect("sign id token");
        Json(serde_json::json!({
            "access_token": format!("access-{}", fixture.case.slug),
            "token_type": "Bearer",
            "expires_in": 3600,
            "scope": format!("openid {}", fixture.case.scopes.join(" ")),
            "id_token": id_token,
        }))
    }

    async fn userinfo_handler(
        Path(issuer): Path<String>,
        State(state): State<Arc<MockOidcServer>>,
    ) -> Json<serde_json::Value> {
        let fixture = state.fixture(&issuer);
        Json(serde_json::json!({
            "sub": fixture.case.subject,
            "email": fixture.case.userinfo_email.or(fixture.case.id_token_email),
            "email_verified": true,
            "name": fixture.case.display_name,
        }))
    }

    fn provider_cases() -> Vec<ProviderCase> {
        // Google OIDC and Entra ID / Azure AD are already covered elsewhere. This matrix keeps
        // the smoke coverage focused on the popular provider families we still want to exercise
        // through the generic OIDC path.
        vec![
            ProviderCase {
                slug: "auth0",
                scopes: &["email", "profile", "offline_access"],
                authorization_params: &[("audience", "https://api.example.com")],
                subject: "auth0-user-01",
                display_name: "Auth0 User",
                id_token_email: Some("auth0.user@example.com"),
                userinfo_email: None,
            },
            ProviderCase {
                slug: "okta",
                scopes: &["email", "profile", "groups"],
                authorization_params: &[("prompt", "login")],
                subject: "okta-user-01",
                display_name: "Okta User",
                id_token_email: Some("okta.user@example.com"),
                userinfo_email: None,
            },
            ProviderCase {
                slug: "keycloak",
                scopes: &["email", "profile", "groups"],
                authorization_params: &[("kc_idp_hint", "corporate")],
                subject: "keycloak-user-01",
                display_name: "Keycloak User",
                id_token_email: Some("keycloak.user@example.com"),
                userinfo_email: None,
            },
            ProviderCase {
                slug: "apple",
                scopes: &["email", "profile"],
                authorization_params: &[("response_mode", "form_post")],
                subject: "apple-user-01",
                display_name: "Apple User",
                id_token_email: Some("apple.user@example.com"),
                userinfo_email: None,
            },
            ProviderCase {
                slug: "github",
                scopes: &["email", "profile", "offline_access"],
                authorization_params: &[("allow_signup", "false")],
                subject: "github-user-01",
                display_name: "GitHub User",
                id_token_email: None,
                userinfo_email: Some("github.user@example.com"),
            },
        ]
    }

    fn requested_scopes(case: &ProviderCase) -> Vec<String> {
        let mut scopes = vec![
            "openid".to_string(),
            "email".to_string(),
            "profile".to_string(),
        ];
        for scope in case.scopes {
            if !scopes.iter().any(|existing| existing == scope) {
                scopes.push(scope.to_string());
            }
        }
        scopes
    }

    fn query_value(url: &reqwest::Url, key: &str) -> Option<String> {
        url.query_pairs()
            .find_map(|(k, v)| (k == key).then_some(v.to_string()))
    }

    #[tokio::test]
    async fn popular_oidc_provider_families_round_trip_through_discovery_and_tokens() {
        let server = match MockOidcServer::start(provider_cases()).await {
            Ok(server) => server,
            Err(error) => {
                eprintln!("skipping mock oidc matrix: {error}");
                return;
            }
        };
        let req_client = reqwest::Client::new();

        for case in provider_cases() {
            let issuer_url = server.issuer_url(case.slug);
            let redirect_url = format!("http://localhost:8080/auth/{}/callback", case.slug);
            let client = build_discovered_oidc_client(
                &req_client,
                &issuer_url,
                format!("{}-client", case.slug),
                "client-secret".to_string(),
                redirect_url.clone(),
            )
            .await
            .unwrap_or_else(|error| panic!("{} discovery failed: {error}", case.slug));

            let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
            let mut authorization = client
                .authorize_url(
                    CoreAuthenticationFlow::AuthorizationCode,
                    CsrfToken::new_random,
                    Nonce::new_random,
                )
                .set_redirect_uri(Cow::Owned(
                    openidconnect::RedirectUrl::new(redirect_url).expect("redirect url"),
                ))
                .set_pkce_challenge(pkce_challenge);
            for scope in requested_scopes(&case) {
                authorization = authorization.add_scope(Scope::new(scope));
            }
            for &(key, value) in case.authorization_params {
                authorization = authorization.add_extra_param(key, value);
            }
            let (auth_url, _csrf_state, nonce) = authorization.url();

            server.set_nonce(case.slug, nonce.secret().to_string());

            let scope_value = query_value(&auth_url, "scope").expect("scope query");
            let scope_set: std::collections::HashSet<String> = scope_value
                .split_whitespace()
                .map(|scope| scope.to_string())
                .collect();
            for scope in requested_scopes(&case) {
                assert!(
                    scope_set.contains(&scope),
                    "missing scope {scope} for {}",
                    case.slug
                );
            }
            for &(key, value) in case.authorization_params {
                assert_eq!(
                    query_value(&auth_url, key).as_deref(),
                    Some(value),
                    "authorization parameter {key} was not preserved for {}",
                    case.slug
                );
            }
            assert!(
                auth_url
                    .path()
                    .contains(&format!("/{}/oauth/authorize", case.slug)),
                "authorization endpoint did not stay on the case-specific issuer path for {}",
                case.slug
            );

            let token_response = client
                .exchange_code(AuthorizationCode::new("mock-code".to_string()))
                .expect("token request")
                .set_pkce_verifier(pkce_verifier)
                .request_async(&req_client)
                .await
                .unwrap_or_else(|error| panic!("{} token exchange failed: {error}", case.slug));

            let id_token = token_response.id_token().expect("id token");
            let claims = id_token
                .claims(&client.id_token_verifier(), &nonce)
                .unwrap_or_else(|error| {
                    panic!("{} claims verification failed: {error}", case.slug)
                });

            assert_eq!(claims.subject().as_str(), case.subject);
            assert_eq!(
                claims.email().map(|email| email.as_str()),
                case.id_token_email
            );
            assert_eq!(
                claims
                    .name()
                    .and_then(|name| name.get(None).map(|value| value.to_string())),
                Some(case.display_name.to_string())
            );

            if case.id_token_email.is_none() {
                let info: CoreUserInfoClaims = client
                    .user_info(token_response.access_token().to_owned(), None)
                    .expect("userinfo request builder")
                    .request_async(&req_client)
                    .await
                    .unwrap_or_else(|error| panic!("{} userinfo failed: {error}", case.slug));
                assert_eq!(
                    info.email().map(|email| email.as_str()),
                    case.userinfo_email
                );
                assert_eq!(
                    info.name()
                        .and_then(|name| name.get(None).map(|value| value.to_string())),
                    Some(case.display_name.to_string())
                );
            }
        }
    }
}
