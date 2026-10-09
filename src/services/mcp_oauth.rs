// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use reqwest::Url;
use uuid::Uuid;

use crate::{
    auth::encryption::{CryptoError, decrypt_key, encrypt_key},
    error::AppError,
    models::mcp_oauth_states,
};

// Tokens written before encryption at rest have no prefix; the prefix lets a failed decrypt be
// reported as an error instead of the ciphertext being sent upstream as a bearer token.
const SEALED_TOKEN_PREFIX: &str = "enc:v1:";

pub fn seal_token(app_key: &[u8; 32], token: &str) -> Result<String, AppError> {
    encrypt_key(app_key, token.as_bytes())
        .map(|ciphertext| format!("{SEALED_TOKEN_PREFIX}{ciphertext}"))
        .map_err(|e| {
            eprintln!("mcp oauth token encryption error: {e:?}");
            AppError::ServiceTemporarilyUnavailable
        })
}

#[derive(Debug, PartialEq, Eq)]
pub enum StoredToken {
    Sealed(String),
    LegacyPlaintext(String),
}

impl StoredToken {
    pub fn open(app_key: &[u8; 32], stored: &str) -> Result<Self, CryptoError> {
        match stored.strip_prefix(SEALED_TOKEN_PREFIX) {
            Some(ciphertext) => decrypt_key(app_key, ciphertext).map(Self::Sealed),
            None => Ok(Self::LegacyPlaintext(stored.to_string())),
        }
    }

    pub fn secret(&self) -> &str {
        match self {
            Self::Sealed(token) | Self::LegacyPlaintext(token) => token,
        }
    }

    pub fn needs_sealing(&self) -> bool {
        matches!(self, Self::LegacyPlaintext(_))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum OAuthStateRejection {
    Expired,
    StartedByAnotherUser,
}

pub fn check_oauth_state(
    oauth_state: &mcp_oauth_states::Model,
    caller_user_id: Uuid,
    now: DateTime<Utc>,
) -> Result<(), OAuthStateRejection> {
    if oauth_state
        .expires_at
        .is_none_or(|expires_at| expires_at <= now)
    {
        return Err(OAuthStateRejection::Expired);
    }
    if oauth_state.user_id != caller_user_id {
        return Err(OAuthStateRejection::StartedByAnotherUser);
    }
    Ok(())
}

pub fn allowed_redirect_uri(candidate: &str, app_url: &str) -> Option<String> {
    // Browsers drop tabs/newlines and treat '\' as '/', so "/\evil.com" or "/\t/evil.com"
    // would leave the origin even though they look like relative paths.
    if candidate.is_empty()
        || candidate
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return None;
    }
    if candidate.starts_with('/') {
        return (!candidate.starts_with("//")).then(|| candidate.to_string());
    }
    let target = Url::parse(candidate).ok()?;
    let app = Url::parse(app_url).ok()?;
    let same_origin = matches!(target.scheme(), "http" | "https")
        && target.username().is_empty()
        && target.password().is_none()
        && target.origin() == app.origin();
    same_origin.then(|| target.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    const KEY: [u8; 32] = [7u8; 32];
    const APP_URL: &str = "https://app.example.com";

    fn oauth_state(user_id: Uuid, expires_at: Option<DateTime<Utc>>) -> mcp_oauth_states::Model {
        mcp_oauth_states::Model {
            id: Uuid::new_v4(),
            server_id: Uuid::new_v4(),
            user_id,
            state: "state".to_string(),
            pkce_verifier: "verifier".to_string(),
            redirect_uri: None,
            expires_at,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn sealed_tokens_round_trip_and_are_not_stored_in_plaintext() {
        let sealed = seal_token(&KEY, "gho_secret").expect("seal");
        assert!(!sealed.contains("gho_secret"));
        assert_eq!(
            StoredToken::open(&KEY, &sealed).expect("open"),
            StoredToken::Sealed("gho_secret".to_string())
        );
    }

    #[test]
    fn legacy_plaintext_tokens_still_open_and_are_flagged_for_sealing() {
        let opened = StoredToken::open(&KEY, "ya29.legacy-plaintext-token").expect("open");
        assert_eq!(opened.secret(), "ya29.legacy-plaintext-token");
        assert!(opened.needs_sealing());
    }

    #[test]
    fn legacy_tokens_that_look_like_base64_are_still_plaintext() {
        let opened = StoredToken::open(&KEY, "QUJDREVGR0hJSktMTU5PUFFSU1RVVldY").expect("open");
        assert_eq!(
            opened,
            StoredToken::LegacyPlaintext("QUJDREVGR0hJSktMTU5PUFFSU1RVVldY".to_string())
        );
    }

    #[test]
    fn sealed_tokens_under_another_key_fail_instead_of_leaking_ciphertext() {
        let sealed = seal_token(&[9u8; 32], "gho_secret").expect("seal");
        assert!(StoredToken::open(&KEY, &sealed).is_err());
    }

    #[test]
    fn sealed_tokens_do_not_need_resealing() {
        let sealed = seal_token(&KEY, "token").expect("seal");
        assert!(
            !StoredToken::open(&KEY, &sealed)
                .expect("open")
                .needs_sealing()
        );
    }

    #[test]
    fn oauth_state_is_accepted_for_the_user_who_started_it() {
        let user = Uuid::new_v4();
        let now = Utc::now();
        let row = oauth_state(user, Some(now + Duration::minutes(5)));
        assert_eq!(check_oauth_state(&row, user, now), Ok(()));
    }

    #[test]
    fn oauth_state_is_rejected_for_a_different_user() {
        let now = Utc::now();
        let row = oauth_state(Uuid::new_v4(), Some(now + Duration::minutes(5)));
        assert_eq!(
            check_oauth_state(&row, Uuid::new_v4(), now),
            Err(OAuthStateRejection::StartedByAnotherUser)
        );
    }

    #[test]
    fn expired_oauth_state_is_rejected_even_for_its_owner() {
        let user = Uuid::new_v4();
        let now = Utc::now();
        let row = oauth_state(user, Some(now - Duration::seconds(1)));
        assert_eq!(
            check_oauth_state(&row, user, now),
            Err(OAuthStateRejection::Expired)
        );
    }

    #[test]
    fn oauth_state_without_expiry_is_treated_as_expired() {
        let user = Uuid::new_v4();
        let row = oauth_state(user, None);
        assert_eq!(
            check_oauth_state(&row, user, Utc::now()),
            Err(OAuthStateRejection::Expired)
        );
    }

    #[test]
    fn relative_redirects_are_allowed() {
        assert_eq!(
            allowed_redirect_uri("/settings/integrations?tab=mcp", APP_URL),
            Some("/settings/integrations?tab=mcp".to_string())
        );
    }

    #[test]
    fn redirects_to_the_app_origin_are_allowed() {
        assert_eq!(
            allowed_redirect_uri("https://app.example.com/settings", APP_URL),
            Some("https://app.example.com/settings".to_string())
        );
        assert!(allowed_redirect_uri("HTTPS://APP.EXAMPLE.COM:443/x", APP_URL).is_some());
    }

    #[test]
    fn protocol_relative_redirects_are_rejected() {
        assert_eq!(allowed_redirect_uri("//evil.com", APP_URL), None);
        assert_eq!(allowed_redirect_uri("//evil.com/path", APP_URL), None);
    }

    #[test]
    fn backslash_and_whitespace_tricks_are_rejected() {
        assert_eq!(allowed_redirect_uri("/\\evil.com", APP_URL), None);
        assert_eq!(allowed_redirect_uri("/\t/evil.com", APP_URL), None);
        assert_eq!(allowed_redirect_uri(" //evil.com", APP_URL), None);
        assert_eq!(allowed_redirect_uri("/\n/evil.com", APP_URL), None);
    }

    #[test]
    fn other_origins_are_rejected() {
        assert_eq!(allowed_redirect_uri("https://evil.com/", APP_URL), None);
        assert_eq!(
            allowed_redirect_uri("https://app.example.com.evil.com/", APP_URL),
            None
        );
        assert_eq!(
            allowed_redirect_uri("https://app.example.com@evil.com/", APP_URL),
            None
        );
        assert_eq!(
            allowed_redirect_uri("https://app.example.com:8443/", APP_URL),
            None
        );
    }

    #[test]
    fn scheme_downgrade_and_script_urls_are_rejected() {
        assert_eq!(
            allowed_redirect_uri("http://app.example.com/settings", APP_URL),
            None
        );
        assert_eq!(allowed_redirect_uri("javascript:alert(1)", APP_URL), None);
        assert_eq!(
            allowed_redirect_uri("data:text/html,<script>alert(1)</script>", APP_URL),
            None
        );
    }

    #[test]
    fn credentials_in_an_app_origin_redirect_are_rejected() {
        assert_eq!(
            allowed_redirect_uri("https://user:pass@app.example.com/", APP_URL),
            None
        );
    }

    #[test]
    fn path_relative_and_empty_redirects_are_rejected() {
        assert_eq!(allowed_redirect_uri("settings", APP_URL), None);
        assert_eq!(allowed_redirect_uri("", APP_URL), None);
    }
}
