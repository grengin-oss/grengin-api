use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;

const PROXY_STATE_TTL_SECS: i64 = 3600; // 1 hour — matches Google/Azure auth timeout

#[derive(Serialize)]
struct ProxyStateClaims<'a> {
    /// Inner CSRF nonce — stored as oauth_sessions.state on the instance
    n: &'a str,
    /// Origin URL of the Grengin instance, e.g. https://acme.grengin.com
    o: &'a str,
    iat: i64,
    exp: i64,
}

/// Returns the SSO proxy URL (where the Cloudflare worker is deployed).
pub fn sso_proxy_url() -> String {
    std::env::var("SSO_PROXY_URL")
        .unwrap_or_else(|_| "https://sso.grengin.com".to_string())
}

/// Returns the shared HMAC secret used to sign proxy state JWTs, if configured.
pub fn sso_proxy_shared_secret() -> Option<String> {
    std::env::var("SSO_PROXY_SHARED_SECRET")
        .ok()
        .filter(|s| !s.is_empty())
}

/// Build a signed JWT to use as the OAuth `state` parameter in proxy mode.
///
/// The JWT encodes the inner CSRF nonce (which gets stored in oauth_sessions.state
/// on the instance) and the instance's public origin. The Cloudflare worker at
/// sso.grengin.com verifies this JWT, extracts the origin, and redirects the
/// OAuth callback to the correct instance.
pub fn build_proxy_state_jwt(inner_nonce: &str, instance_origin: &str, secret: &str) -> Option<String> {
    let now = chrono::Utc::now().timestamp();
    let claims = ProxyStateClaims {
        n: inner_nonce,
        o: instance_origin,
        iat: now,
        exp: now + PROXY_STATE_TTL_SECS,
    };
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .ok()
}

/// Rewrite the `state` query parameter in an authorization URL.
///
/// The openidconnect crate generates a random CSRF token and embeds it as
/// `state=`. In proxy mode we replace that value with our signed JWT so the
/// Cloudflare worker can extract the instance origin on the callback.
pub fn replace_state_in_url(auth_url: url::Url, new_state: &str) -> url::Url {
    let params: Vec<(String, String)> = auth_url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    let mut out = auth_url.clone();
    out.set_query(None);
    for (k, v) in params {
        if k == "state" {
            out.query_pairs_mut().append_pair("state", new_state);
        } else {
            out.query_pairs_mut().append_pair(&k, &v);
        }
    }
    out
}
