// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::auth::error::AuthError;
use crate::auth::jwt::KEYS;
use crate::services::auth_session::ensure_request_session;
use anyhow::Error;
use axum::{RequestPartsExt, extract::FromRequestParts, http::request::Parts};
use axum_extra::{
    TypedHeader,
    headers::{Authorization, authorization::Bearer},
};
use jsonwebtoken::{Validation, decode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

pub const ACCESS_TOKEN_TTL_SECS: u64 = 3600;
const REFRESH_TOKEN_TTL_SECS: u64 = 3600 * 24 * 7;
const LEGACY_ACCESS_TOKEN_CLOCK_SKEW_SECS: u64 = 60;

pub trait Claiming: Serialize + DeserializeOwned {
    fn get_token_string(&self) -> String {
        jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &self,
            &KEYS.get().unwrap().encoding,
        )
        .unwrap()
    }

    fn from_token_string(token: &str) -> Result<Self, Error> {
        let data = decode::<Self>(
            token,
            &KEYS.get().expect("JWT KEYS is not set").decoding,
            &Validation::default(),
        )?;
        Ok(data.claims)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenUse {
    Access,
    Refresh,
}

#[derive(Serialize)]
struct SessionTokenPayload<'a, T> {
    #[serde(flatten)]
    claims: &'a T,
    token_use: TokenUse,
}

#[derive(Debug, Deserialize)]
struct SessionTokenMarker {
    #[serde(default)]
    token_use: Option<TokenUse>,
    #[serde(default)]
    refresh: Option<bool>,
    exp: u64,
}

impl SessionTokenMarker {
    fn token_use(&self, now: u64) -> Option<TokenUse> {
        let legacy_refresh_flag = self.refresh == Some(true);
        match self.token_use {
            Some(TokenUse::Access) if self.refresh.is_some() => None,
            Some(TokenUse::Refresh) if !legacy_refresh_flag => None,
            Some(token_use) => Some(token_use),
            // Pre-`token_use` tokens: refresh ones carry `refresh: true`, access ones never did and live <= 1h.
            None if legacy_refresh_flag => Some(TokenUse::Refresh),
            None if self.refresh.is_none()
                && self.exp
                    <= now + ACCESS_TOKEN_TTL_SECS + LEGACY_ACCESS_TOKEN_CLOCK_SKEW_SECS =>
            {
                Some(TokenUse::Access)
            }
            None => None,
        }
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs()
}

fn encode_session_token<T: Serialize>(claims: &T, token_use: TokenUse) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &SessionTokenPayload { claims, token_use },
        &KEYS.get().expect("JWT KEYS is not set").encoding,
    )
    .expect("session token claims serialize to JSON")
}

fn decode_session_token<T: DeserializeOwned>(token: &str, expected: TokenUse) -> Result<T, Error> {
    let payload = decode::<serde_json::Value>(
        token,
        &KEYS.get().expect("JWT KEYS is not set").decoding,
        &Validation::default(),
    )?
    .claims;
    let marker = SessionTokenMarker::deserialize(&payload)?;
    anyhow::ensure!(
        marker.token_use(unix_now()) == Some(expected),
        "token is not a {expected:?} token"
    );
    Ok(serde_json::from_value(payload)?)
}

#[derive(Debug, Serialize, Deserialize, ToSchema, IntoParams)]
pub struct RefreshClaims {
    pub refresh: bool,
    pub sub: String,   // Email Subject (user identifier)
    pub user_id: Uuid, //user id
    pub exp: usize,    // Expiration time
}

impl Claiming for RefreshClaims {
    fn get_token_string(&self) -> String {
        encode_session_token(self, TokenUse::Refresh)
    }

    fn from_token_string(token: &str) -> Result<Self, Error> {
        decode_session_token(token, TokenUse::Refresh)
    }
}

impl RefreshClaims {
    pub fn new_refresh_token<S: Into<String>>(sub: S, user_id: Uuid) -> Self {
        Self {
            sub: sub.into(),
            refresh: true,
            user_id,
            exp: (unix_now() + REFRESH_TOKEN_TTL_SECS) as usize,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema, IntoParams)]
pub struct Claims {
    pub sub: String, // Email Subject (user identifier)
    pub name: Option<String>,
    pub user_id: Uuid, //user id
    pub exp: usize,    // Expiration time
}

impl Claiming for Claims {
    fn get_token_string(&self) -> String {
        encode_session_token(self, TokenUse::Access)
    }

    fn from_token_string(token: &str) -> Result<Self, Error> {
        decode_session_token(token, TokenUse::Access)
    }
}

impl Claims {
    pub fn new_access_token<S: Into<String>>(sub: S, name: Option<S>, user_id: Uuid) -> Self {
        Self {
            sub: sub.into(),
            name: name.map(|v| v.into()),
            user_id,
            exp: (unix_now() + ACCESS_TOKEN_TTL_SECS) as usize,
        }
    }

    pub fn default() -> Self {
        Self {
            sub: String::default(),
            name: None,
            user_id: Uuid::new_v4(),
            exp: 0,
        }
    }
}

impl<S> FromRequestParts<S> for Claims
where
    S: Send + Sync,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let TypedHeader(Authorization(bearer)) = parts
            .extract::<TypedHeader<Authorization<Bearer>>>()
            .await
            .map_err(|_| AuthError::InvalidToken)?;
        let claims =
            Self::from_token_string(bearer.token()).map_err(|_| AuthError::InvalidToken)?;
        ensure_request_session(&mut parts.extensions, claims.user_id).await?;
        Ok(claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::jwt::Keys;
    use serde_json::json;

    const NOW: u64 = 1_800_000_000;

    fn init_keys() {
        let _ = KEYS.set(Keys::new(b"session-token-test-secret"));
    }

    fn sign_raw(payload: serde_json::Value) -> String {
        init_keys();
        jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &payload,
            &KEYS.get().unwrap().encoding,
        )
        .unwrap()
    }

    fn marker(payload: serde_json::Value) -> SessionTokenMarker {
        serde_json::from_value(payload).unwrap()
    }

    #[test]
    fn access_token_round_trips_as_access_claims() {
        init_keys();
        let user_id = Uuid::new_v4();
        let token =
            Claims::new_access_token("ada@example.com", Some("Ada"), user_id).get_token_string();

        let claims = Claims::from_token_string(&token).expect("access token is accepted");

        assert_eq!(claims.user_id, user_id);
        assert_eq!(claims.sub, "ada@example.com");
        assert_eq!(claims.name.as_deref(), Some("Ada"));
    }

    #[test]
    fn refresh_token_round_trips_as_refresh_claims() {
        init_keys();
        let user_id = Uuid::new_v4();
        let token = RefreshClaims::new_refresh_token("ada@example.com", user_id).get_token_string();

        let claims = RefreshClaims::from_token_string(&token).expect("refresh token is accepted");

        assert_eq!(claims.user_id, user_id);
        assert!(claims.refresh);
    }

    #[test]
    fn refresh_token_is_rejected_as_access_token() {
        init_keys();
        let token =
            RefreshClaims::new_refresh_token("ada@example.com", Uuid::new_v4()).get_token_string();

        assert!(Claims::from_token_string(&token).is_err());
    }

    #[test]
    fn access_token_is_rejected_as_refresh_token() {
        init_keys();
        let token =
            Claims::new_access_token("ada@example.com", None, Uuid::new_v4()).get_token_string();

        assert!(RefreshClaims::from_token_string(&token).is_err());
    }

    #[test]
    fn new_tokens_carry_an_explicit_token_use_claim() {
        init_keys();
        let access = Claims::new_access_token("ada@example.com", None, Uuid::new_v4());
        let refresh = RefreshClaims::new_refresh_token("ada@example.com", Uuid::new_v4());

        let access_payload: serde_json::Value =
            jsonwebtoken::dangerous::insecure_decode(access.get_token_string())
                .unwrap()
                .claims;
        let refresh_payload: serde_json::Value =
            jsonwebtoken::dangerous::insecure_decode(refresh.get_token_string())
                .unwrap()
                .claims;

        assert_eq!(access_payload["token_use"], "access");
        assert_eq!(refresh_payload["token_use"], "refresh");
        assert_eq!(refresh_payload["refresh"], true);
    }

    #[test]
    fn refresh_token_issued_before_the_deploy_is_rejected_as_access_token() {
        let exp = unix_now() + REFRESH_TOKEN_TTL_SECS;
        let token = sign_raw(json!({
            "refresh": true,
            "sub": "ada@example.com",
            "user_id": Uuid::new_v4(),
            "exp": exp,
        }));

        assert!(Claims::from_token_string(&token).is_err());
    }

    #[test]
    fn refresh_token_issued_before_the_deploy_still_refreshes() {
        let user_id = Uuid::new_v4();
        let token = sign_raw(json!({
            "refresh": true,
            "sub": "ada@example.com",
            "user_id": user_id,
            "exp": unix_now() + REFRESH_TOKEN_TTL_SECS,
        }));

        let claims = RefreshClaims::from_token_string(&token).expect("legacy refresh accepted");

        assert_eq!(claims.user_id, user_id);
    }

    #[test]
    fn access_token_issued_before_the_deploy_keeps_working_until_it_expires() {
        let user_id = Uuid::new_v4();
        let token = sign_raw(json!({
            "sub": "ada@example.com",
            "name": null,
            "user_id": user_id,
            "exp": unix_now() + ACCESS_TOKEN_TTL_SECS,
        }));

        let claims = Claims::from_token_string(&token).expect("legacy access accepted");

        assert_eq!(claims.user_id, user_id);
        assert!(RefreshClaims::from_token_string(&token).is_err());
    }

    #[test]
    fn expired_access_token_is_rejected() {
        let token = sign_raw(json!({
            "sub": "ada@example.com",
            "name": null,
            "user_id": Uuid::new_v4(),
            "token_use": "access",
            "exp": unix_now() - 3600,
        }));

        assert!(Claims::from_token_string(&token).is_err());
    }

    #[test]
    fn untyped_token_outliving_the_access_ttl_is_not_an_access_token() {
        let long_lived = marker(json!({ "exp": NOW + ACCESS_TOKEN_TTL_SECS + 3600 }));
        let within_ttl = marker(json!({ "exp": NOW + ACCESS_TOKEN_TTL_SECS }));
        let within_skew = marker(json!({
            "exp": NOW + ACCESS_TOKEN_TTL_SECS + LEGACY_ACCESS_TOKEN_CLOCK_SKEW_SECS
        }));

        assert_eq!(long_lived.token_use(NOW), None);
        assert_eq!(within_ttl.token_use(NOW), Some(TokenUse::Access));
        assert_eq!(within_skew.token_use(NOW), Some(TokenUse::Access));
    }

    #[test]
    fn untyped_token_with_refresh_false_is_neither_access_nor_refresh() {
        let token = marker(json!({ "refresh": false, "exp": NOW + 60 }));

        assert_eq!(token.token_use(NOW), None);
    }

    #[test]
    fn conflicting_token_markers_are_rejected() {
        let access_with_refresh_flag =
            marker(json!({ "token_use": "access", "refresh": true, "exp": NOW + 60 }));
        let refresh_without_refresh_flag =
            marker(json!({ "token_use": "refresh", "exp": NOW + 60 }));

        assert_eq!(access_with_refresh_flag.token_use(NOW), None);
        assert_eq!(refresh_without_refresh_flag.token_use(NOW), None);
    }

    #[test]
    fn explicit_token_use_wins_regardless_of_lifetime() {
        let refresh = marker(json!({
            "token_use": "refresh",
            "refresh": true,
            "exp": NOW + REFRESH_TOKEN_TTL_SECS
        }));
        let access = marker(json!({ "token_use": "access", "exp": NOW + ACCESS_TOKEN_TTL_SECS }));

        assert_eq!(refresh.token_use(NOW), Some(TokenUse::Refresh));
        assert_eq!(access.token_use(NOW), Some(TokenUse::Access));
    }

    #[test]
    fn unknown_token_use_is_rejected() {
        let token = sign_raw(json!({
            "sub": "ada@example.com",
            "name": null,
            "user_id": Uuid::new_v4(),
            "token_use": "id",
            "exp": unix_now() + 60,
        }));

        assert!(Claims::from_token_string(&token).is_err());
        assert!(RefreshClaims::from_token_string(&token).is_err());
    }

    #[test]
    fn token_signed_with_another_key_is_rejected() {
        init_keys();
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &json!({
                "sub": "ada@example.com",
                "name": null,
                "user_id": Uuid::new_v4(),
                "token_use": "access",
                "exp": unix_now() + 60,
            }),
            &jsonwebtoken::EncodingKey::from_secret(b"some-other-secret"),
        )
        .unwrap();

        assert!(Claims::from_token_string(&token).is_err());
    }
}
