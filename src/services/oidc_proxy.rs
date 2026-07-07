use crate::{
    auth::{error::AuthError, sso_proxy::sso_proxy_jwks_url},
    dto::oauth::AuthProvider,
    state::SharedState,
};
use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{Jwk, JwkSet},
};
use serde::Deserialize;

#[allow(dead_code)]
#[derive(Deserialize)]
pub struct ProxyAssertionClaims {
    pub aud: String,
    pub email: Option<String>,
    pub exp: u64,
    pub iat: u64,
    pub iss: String,
    pub name: Option<String>,
    pub nonce: Option<String>,
    pub picture: Option<String>,
    pub provider: String,
    pub provider_sub: Option<String>,
}

pub async fn provider_uses_proxy(app_state: &SharedState, provider: &AuthProvider) -> bool {
    match provider.to_lowercase().as_str() {
        "google" => app_state
            .settings
            .google
            .read()
            .await
            .as_ref()
            .map(|s| s.use_grengin_proxy)
            .unwrap_or(false),
        "azure" => app_state
            .settings
            .azure
            .read()
            .await
            .as_ref()
            .map(|s| s.use_grengin_proxy)
            .unwrap_or(false),
        _ => false,
    }
}

fn select_assertion_jwk<'a>(jwks: &'a JwkSet, kid: Option<&str>) -> Option<&'a Jwk> {
    if let Some(kid) = kid {
        if let Some(found) = jwks.find(kid) {
            return Some(found);
        }
    }
    jwks.keys.iter().find(|key| key.common.key_id.is_some()).or_else(|| jwks.keys.first())
}

pub async fn verify_proxy_assertion(
    app_state: &SharedState,
    provider: &AuthProvider,
    assertion: &str,
    expected_nonce: &str,
    expected_audience: &str,
) -> Result<ProxyAssertionClaims, AuthError> {
    let header = decode_header(assertion).map_err(|e| {
        eprintln!("proxy assertion header decode error: {e:?}");
        AuthError::InvalidToken
    })?;
    if header.alg != Algorithm::EdDSA {
        return Err(AuthError::InvalidToken);
    }

    let jwks_url = sso_proxy_jwks_url();
    let jwks = app_state
        .req_client
        .get(jwks_url)
        .send()
        .await
        .map_err(|e| {
            eprintln!("proxy jwks fetch error: {e:?}");
            AuthError::ServiceTemporarilyUnavailable
        })?
        .error_for_status()
        .map_err(|e| {
            eprintln!("proxy jwks http error: {e:?}");
            AuthError::ServiceTemporarilyUnavailable
        })?
        .json::<JwkSet>()
        .await
        .map_err(|e| {
            eprintln!("proxy jwks parse error: {e:?}");
            AuthError::ServiceTemporarilyUnavailable
        })?;
    let jwk = select_assertion_jwk(&jwks, header.kid.as_deref()).ok_or_else(|| {
        eprintln!("proxy jwks key selection failed for kid={:?}", header.kid);
        AuthError::InvalidToken
    })?;
    let key = DecodingKey::from_jwk(jwk).map_err(|e| {
        eprintln!("proxy jwk decoding key error: {e:?}");
        AuthError::InvalidToken
    })?;

    let mut validation = Validation::new(Algorithm::EdDSA);
    validation.set_required_spec_claims(&["exp", "aud", "iss", "sub"]);
    validation.set_audience(&[expected_audience]);
    let expected_issuer = crate::auth::sso_proxy::sso_proxy_url();
    validation.set_issuer(&[&expected_issuer]);

    let token = decode::<ProxyAssertionClaims>(assertion, &key, &validation).map_err(|e| {
        eprintln!("proxy assertion validation error: {e:?}");
        AuthError::InvalidToken
    })?;
    let claims = token.claims;

    if !claims.provider.eq_ignore_ascii_case(provider) {
        return Err(AuthError::InvalidToken);
    }
    if claims.nonce.as_deref() != Some(expected_nonce) {
        return Err(AuthError::InvalidToken);
    }

    Ok(claims)
}
