// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::auth::error::AuthError;
use crate::auth::jwt::KEYS;
use anyhow::Error;
use axum::{RequestPartsExt, extract::FromRequestParts, http::request::Parts};
use axum_extra::{
    TypedHeader,
    headers::{Authorization, authorization::Bearer},
};
use jsonwebtoken::{Validation, decode};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

pub trait Claiming: Serialize + for<'a> Deserialize<'a> {
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

// deny_unknown_fields: an access token carries `name`/`refresh` that a refresh
// token doesn't, so without this an access token decodes fine as RefreshClaims
// (extra fields silently ignored) and can mint new access tokens via /auth/refresh
// forever, defeating the shorter access-token lifetime.
#[derive(Debug, Serialize, Deserialize, ToSchema, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct RefreshClaims {
    pub sub: String,   // Email Subject (user identifier)
    pub user_id: Uuid, //user id
    pub exp: usize,    // Expiration time
}

impl Claiming for RefreshClaims {}

impl RefreshClaims {
    pub fn new_refresh_token<S: Into<String>>(sub: S, user_id: Uuid) -> Self {
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs()
            + 3600 * 24 * 7;
        Self {
            sub: sub.into(),
            user_id,
            exp: exp as usize,
        }
    }
}

// deny_unknown_fields: a refresh token has no `refresh` field, so it fails the
// required field instead of silently decoding as an access token. Kept for
// symmetry with RefreshClaims so neither shape can be coerced into the other.
#[derive(Debug, Serialize, Deserialize, ToSchema, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct Claims {
    pub sub: String, // Email Subject (user identifier)
    pub name: Option<String>,
    pub user_id: Uuid, //user id
    pub exp: usize,    // Expiration time
    pub refresh: bool,
}

impl Claiming for Claims {}

impl Claims {
    pub fn new_access_token<S: Into<String>>(sub: S, name: Option<S>, user_id: Uuid) -> Self {
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs()
            + 3600;
        Self {
            sub: sub.into(),
            name: name.map(|v| v.into()),
            user_id,
            exp: exp as usize,
            refresh:false,
        }
    }

    pub fn default() -> Self {
        Self {
            sub: String::default(),
            name: None,
            user_id: Uuid::new_v4(),
            exp: 0,
            refresh: false,
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
        Ok(claims)
    }
}
