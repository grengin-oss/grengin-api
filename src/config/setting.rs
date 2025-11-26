use openidconnect::{core::{CoreClient},EndpointMaybeSet, EndpointNotSet, EndpointSet};
use thiserror::Error;
use crate::auth::jwt::{KEYS, Keys};

pub type OidcClient = CoreClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointMaybeSet, EndpointMaybeSet>;

pub struct Settings {
    pub auth: AuthSettings,
    pub google:GoogleSettings,
    pub azure:AzureSettings,
    pub server:ServerSettings,
}

#[derive(Debug, Clone)]
pub struct ServerSettings {
    pub host: String,
    pub port: u16,
}

pub struct AuthSettings {
    pub jwt_secret: String,
    pub redirect_url:String,
    pub database_url:String,
}

pub struct GoogleSettings {
    pub client_id:String,
    pub client_secret:String,
    pub redirect_url:String
}

pub struct AzureSettings {
    pub client_id:String,
    pub client_secret:String,
    pub tenant_id:String,
    pub redirect_url:String
}

impl Settings {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            auth:AuthSettings::from_env()?,
            google:GoogleSettings::from_env()?,
            server:ServerSettings::from_env()?,
            azure:AzureSettings::from_env()?,
        })
    }
}

impl ServerSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        let host = std::env::var("APP_HOST").unwrap_or_else(|_| "0.0.0.0".to_string()); // default
        let port = std::env::var("APP_PORT")
            .ok()
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(8080); // default
        Ok(Self { host, port })
    }
}

impl AuthSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        let jwt_secret = std::env::var("JWT_SECRET").map_err(|_| ConfigError::Missing("JWT_SECRET"))?;
        KEYS.set(Keys::new(jwt_secret.as_bytes())).map_err(|_| ConfigError::AlreadyInitilized("KEYS"))?;
        let redirect_url = std::env::var("REDIRECT_URL").map_err(|_| ConfigError::Missing("REDIRECT_URL"))?;
        let database_url = std::env::var("DATABASE_URL").map_err(|_| ConfigError::Missing("DATABASE_URL"))?;
        Ok(Self { jwt_secret,redirect_url,database_url })
    }
}

impl GoogleSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        let client_id = std::env::var("GOOGLE_CLIENT_ID").map_err(|_| ConfigError::Missing("GOOGLE_CLIENT_ID"))?;
        let client_secret = std::env::var("GOOGLE_CLIENT_SECRET").map_err(|_| ConfigError::Missing("GOOGLE_CLIENT_SECRET"))?;
        let app_redirect_url = std::env::var("REDIRECT_URL").map_err(|_| ConfigError::Missing("REDIRECT_URL"))?;
        let redirect_url = format!("{}/auth/google/callback",app_redirect_url.replace("https","http"));
      Ok(Self {client_id,client_secret,redirect_url })
    }
}

impl AzureSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        let client_id = std::env::var("AZURE_CLIENT_ID").map_err(|_| ConfigError::Missing("AZURE_CLIENT_ID"))?;
        let client_secret = std::env::var("AZURE_CLIENT_SECRET").map_err(|_| ConfigError::Missing("AZURE_CLIENT_SECRET"))?;
        let tenant_id = std::env::var("AZURE_TENANT_ID").map_err(|_| ConfigError::Missing("AZURE_TENANT_ID"))?;
        let app_redirect_url = std::env::var("REDIRECT_URL").map_err(|_| ConfigError::Missing("REDIRECT_URL"))?;
        let redirect_url = format!("{}/auth/azure/callback",app_redirect_url);
      Ok(Self {client_id,client_secret,redirect_url,tenant_id })
    }
}

#[derive(Debug,Error)]
pub enum ConfigError {
    #[error("missing configuration variable: {0}")]
    Missing(&'static str),
    #[error("already initilized global variable: {0}")]
    AlreadyInitilized(&'static str),
}

