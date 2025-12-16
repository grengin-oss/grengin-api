use openidconnect::{core::{CoreClient},EndpointMaybeSet, EndpointNotSet, EndpointSet};
use thiserror::Error;
use crate::auth::jwt::{KEYS, Keys};

pub type OidcClient = CoreClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointMaybeSet, EndpointMaybeSet>;

pub struct Settings {
    pub auth: AuthSettings,
    pub google:GoogleSettings,
    pub azure:AzureSettings,
    pub server:ServerSettings,
    pub openai:Option<OpenaiSettings>,
    pub anthropic:Option<AnthropicSettings>,
    pub groq:Option<GroqSettings>,
    pub gemini:Option<GeminiSettings>,
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

pub struct OpenaiSettings {
    pub api_key:String,
    pub org_id:Option<String>,
    pub project_id:Option<String>,
    pub timeout_ms:i32,
    pub max_retries:i32,
}

pub struct AnthropicSettings {
    pub api_key: String,
}

pub struct GroqSettings {
    pub api_key: String,
}

pub struct GeminiSettings {
    pub api_key: String,
}

impl Settings {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            auth:AuthSettings::from_env()?,
            google:GoogleSettings::from_env()?,
            azure:AzureSettings::from_env()?,
            server:ServerSettings::from_env()?,
            openai:OpenaiSettings::from_env().ok(),
            anthropic:AnthropicSettings::from_env().ok(),
            groq:GroqSettings::from_env().ok(),
            gemini:GeminiSettings::from_env().ok(),
        })
    }
}

impl ServerSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string()); // default
        let port = std::env::var("PORT")
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
        let redirect_url = format!("{}/auth/google/callback",app_redirect_url);
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

impl OpenaiSettings {
    pub fn from_env() -> Result<Self,ConfigError> {
        let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| ConfigError::Missing("OPENAI_API_KEY"))?;
        let org_id = std::env::var("OPENAI_ORG_ID").ok();
        let project_id = std::env::var("OPENAI_PROJECT_ID").ok();
        let timeout_ms = std::env::var("OPENAI_TIMEOUT_MS").unwrap_or("60000".to_string()).parse::<i32>().map_err(|_| ConfigError::ParseError("OPENAI_TIMEOUT_MS"))?;
        let max_retries = std::env::var("OPENAI_MAX_TRIES").unwrap_or("1".to_string()).parse::<i32>().map_err(|_| ConfigError::ParseError("OPENAI_MAX_RETRIES"))?;
      Ok(Self { api_key, org_id, project_id, timeout_ms, max_retries })
    }
}

impl AnthropicSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| ConfigError::Missing("ANTHROPIC_API_KEY"))?;
        Ok(Self { api_key })
    }
}

impl GroqSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        let api_key = std::env::var("GROQ_API_KEY").map_err(|_| ConfigError::Missing("GROQ_API_KEY"))?;
        Ok(Self { api_key })
    }
}

impl GeminiSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        let api_key = std::env::var("GEMINI_API_KEY").map_err(|_| ConfigError::Missing("GEMINI_API_KEY"))?;
        Ok(Self { api_key })
    }
}

#[derive(Debug,Error)]
pub enum ConfigError {
    #[error("missing configuration variable: {0}")]
    Missing(&'static str),
    #[error("already initilized env variable: {0}")]
    AlreadyInitilized(&'static str),
    #[error("parsing error env variable: {0}")]
    ParseError(&'static str)
}

