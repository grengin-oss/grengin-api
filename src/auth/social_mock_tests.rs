// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::auth::provider_config::{OidcProviderConfiguration, build_discovered_oidc_client};
use anyhow::{Context, Result, anyhow, bail};
use openidconnect::{
    AuthorizationCode, CsrfToken, Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
    TokenResponse, core::CoreAuthenticationFlow,
};
use quick_xml::{Reader, events::Event};
use reqwest::{Client as ReqwestClient, StatusCode, Url, header::LOCATION};
use std::{borrow::Cow, collections::HashMap, env};

const CLIENT_ID: &str = "grengin-social-mock-client";
const CLIENT_SECRET: &str = "grengin-social-mock-secret";
const LINKEDIN_REDIRECT_URI: &str = "http://127.0.0.1:18081/auth/linkedin/callback";
const APPLE_REDIRECT_URI: &str = "https://login.grengin.test/auth/apple/callback";

#[derive(Clone, Copy)]
enum CallbackMode {
    Query,
    FormPost,
}

struct ProviderProfile {
    issuer_id: &'static str,
    redirect_uri: &'static str,
    scopes: &'static [&'static str],
    authorization_params: &'static [(&'static str, &'static str)],
    callback_mode: CallbackMode,
    expected_sub: &'static str,
    expected_email: &'static str,
}

struct PendingAuthorization {
    client: crate::config::setting::OidcClient,
    http_client: ReqwestClient,
    code: String,
    nonce: Nonce,
    pkce_verifier: String,
    redirect_uri: String,
    authorization_url: Url,
}

fn mock_issuer(issuer_id: &str) -> String {
    let base_url = env::var("MOCK_OAUTH2_BASE_URL")
        .expect("MOCK_OAUTH2_BASE_URL is set by tests/auth/mock_social_oauth.sh");
    format!("{}/{issuer_id}", base_url.trim_end_matches('/'))
}

fn http_client() -> ReqwestClient {
    ReqwestClient::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("mock OAuth HTTP client")
}

fn linkedin_profile() -> ProviderProfile {
    ProviderProfile {
        issuer_id: "linkedin",
        redirect_uri: LINKEDIN_REDIRECT_URI,
        scopes: &["openid", "profile", "email"],
        authorization_params: &[],
        callback_mode: CallbackMode::Query,
        expected_sub: "linkedin-user-123",
        expected_email: "linkedin.user@example.com",
    }
}

fn apple_profile() -> ProviderProfile {
    ProviderProfile {
        issuer_id: "apple",
        redirect_uri: APPLE_REDIRECT_URI,
        scopes: &["openid", "email", "name"],
        authorization_params: &[("response_mode", "form_post")],
        callback_mode: CallbackMode::FormPost,
        expected_sub: "apple-user-123",
        expected_email: "apple.user@privaterelay.appleid.com",
    }
}

fn configuration(profile: &ProviderProfile) -> Result<OidcProviderConfiguration> {
    let authorization_params = profile
        .authorization_params
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    let configuration = OidcProviderConfiguration {
        scopes: profile
            .scopes
            .iter()
            .map(|scope| scope.to_string())
            .collect(),
        authorization_params,
        ..Default::default()
    };
    configuration.validate_for_provider(profile.issuer_id)?;
    Ok(configuration)
}

fn query_value(url: &Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find_map(|(key, value)| (key == name).then(|| value.into_owned()))
}

fn parse_form_post(body: &str) -> Result<(String, HashMap<String, String>)> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    reader.config_mut().check_end_names = false;
    let mut action = None;
    let mut method = None;
    let mut fields = HashMap::new();

    loop {
        match reader.read_event()? {
            Event::Start(element) | Event::Empty(element) => {
                let element_name = element.name();
                if element_name.as_ref() == b"form" {
                    for attribute in element.attributes() {
                        let attribute = attribute?;
                        let value = attribute
                            .decoded_and_normalized_value(
                                quick_xml::XmlVersion::Implicit1_0,
                                reader.decoder(),
                            )?
                            .into_owned();
                        match attribute.key.as_ref() {
                            b"action" => action = Some(value),
                            b"method" => method = Some(value),
                            _ => {}
                        }
                    }
                } else if element_name.as_ref() == b"input" {
                    let mut name = None;
                    let mut value = None;
                    for attribute in element.attributes() {
                        let attribute = attribute?;
                        let decoded = attribute
                            .decoded_and_normalized_value(
                                quick_xml::XmlVersion::Implicit1_0,
                                reader.decoder(),
                            )?
                            .into_owned();
                        match attribute.key.as_ref() {
                            b"name" => name = Some(decoded),
                            b"value" => value = Some(decoded),
                            _ => {}
                        }
                    }
                    if let (Some(name), Some(value)) = (name, value) {
                        fields.insert(name, value);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    if !method.is_some_and(|method| method.eq_ignore_ascii_case("post")) {
        bail!("mock callback form did not use POST");
    }
    Ok((action.context("mock callback form has no action")?, fields))
}

async fn begin_authorization(profile: &ProviderProfile) -> Result<PendingAuthorization> {
    let http_client = http_client();
    let client = build_discovered_oidc_client(
        &http_client,
        &mock_issuer(profile.issuer_id),
        CLIENT_ID.to_string(),
        CLIENT_SECRET.to_string(),
        profile.redirect_uri.to_string(),
    )
    .await?;
    let configuration = configuration(profile)?;
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let mut authorization = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .set_pkce_challenge(pkce_challenge);
    for scope in configuration.scopes {
        if scope != "openid" {
            authorization = authorization.add_scope(Scope::new(scope));
        }
    }
    for (key, value) in configuration.authorization_params {
        authorization = authorization.add_extra_param(key, value);
    }
    let (authorization_url, csrf_state, nonce) = authorization.url();

    let scope = query_value(&authorization_url, "scope").context("authorization scope")?;
    let requested_scopes: std::collections::HashSet<_> = scope.split_whitespace().collect();
    for expected in profile.scopes {
        if !requested_scopes.contains(expected) {
            bail!("{} authorization omitted {expected}", profile.issuer_id);
        }
    }
    for (key, value) in profile.authorization_params {
        if query_value(&authorization_url, key).as_deref() != Some(*value) {
            bail!("{} authorization omitted {key}", profile.issuer_id);
        }
    }

    let response = http_client.get(authorization_url.clone()).send().await?;
    let (callback_url, callback_fields) = match profile.callback_mode {
        CallbackMode::Query => {
            if response.status() != StatusCode::FOUND {
                bail!(
                    "mock authorization returned {}, expected 302",
                    response.status()
                );
            }
            let location = response
                .headers()
                .get(LOCATION)
                .context("mock authorization response has no Location header")?
                .to_str()?;
            let mut callback = Url::parse(location)?;
            let fields = callback.query_pairs().into_owned().collect();
            callback.set_query(None);
            (callback.to_string(), fields)
        }
        CallbackMode::FormPost => {
            if response.status() != StatusCode::OK {
                bail!(
                    "mock form-post returned {}, expected 200",
                    response.status()
                );
            }
            parse_form_post(&response.text().await?)?
        }
    };

    if callback_url != profile.redirect_uri {
        bail!("mock callback did not preserve the exact redirect URI");
    }
    if callback_fields.get("state") != Some(csrf_state.secret()) {
        bail!("mock callback returned the wrong OAuth state");
    }
    let code = callback_fields
        .get("code")
        .cloned()
        .context("mock callback has no authorization code")?;

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

async fn run_flow(profile: ProviderProfile) -> Result<()> {
    let pending = begin_authorization(&profile).await?;
    let token_response = pending
        .client
        .exchange_code(AuthorizationCode::new(pending.code.clone()))?
        .set_pkce_verifier(PkceCodeVerifier::new(pending.pkce_verifier.clone()))
        .set_redirect_uri(Cow::Owned(RedirectUrl::new(pending.redirect_uri.clone())?))
        .request_async(&pending.http_client)
        .await?;
    let id_token = token_response
        .id_token()
        .context("mock token response has no ID token")?;
    let claims = id_token.claims(&pending.client.id_token_verifier(), &pending.nonce)?;

    if claims.subject().as_str() != profile.expected_sub {
        bail!("{} returned the wrong subject", profile.issuer_id);
    }
    if claims.email().map(|email| email.as_str()) != Some(profile.expected_email) {
        bail!("{} returned the wrong email", profile.issuer_id);
    }
    if claims.email_verified() != Some(true) {
        bail!("{} email is not verified", profile.issuer_id);
    }

    let replay = pending
        .client
        .exchange_code(AuthorizationCode::new(pending.code))?
        .set_pkce_verifier(PkceCodeVerifier::new(pending.pkce_verifier))
        .set_redirect_uri(Cow::Owned(RedirectUrl::new(pending.redirect_uri)?))
        .request_async(&pending.http_client)
        .await;
    if replay.is_ok() {
        return Err(anyhow!(
            "{} authorization code was replayable",
            profile.issuer_id
        ));
    }

    assert!(query_value(&pending.authorization_url, "code_challenge").is_some());
    assert_eq!(
        query_value(&pending.authorization_url, "code_challenge_method").as_deref(),
        Some("S256")
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires tests/auth/mock_social_oauth.sh"]
async fn linkedin_oidc_round_trip_matches_current_provider_contract() -> Result<()> {
    run_flow(linkedin_profile()).await
}

#[tokio::test]
#[ignore = "requires tests/auth/mock_social_oauth.sh"]
async fn apple_oidc_core_returns_urlencoded_form_post() -> Result<()> {
    run_flow(apple_profile()).await
}
