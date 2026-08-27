// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::provider_config::{OidcProviderConfiguration, build_discovered_oidc_client},
    config::setting::OidcClient,
    utils::uri::is_azure_mobile_redirect_uri,
};
use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use openidconnect::{
    AuthorizationCode, ClientId, CsrfToken, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, Scope, TokenResponse,
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata, CoreTokenResponse},
};
use reqwest::{Client as ReqwestClient, StatusCode, Url, header::LOCATION};
use serde_json::Value;
use std::{borrow::Cow, env};

const CLIENT_ID: &str = "grengin-mock-client";
const CLIENT_SECRET: &str = "grengin-mock-secret";
const WEB_REDIRECT_URI: &str = "http://127.0.0.1:18081/auth/mock/callback";
const ANDROID_REDIRECT_URI: &str = "msauth://com.grengin.mobile/6%2FaB1cD2eF3gH4iJ5kL6-mN7oP8qR%3D";
const IOS_REDIRECT_URI: &str = "msauth.com.grengin.mobile://auth";
struct FlowProfile {
    issuer_id: &'static str,
    redirect_uri: &'static str,
    public_client: bool,
    configuration: OidcProviderConfiguration,
    expected_sub: &'static str,
    expected_email: &'static str,
}

impl FlowProfile {
    fn grengin(redirect_uri: &'static str, public_client: bool) -> Self {
        Self {
            issuer_id: "grengin",
            redirect_uri,
            public_client,
            configuration: OidcProviderConfiguration::default(),
            expected_sub: "mock-user-123",
            expected_email: "mock.user@example.com",
        }
    }
}

struct PendingAuthorization {
    client: OidcClient,
    http_client: ReqwestClient,
    code: String,
    nonce: Nonce,
    pkce_verifier: String,
    redirect_uri: String,
    authorization_url: Url,
}

struct VerifiedFlow {
    authorization_url: Url,
    token_payload: Value,
}

fn mock_issuer(issuer_id: &str) -> String {
    let base_url = env::var("MOCK_OAUTH2_BASE_URL")
        .expect("MOCK_OAUTH2_BASE_URL is set by tests/auth/mock_oauth2.sh");
    format!("{}/{issuer_id}", base_url.trim_end_matches('/'))
}

fn http_client() -> ReqwestClient {
    ReqwestClient::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("mock OAuth HTTP client")
}

async fn mock_client(profile: &FlowProfile) -> Result<(OidcClient, ReqwestClient)> {
    let http_client = http_client();
    let issuer = mock_issuer(profile.issuer_id);
    let client = if profile.public_client {
        let metadata =
            CoreProviderMetadata::discover_async(IssuerUrl::new(issuer)?, &http_client).await?;
        CoreClient::from_provider_metadata(metadata, ClientId::new(CLIENT_ID.to_string()), None)
            .set_redirect_uri(RedirectUrl::new(profile.redirect_uri.to_string())?)
    } else {
        build_discovered_oidc_client(
            &http_client,
            &issuer,
            CLIENT_ID.to_string(),
            CLIENT_SECRET.to_string(),
            profile.redirect_uri.to_string(),
        )
        .await?
    };
    Ok((client, http_client))
}

async fn begin_authorization(profile: &FlowProfile) -> Result<PendingAuthorization> {
    let (client, http_client) = mock_client(profile).await?;
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let mut authorization = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .set_pkce_challenge(pkce_challenge);
    for scope in &profile.configuration.scopes {
        if scope != "openid" {
            authorization = authorization.add_scope(Scope::new(scope.clone()));
        }
    }
    for (key, value) in &profile.configuration.authorization_params {
        authorization = authorization.add_extra_param(key.clone(), value.clone());
    }
    let (authorization_url, csrf_state, nonce) = authorization.url();
    let response = http_client.get(authorization_url.as_str()).send().await?;
    if response.status() != StatusCode::FOUND {
        return Err(anyhow!(
            "mock authorization returned {}, expected 302",
            response.status()
        ));
    }
    let location = response
        .headers()
        .get(LOCATION)
        .context("mock authorization response has no Location header")?
        .to_str()?;
    let callback = Url::parse(location)?;
    let callback_state = callback
        .query_pairs()
        .find_map(|(key, value)| (key == "state").then(|| value.into_owned()))
        .context("mock callback has no state")?;
    let code = callback
        .query_pairs()
        .find_map(|(key, value)| (key == "code").then(|| value.into_owned()))
        .context("mock callback has no authorization code")?;

    if callback_state != csrf_state.secret().as_str() {
        return Err(anyhow!("mock callback returned the wrong OAuth state"));
    }
    if callback.scheme() != Url::parse(profile.redirect_uri)?.scheme()
        || !location.starts_with(profile.redirect_uri)
    {
        return Err(anyhow!("mock callback did not preserve the redirect URI"));
    }

    Ok(PendingAuthorization {
        client,
        http_client,
        code,
        nonce,
        pkce_verifier: pkce_verifier.secret().to_string(),
        redirect_uri: profile.redirect_uri.to_string(),
        authorization_url,
    })
}

async fn exchange(
    pending: &PendingAuthorization,
    pkce_verifier: &str,
) -> Result<CoreTokenResponse> {
    Ok(pending
        .client
        .exchange_code(AuthorizationCode::new(pending.code.clone()))?
        .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier.to_string()))
        .set_redirect_uri(Cow::Owned(RedirectUrl::new(pending.redirect_uri.clone())?))
        .request_async(&pending.http_client)
        .await?)
}

fn jwt_payload(token: &str) -> Result<Value> {
    let payload = token
        .split('.')
        .nth(1)
        .context("mock ID token is malformed")?;
    Ok(serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload)?)?)
}

async fn assert_successful_flow(profile: &FlowProfile) -> Result<VerifiedFlow> {
    let pending = begin_authorization(profile).await?;
    let token_response = exchange(&pending, &pending.pkce_verifier).await?;
    let id_token = token_response
        .id_token()
        .context("mock token response has no ID token")?;
    let claims = id_token.claims(&pending.client.id_token_verifier(), &pending.nonce)?;

    assert_eq!(claims.subject().as_str(), profile.expected_sub);
    assert_eq!(
        claims.email().map(|email| email.as_str()),
        Some(profile.expected_email)
    );
    assert_eq!(
        claims.email_verified(),
        Some(true),
        "mock email must be safe for verified-email linking"
    );

    let replay = exchange(&pending, &pending.pkce_verifier).await;
    assert!(replay.is_err(), "authorization codes must be single-use");
    Ok(VerifiedFlow {
        authorization_url: pending.authorization_url,
        token_payload: jwt_payload(&id_token.to_string())?,
    })
}

fn authorization_parameter<'a>(url: &'a Url, name: &str) -> Option<std::borrow::Cow<'a, str>> {
    url.query_pairs()
        .find_map(|(key, value)| (key == name).then_some(value))
}

#[tokio::test]
#[ignore = "requires tests/auth/mock_oauth2.sh"]
async fn web_authorization_code_flow_verifies_pkce_nonce_jwks_and_replay() -> Result<()> {
    assert_successful_flow(&FlowProfile::grengin(WEB_REDIRECT_URI, false))
        .await
        .map(|_| ())
}

#[tokio::test]
#[ignore = "requires tests/auth/mock_oauth2.sh"]
async fn android_deep_link_completes_public_client_flow() -> Result<()> {
    assert!(is_azure_mobile_redirect_uri(
        &"azure".to_string(),
        ANDROID_REDIRECT_URI
    ));
    assert_successful_flow(&FlowProfile::grengin(ANDROID_REDIRECT_URI, true))
        .await
        .map(|_| ())
}

#[tokio::test]
#[ignore = "requires tests/auth/mock_oauth2.sh"]
async fn ios_deep_link_completes_public_client_flow() -> Result<()> {
    assert!(is_azure_mobile_redirect_uri(
        &"azure".to_string(),
        IOS_REDIRECT_URI
    ));
    assert_successful_flow(&FlowProfile::grengin(IOS_REDIRECT_URI, true))
        .await
        .map(|_| ())
}

#[tokio::test]
#[ignore = "requires tests/auth/mock_oauth2.sh"]
async fn auth0_profile_supports_audience_offline_access_and_namespaced_claims() -> Result<()> {
    let configuration = OidcProviderConfiguration::from_value(Some(&serde_json::json!({
        "version": "1.0",
        "scopes": ["openid", "profile", "email", "offline_access"],
        "authorizationParams": {
            "audience": "https://api.grengin.test",
            "organization": "org_mock"
        },
        "emailLinking": "verifiedEmail",
        "autoRedirect": false
    })))?;
    let profile = FlowProfile {
        issuer_id: "auth0",
        redirect_uri: WEB_REDIRECT_URI,
        public_client: false,
        configuration,
        expected_sub: "auth0|mock-user-123",
        expected_email: "auth0.user@example.com",
    };
    let verified = assert_successful_flow(&profile).await?;

    assert_eq!(
        authorization_parameter(&verified.authorization_url, "audience").as_deref(),
        Some("https://api.grengin.test")
    );
    assert_eq!(
        authorization_parameter(&verified.authorization_url, "organization").as_deref(),
        Some("org_mock")
    );
    let scopes = authorization_parameter(&verified.authorization_url, "scope")
        .context("Auth0 authorization request has no scope")?;
    assert!(scopes.split(' ').any(|scope| scope == "offline_access"));
    assert_eq!(
        verified.token_payload["https://grengin.com/roles"],
        serde_json::json!(["member", "billing-admin"])
    );
    assert_eq!(verified.token_payload["org_id"], "org_mock");
    Ok(())
}

#[tokio::test]
#[ignore = "requires tests/auth/mock_oauth2.sh"]
async fn keycloak_profile_supports_groups_realm_roles_and_idp_hint() -> Result<()> {
    let configuration = OidcProviderConfiguration::from_value(Some(&serde_json::json!({
        "version": "1.0",
        "scopes": ["openid", "profile", "email", "groups"],
        "authorizationParams": {"kc_idp_hint": "corporate"},
        "emailLinking": "verifiedEmail",
        "autoRedirect": false
    })))?;
    let profile = FlowProfile {
        issuer_id: "keycloak",
        redirect_uri: WEB_REDIRECT_URI,
        public_client: false,
        configuration,
        expected_sub: "keycloak-user-123",
        expected_email: "keycloak.user@example.com",
    };
    let verified = assert_successful_flow(&profile).await?;

    assert_eq!(
        authorization_parameter(&verified.authorization_url, "kc_idp_hint").as_deref(),
        Some("corporate")
    );
    let scopes = authorization_parameter(&verified.authorization_url, "scope")
        .context("Keycloak authorization request has no scope")?;
    assert!(scopes.split(' ').any(|scope| scope == "groups"));
    assert_eq!(
        verified.token_payload["realm_access"]["roles"],
        serde_json::json!(["grengin-user", "project-admin"])
    );
    assert_eq!(
        verified.token_payload["groups"],
        serde_json::json!(["/Engineering/Platform"])
    );
    assert_eq!(
        verified.token_payload["preferred_username"],
        "keycloak.user"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires tests/auth/mock_oauth2.sh"]
async fn token_endpoint_rejects_pkce_mismatch_and_consumes_the_code() -> Result<()> {
    let pending = begin_authorization(&FlowProfile::grengin(WEB_REDIRECT_URI, false)).await?;
    let (_, wrong_verifier) = PkceCodeChallenge::new_random_sha256();

    assert!(
        exchange(&pending, wrong_verifier.secret()).await.is_err(),
        "token endpoint accepted a mismatched PKCE verifier"
    );
    assert!(
        exchange(&pending, &pending.pkce_verifier).await.is_err(),
        "authorization code survived a failed PKCE exchange"
    );
    Ok(())
}
