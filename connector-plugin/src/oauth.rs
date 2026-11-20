// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OAuthExpirySource {
    #[default]
    ExpiresIn,
    JwtExp,
}

#[derive(Debug, thiserror::Error)]
#[error("invalid OAuth token expiry")]
pub struct InvalidOAuthExpiry;

impl OAuthExpirySource {
    pub fn expiry_hint(
        self,
        token: &str,
        now_unix: i64,
    ) -> Result<Option<i64>, InvalidOAuthExpiry> {
        if self == Self::ExpiresIn {
            return Ok(None);
        }
        if token.len() > 16384 {
            return Err(InvalidOAuthExpiry);
        }
        let mut segments = token.split('.');
        let header = segments.next().ok_or(InvalidOAuthExpiry)?;
        let payload = segments.next().ok_or(InvalidOAuthExpiry)?;
        let signature = segments.next().ok_or(InvalidOAuthExpiry)?;
        if header.is_empty()
            || payload.is_empty()
            || signature.is_empty()
            || segments.next().is_some()
        {
            return Err(InvalidOAuthExpiry);
        }
        #[derive(Deserialize)]
        struct Lifetime {
            exp: i64,
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| InvalidOAuthExpiry)?;
        let lifetime: Lifetime = serde_json::from_slice(&bytes).map_err(|_| InvalidOAuthExpiry)?;
        if lifetime.exp <= now_unix || lifetime.exp <= 0 {
            return Err(InvalidOAuthExpiry);
        }
        // This is a refresh scheduling hint from a TLS token response, never identity/signature validation.
        Ok(Some(lifetime.exp))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(body: &str) -> String {
        format!("e30.{}.fixture-signature", URL_SAFE_NO_PAD.encode(body))
    }

    #[test]
    fn default_mode_preserves_opaque_access_tokens() {
        assert_eq!(
            OAuthExpirySource::ExpiresIn
                .expiry_hint("opaque", 100)
                .unwrap(),
            None
        );
    }

    #[test]
    fn explicit_jwt_mode_reads_future_expiry_without_authorizing_identity() {
        assert_eq!(
            OAuthExpirySource::JwtExp
                .expiry_hint(&token(r#"{"exp":200,"sub":"not-used-for-auth"}"#), 100)
                .unwrap(),
            Some(200)
        );
    }

    #[test]
    fn malformed_expired_duplicate_and_oversized_expiry_hints_fail_closed() {
        for body in [
            r#"{"exp":100}"#,
            r#"{"exp":-1}"#,
            r#"{"exp":0}"#,
            r#"{"exp":null}"#,
            r#"{"exp":"200"}"#,
            r#"{"exp":200.5}"#,
            r#"{"exp":200,"exp":300}"#,
            r#"{"sub":"x"}"#,
            r#"{"exp":18446744073709551615}"#,
        ] {
            assert!(
                OAuthExpirySource::JwtExp
                    .expiry_hint(&token(body), 100)
                    .is_err(),
                "{body}"
            );
        }
        for value in [
            "opaque".to_string(),
            "a.b.c.d".to_string(),
            "a.!.c".to_string(),
            "a.b.".to_string(),
            "x".repeat(16385),
        ] {
            assert!(OAuthExpirySource::JwtExp.expiry_hint(&value, 100).is_err());
        }
    }
}
