use axum::{extract::FromRequestParts, http::request::Parts, RequestPartsExt};
use axum_extra::{headers::{authorization::{Bearer}, Authorization},TypedHeader};
use jsonwebtoken::{Validation, decode};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use crate::auth::jwt::KEYS;
use crate::auth::error::AuthError;

#[derive(Debug, Serialize, Deserialize,ToSchema,IntoParams)]
pub struct Claims {
    pub sub: String, // Email Subject (user identifier)
    pub name:Option<String>,
    pub user_id:Uuid,//user id
    pub exp: usize,  // Expiration time
}

impl Claims {
    pub fn new<S: Into<String>>(sub:S,name:Option<S>,user_id:Uuid) -> Self {
         let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs()+ 3600;
      Self { sub:sub.into(),
             name:name.map(|v| v.into()),
             user_id,
             exp:exp as usize 
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
        let token_data = decode::<Claims>(bearer.token(), &KEYS.get().expect("JWT KEYS is not set").decoding, &Validation::default())
            .map_err(|_| AuthError::InvalidToken)?;
        Ok(token_data.claims)
    }
}
