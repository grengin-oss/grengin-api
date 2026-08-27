// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::provider_config::build_discovered_oidc_client, config::setting::OidcClient,
    utils::uri::is_azure_mobile_redirect_uri,
};
use anyhow::{Context, Result, anyhow};
use openidconnect::{
    AuthorizationCode, ClientId, CsrfToken, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, TokenResponse,
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata, CoreTokenResponse},
};
use reqwest::{Client as ReqwestClient, StatusCode, Url, header::LOCATION};
use std::{borrow::Cow, env};

const CLIENT_ID: &str = "grengin-mock-client";
const CLIENT_SECRET: &str = "grengin-mock-secret";
const WEB_REDIRECT_URI: &str = "http://127.0.0.1:18081/auth/mock/callback";
const ANDROID_REDIRECT_URI: &str = "msauth://com.grengin.mobile/6%2FaB1cD2eF3gH4iJ5kL6-mN7oP8qR%3D";
const IOS_REDIRECT_URI: &str = "msauth.com.grengin.mobile://auth";

struct PendingAuthorization {
    client: OidcClient,
    http_client: ReqwestClient,
    code: String,
    nonce: Nonce,
    pkce_verifier: String,
    redirect_uri: String,
}

fn mock_issuer() -> String {
    env::var("MOCK_OAUTH2_ISSUER").expect("MOCK_OAUTH2_ISSUER is set by tests/auth/mock_oauth2.sh")
}

fn http_client() -> ReqwestClient {
    ReqwestClient::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("mock OAuth HTTP client")
}

async fn mock_client(
    redirect_uri: &str,
    public_client: bool,
) -> Result<(OidcClient, ReqwestClient)> {
    let http_client = http_client();
    let issuer = mock_issuer();
    let client = if public_client {
        let metadata =
            CoreProviderMetadata::discover_async(IssuerUrl::new(issuer)?, &http_client).await?;
        CoreClient::from_provider_metadata(metadata, ClientId::new(CLIENT_ID.to_string()), None)
            .set_redirect_uri(RedirectUrl::new(redirect_uri.to_string())?)
    } else {
        build_discovered_oidc_client(
            &http_client,
            &issuer,
            CLIENT_ID.to_string(),
            CLIENT_SECRET.to_string(),
            redirect_uri.to_string(),
        )
        .await?
    };
    Ok((client, http_client))
}

async fn begin_authorization(
    redirect_uri: &str,
    public_client: bool,
) -> Result<PendingAuthorization> {
    let (client, http_client) = mock_client(redirect_uri, public_client).await?;
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (authorization_url, csrf_state, nonce) = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .set_pkce_challenge(pkce_challenge)
        .url();
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
    if callback.scheme() != Url::parse(redirect_uri)?.scheme()
        || !location.starts_with(redirect_uri)
    {
        return Err(anyhow!("mock callback did not preserve the redirect URI"));
    }

    Ok(PendingAuthorization {
        client,
        http_client,
        code,
        nonce,
        pkce_verifier: pkce_verifier.secret().to_string(),
        redirect_uri: redirect_uri.to_string(),
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

async fn assert_successful_flow(redirect_uri: &str, public_client: bool) -> Result<()> {
    let pending = begin_authorization(redirect_uri, public_client).await?;
    let token_response = exchange(&pending, &pending.pkce_verifier).await?;
    let id_token = token_response
        .id_token()
        .context("mock token response has no ID token")?;
    let claims = id_token.claims(&pending.client.id_token_verifier(), &pending.nonce)?;

    assert_eq!(claims.subject().as_str(), "mock-user-123");
    assert_eq!(
        claims.email().map(|email| email.as_str()),
        Some("mock.user@example.com")
    );
    assert_eq!(
        claims.email_verified(),
        Some(true),
        "mock email must be safe for verified-email linking"
    );

    let replay = exchange(&pending, &pending.pkce_verifier).await;
    assert!(replay.is_err(), "authorization codes must be single-use");
    Ok(())
}

#[tokio::test]
#[ignore = "requires tests/auth/mock_oauth2.sh"]
async fn web_authorization_code_flow_verifies_pkce_nonce_jwks_and_replay() -> Result<()> {
    assert_successful_flow(WEB_REDIRECT_URI, false).await
}

#[tokio::test]
#[ignore = "requires tests/auth/mock_oauth2.sh"]
async fn android_deep_link_completes_public_client_flow() -> Result<()> {
    assert!(is_azure_mobile_redirect_uri(
        &"azure".to_string(),
        ANDROID_REDIRECT_URI
    ));
    assert_successful_flow(ANDROID_REDIRECT_URI, true).await
}

#[tokio::test]
#[ignore = "requires tests/auth/mock_oauth2.sh"]
async fn ios_deep_link_completes_public_client_flow() -> Result<()> {
    assert!(is_azure_mobile_redirect_uri(
        &"azure".to_string(),
        IOS_REDIRECT_URI
    ));
    assert_successful_flow(IOS_REDIRECT_URI, true).await
}

#[tokio::test]
#[ignore = "requires tests/auth/mock_oauth2.sh"]
async fn token_endpoint_rejects_pkce_mismatch_and_consumes_the_code() -> Result<()> {
    let pending = begin_authorization(WEB_REDIRECT_URI, false).await?;
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
