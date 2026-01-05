use anyhow::Error;
use axum::{extract::FromRequestParts, http::request::Parts, RequestPartsExt};
use axum_extra::{headers::{authorization::{Bearer}, Authorization},TypedHeader};
use jsonwebtoken::{Validation, decode};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use crate::{auth::jwt::KEYS, models::users::UserRole};
use crate::auth::error::AuthError;

pub trait Claiming:Serialize + for<'a> Deserialize<'a> {
    fn get_token_string(&self) -> String {
       jsonwebtoken::encode(&jsonwebtoken::Header::default(),&self,&KEYS.get().unwrap().encoding)
         .unwrap()
    }

    fn from_token_string(token:&str) -> Result<Self,Error>{
        let data = decode::<Self>(token, &KEYS.get().expect("JWT KEYS is not set").decoding, &Validation::default())?;
        Ok(data.claims)
    }
}

#[derive(Debug, Serialize, Deserialize,ToSchema,IntoParams)]
pub struct RefreshClaims {
    pub refresh:bool,
    pub sub: String, // Email Subject (user identifier)
    pub user_id:Uuid,//user id
    pub exp: usize,  // Expiration time
}

impl Claiming for RefreshClaims {}

impl RefreshClaims {
    pub fn new_refresh_token<S: Into<String>>(sub:S,user_id:Uuid) -> Self {
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs()+ 3600 * 24 * 7;
        Self { 
          sub:sub.into(),
          refresh:true,
          user_id,
          exp:exp as usize,
        }
    }
}

#[derive(Debug, Serialize, Deserialize,ToSchema,IntoParams)]
pub struct Claims {
    pub sub: String, // Email Subject (user identifier)
    pub name:Option<String>,
    pub user_id:Uuid,//user id
    pub org_id:Option<Uuid>,
    pub role:UserRole,
    pub exp: usize,  // Expiration time
}

impl Claiming for Claims  {}

impl Claims {
    pub fn new_access_token<S: Into<String>>(sub:S,name:Option<S>,user_id:Uuid,org_id:Option<Uuid>,role:UserRole) -> Self {
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs()+ 3600;
        Self { 
          sub:sub.into(),
          name:name.map(|v| v.into()),
          user_id,
          org_id,
          role,
          exp:exp as usize,
        }
    }

    pub fn default() -> Self {
       Self { 
           sub:String::default(), 
           name:None,
           user_id:Uuid::new_v4(),
           org_id:None,
           role:UserRole::SuperAdmin, 
           exp:0,
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
        let claims = Self::from_token_string(bearer.token())
            .map_err(|_| AuthError::InvalidToken)?;
        Ok(claims)
    }
}
