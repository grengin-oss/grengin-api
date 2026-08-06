// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use reqwest::Url;

/// Returns the SSO proxy URL (where the Cloudflare worker is deployed).
pub fn sso_proxy_url() -> String {
    std::env::var("SSO_PROXY_URL")
        .unwrap_or_else(|_| "https://sso.grengin.com".to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Returns the JWKS URL for verifying SSO proxy assertions.
pub fn sso_proxy_jwks_url() -> String {
    std::env::var("SSO_PROXY_JWKS_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("{}/.well-known/jwks.json", sso_proxy_url()))
}

/// Build the worker authorize URL for SSO proxy mode.
///
/// Query params:
/// - redirect_uri: Grengin API callback URL (e.g. https://app.example.com/auth/google/callback)
/// - state: CSRF nonce generated and stored by the API
/// - nonce: OIDC nonce generated and stored by the API
pub fn build_proxy_authorize_url(
    provider: &str,
    callback_redirect_uri: &str,
    state: &str,
    nonce: &str,
) -> Option<String> {
    let base = sso_proxy_url();
    let mut url = Url::parse(&format!(
        "{}/authorize/{}",
        base,
        provider.to_ascii_lowercase()
    ))
    .ok()?;
    url.query_pairs_mut()
        .append_pair("redirect_uri", callback_redirect_uri)
        .append_pair("state", state)
        .append_pair("nonce", nonce);
    Some(url.to_string())
}
