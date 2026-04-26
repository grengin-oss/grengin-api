use crate::{
    auth::{
        encryption::{decrypt_key, key_from_b64},
        jwt::{Keys, KEYS},
    },
    models::{ai_engines, embedding_configs, sso_providers},
};
use openidconnect::{core::CoreClient, EndpointMaybeSet, EndpointNotSet, EndpointSet};
use reqwest::Url;
use sea_orm::{DatabaseConnection, EntityTrait, QueryOrder};
use std::collections::HashMap;
use thiserror::Error;
use tokio::sync::RwLock;

pub type OidcClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

pub struct Settings {
    pub auth: AuthSettings,
    pub google: RwLock<Option<GoogleSettings>>,
    pub azure: RwLock<Option<AzureSettings>>,
    pub server: ServerSettings,
    pub openai: RwLock<Option<OpenaiSettings>>,
    pub anthropic: RwLock<Option<AnthropicSettings>>,
    pub mistral: RwLock<Option<MistralSettings>>,
    pub gemini: RwLock<Option<GeminiSettings>>,
    pub ai_engines_cache: RwLock<HashMap<String, AiEngineStateCache>>,
    pub embedding: RwLock<Option<EmbeddingSettings>>,
    pub rag: RagSettings,
}

pub struct ServerSettings {
    pub host: String,
    pub port: u16,
}

pub struct AuthSettings {
    pub jwt_secret: String,
    pub app_key: [u8; 32],
    pub redirect_url: String,
    pub database_url: String,
}

#[derive(Clone)]
pub struct GoogleSettings {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
    pub is_enabled: bool,
    pub allowed_domains: Vec<String>,
}

#[derive(Clone)]
pub struct AzureSettings {
    pub client_id: String,
    pub client_secret: String,
    pub tenant_id: String,
    pub redirect_url: String,
    pub is_enabled: bool,
    pub allowed_domains: Vec<String>,
}

#[derive(Clone)]
pub struct OpenaiSettings {
    pub api_key: String,
    pub org_id: Option<String>,
    pub project_id: Option<String>,
    pub timeout_ms: i32,
    pub max_retries: i32,
    pub is_enabled: bool,
}

#[derive(Clone)]
pub struct AnthropicSettings {
    pub api_key: String,
    pub is_enabled: bool,
}

#[derive(Clone)]
pub struct MistralSettings {
    pub api_key: String,
    pub is_enabled: bool,
}

#[derive(Clone)]
pub struct GeminiSettings {
    pub api_key: String,
    pub is_enabled: bool,
}

#[derive(Clone)]
pub struct EmbeddingSettings {
    pub provider: String,
    pub model: String,
    pub dimensions: Option<i32>,
    pub is_enabled: bool,
}

#[derive(Clone)]
pub struct RagSettings {
    pub enabled: bool,
    pub recent_message_pairs: usize,
    pub retrieval_top_k: usize,
    pub max_context_tokens: usize,
    pub summary_model_openai: Option<String>,
    pub summary_model_anthropic: Option<String>,
}

#[derive(Clone, Default)]
pub struct AiEngineStateCache {
    pub api_key: Option<String>,
    pub is_enabled: bool,
    pub whitelist_models: Vec<String>,
}

impl Settings {
    pub async fn load_ai_engines_from_db(
        &mut self,
        database: &DatabaseConnection,
    ) -> Result<(), ConfigError> {
        let ai_engines = ai_engines::Entity::find()
            .order_by_desc(ai_engines::Column::CreatedAt)
            .all(database)
            .await
            .map_err(|e| ConfigError::DbError(e.to_string()))?;
        for engine in ai_engines {
            let api_key = engine.api_key.as_ref().and_then(|encrypted_api_key| {
                decrypt_key(&self.auth.app_key, encrypted_api_key).ok()
            });
            self.load_ai_engine_in_state(
                engine.engine_key,
                api_key,
                engine.is_enabled,
                engine.whitelist_models,
            )
            .await?;
        }
        Ok(())
    }

    pub async fn load_embedding_config_from_db(
        &mut self,
        database: &DatabaseConnection,
    ) -> Result<(), ConfigError> {
        let config = embedding_configs::Entity::find()
            .order_by_desc(embedding_configs::Column::UpdatedAt)
            .one(database)
            .await
            .map_err(|e| ConfigError::DbError(e.to_string()))?;
        let embedding_config = config.map(|config| EmbeddingSettings {
            provider: config.provider,
            model: config.model,
            dimensions: config.dimensions,
            is_enabled: config.is_enabled,
        });
        *self.embedding.write().await = embedding_config;
        Ok(())
    }

    pub async fn get_ai_engine_api_key<S: Into<String>>(&self, provider: S) -> Option<String> {
        match provider.into().as_str() {
            "openai" => {
                let api_key = self
                    .openai
                    .read()
                    .await
                    .clone()
                    .map(|openai| openai.api_key);
                return api_key;
            }
            "anthropic" => {
                let api_key = self
                    .anthropic
                    .read()
                    .await
                    .clone()
                    .map(|anthropic| anthropic.api_key);
                return api_key;
            }
            "mistral" => {
                let api_key = self
                    .mistral
                    .read()
                    .await
                    .clone()
                    .map(|mistral| mistral.api_key);
                return api_key;
            }
            "gemini" => {
                let api_key = self
                    .gemini
                    .read()
                    .await
                    .clone()
                    .map(|gemini| gemini.api_key);
                return api_key;
            }
            _ => return None,
        }
    }

    pub async fn load_ai_engine_in_state<S: Into<String>>(
        &self,
        engine_key: S,
        api_key: Option<String>,
        is_enabled: bool,
        whitelist_models: Vec<String>,
    ) -> Result<(), ConfigError> {
        let engine_key = engine_key.into();
        let cache_key = engine_key.to_lowercase();
        self.set_ai_engine_cache(
            cache_key.clone(),
            api_key.clone(),
            is_enabled,
            whitelist_models,
        )
        .await;
        match cache_key.as_str() {
            "openai" => {
                if is_enabled {
                    println!("openai api key added successfully from ai_engines Table");
                    *self.openai.write().await = Some(OpenaiSettings {
                        api_key: api_key.clone().unwrap_or_default(),
                        org_id: None,
                        project_id: None,
                        timeout_ms: 10_000,
                        max_retries: 10,
                        is_enabled,
                    });
                } else {
                    *self.openai.write().await = None;
                }
            }
            "anthropic" => {
                if is_enabled {
                    println!("anthropic api key added successfully from ai_engines Table");
                    *self.anthropic.write().await = Some(AnthropicSettings {
                        api_key: api_key.unwrap_or_default(),
                        is_enabled,
                    });
                } else {
                    *self.anthropic.write().await = None;
                }
            }
            "mistral" => {
                if is_enabled {
                    println!("mistral api key added successfully from ai_engines Table");
                    *self.mistral.write().await = Some(MistralSettings {
                        api_key: api_key.unwrap_or_default(),
                        is_enabled,
                    });
                } else {
                    *self.mistral.write().await = None;
                }
            }
            "gemini" => {
                if is_enabled {
                    println!("gemini api key added successfully from ai_engines Table");
                    *self.gemini.write().await = Some(GeminiSettings {
                        api_key: api_key.unwrap_or_default(),
                        is_enabled,
                    });
                } else {
                    *self.gemini.write().await = None;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub async fn set_ai_engine_cache(
        &self,
        engine_key: String,
        api_key: Option<String>,
        is_enabled: bool,
        whitelist_models: Vec<String>,
    ) {
        let mut cache = self.ai_engines_cache.write().await;
        cache.insert(
            engine_key,
            AiEngineStateCache {
                api_key,
                is_enabled,
                whitelist_models,
            },
        );
    }

    pub async fn get_ai_engine_whitelist<S: AsRef<str>>(
        &self,
        engine_key: S,
    ) -> Option<Vec<String>> {
        let key = engine_key.as_ref().to_lowercase();
        let cache = self.ai_engines_cache.read().await;
        cache.get(&key).map(|entry| entry.whitelist_models.clone())
    }

    pub async fn set_embedding_config_in_state(&self, config: EmbeddingSettings) {
        *self.embedding.write().await = Some(config);
    }

    pub async fn get_embedding_config(&self) -> Option<EmbeddingSettings> {
        self.embedding.read().await.clone()
    }

    pub async fn load_sso_providers_from_db(
        &mut self,
        database: &DatabaseConnection,
    ) -> Result<(), ConfigError> {
        let sso_providers = sso_providers::Entity::find()
            .order_by_desc(sso_providers::Column::CreatedAt)
            .all(database)
            .await
            .map_err(|e| ConfigError::DbError(e.to_string()))?;
        for sso_provider in sso_providers {
            let Ok(client_secret) = decrypt_key(&self.auth.app_key, &sso_provider.client_secret)
            else {
                continue;
            }; // fall back for default <empty> string
            let Ok(_) = Url::parse(&sso_provider.redirect_url) else {
                continue;
            };
            let Ok(_) = Url::parse(&sso_provider.issuer_url) else {
                continue;
            };
            if !sso_provider.is_enabled {
                continue;
            }
            self.load_sso_provider_in_state(
                sso_provider.provider,
                client_secret,
                sso_provider.client_id,
                sso_provider.redirect_url,
                sso_provider.tenant_id,
                true,
                sso_provider.allowed_domains,
            )
            .await?;
        }
        Ok(())
    }

    pub async fn load_sso_provider_in_state<S: Into<String>>(
        &self,
        provider: S,
        client_secret: S,
        client_id: S,
        redirect_url: S,
        tenant_id: Option<S>,
        is_enabled: bool,
        allowed_domains: Vec<S>,
    ) -> Result<(), ConfigError> {
        match provider.into().as_str() {
            "azure" => {
                println!("azure sso provider added from sso_provider table");
                *self.azure.write().await = Some(AzureSettings {
                    client_id: client_id.into(),
                    client_secret: client_secret.into(),
                    tenant_id: tenant_id.map(|t| t.into()).unwrap_or("common".into()),
                    redirect_url: redirect_url.into(),
                    is_enabled,
                    allowed_domains: allowed_domains.into_iter().map(|d| d.into()).collect(),
                });
            }
            "google" => {
                println!("google sso provider added from sso_provider table");
                *self.google.write().await = Some(GoogleSettings {
                    client_id: client_id.into(),
                    client_secret: client_secret.into(),
                    redirect_url: redirect_url.into(),
                    is_enabled,
                    allowed_domains: allowed_domains.into_iter().map(|d| d.into()).collect(),
                });
            }
            _ => {}
        }
        Ok(())
    }

    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            auth: AuthSettings::from_env()?,
            google: RwLock::new(GoogleSettings::from_env().ok()),
            azure: RwLock::new(AzureSettings::from_env().ok()),
            server: ServerSettings::from_env()?,
            openai: RwLock::new(OpenaiSettings::from_env().ok()),
            anthropic: RwLock::new(AnthropicSettings::from_env().ok()),
            mistral: RwLock::new(MistralSettings::from_env().ok()),
            gemini: RwLock::new(GeminiSettings::from_env().ok()),
            ai_engines_cache: RwLock::new(HashMap::new()),
            embedding: RwLock::new(None),
            rag: RagSettings::from_env(),
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
        let jwt_secret =
            std::env::var("JWT_SECRET").map_err(|_| ConfigError::Missing("JWT_SECRET"))?;
        let app_key = key_from_b64(
            std::env::var("APP_KEY")
                .map_err(|_| ConfigError::Missing("APP_KEY"))?
                .as_str(),
        )
        .map_err(|e| ConfigError::Custom(e.to_string()))?;
        KEYS.set(Keys::new(jwt_secret.as_bytes()))
            .map_err(|_| ConfigError::AlreadyInitilized("KEYS"))?;
        let redirect_url =
            std::env::var("REDIRECT_URL").map_err(|_| ConfigError::Missing("REDIRECT_URL"))?;
        let database_url =
            std::env::var("DATABASE_URL").map_err(|_| ConfigError::Missing("DATABASE_URL"))?;
        Ok(Self {
            jwt_secret,
            redirect_url,
            database_url,
            app_key,
        })
    }
}

impl GoogleSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        let client_id = std::env::var("GOOGLE_CLIENT_ID")
            .or_else(|_| std::env::var("GOOGLE_CLIENT"))
            .map_err(|_| ConfigError::Missing("GOOGLE_CLIENT_ID"))?;
        let client_secret = std::env::var("GOOGLE_CLIENT_SECRET")
            .map_err(|_| ConfigError::Missing("GOOGLE_CLIENT_SECRET"))?;
        let app_redirect_url =
            std::env::var("REDIRECT_URL").map_err(|_| ConfigError::Missing("REDIRECT_URL"))?;
        let redirect_url = format!("{}/auth/google/callback", app_redirect_url);
        Ok(Self {
            client_id,
            client_secret,
            redirect_url,
            is_enabled: true,
            allowed_domains: Vec::new(),
        })
    }
}

impl AzureSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        let client_id = std::env::var("AZURE_CLIENT_ID")
            .map_err(|_| ConfigError::Missing("AZURE_CLIENT_ID"))?;
        let client_secret = std::env::var("AZURE_CLIENT_SECRET")
            .map_err(|_| ConfigError::Missing("AZURE_CLIENT_SECRET"))?;
        let tenant_id = std::env::var("AZURE_TENANT_ID")
            .map_err(|_| ConfigError::Missing("AZURE_TENANT_ID"))?;
        let app_redirect_url =
            std::env::var("REDIRECT_URL").map_err(|_| ConfigError::Missing("REDIRECT_URL"))?;
        let redirect_url = format!("{}/auth/azure/callback", app_redirect_url);
        Ok(Self {
            client_id,
            client_secret,
            redirect_url,
            tenant_id,
            is_enabled: true,
            allowed_domains: Vec::new(),
        })
    }
}

impl OpenaiSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        let api_key =
            std::env::var("OPENAI_API_KEY").map_err(|_| ConfigError::Missing("OPENAI_API_KEY"))?;
        let org_id = std::env::var("OPENAI_ORG_ID").ok();
        let project_id = std::env::var("OPENAI_PROJECT_ID").ok();
        let timeout_ms = std::env::var("OPENAI_TIMEOUT_MS")
            .unwrap_or("60000".to_string())
            .parse::<i32>()
            .map_err(|_| ConfigError::ParseError("OPENAI_TIMEOUT_MS"))?;
        let max_retries = std::env::var("OPENAI_MAX_TRIES")
            .unwrap_or("1".to_string())
            .parse::<i32>()
            .map_err(|_| ConfigError::ParseError("OPENAI_MAX_RETRIES"))?;
        Ok(Self {
            api_key,
            org_id,
            project_id,
            timeout_ms,
            max_retries,
            is_enabled: true,
        })
    }
}

impl AnthropicSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| ConfigError::Missing("ANTHROPIC_API_KEY"))?;
        Ok(Self {
            api_key,
            is_enabled: true,
        })
    }
}

impl MistralSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        let api_key = std::env::var("MISTRAL_API_KEY")
            .map_err(|_| ConfigError::Missing("MISTRAL_API_KEY"))?;
        Ok(Self {
            api_key,
            is_enabled: true,
        })
    }
}

impl GeminiSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        let api_key =
            std::env::var("GEMINI_API_KEY").map_err(|_| ConfigError::Missing("GEMINI_API_KEY"))?;
        Ok(Self {
            api_key,
            is_enabled: true,
        })
    }
}

impl RagSettings {
    pub fn from_env() -> Self {
        let enabled = std::env::var("RAG_ENABLED")
            .ok()
            .and_then(|val| val.parse::<bool>().ok())
            .unwrap_or(false);
        let recent_message_pairs = std::env::var("RAG_RECENT_MESSAGE_PAIRS")
            .ok()
            .and_then(|val| val.parse::<usize>().ok())
            .unwrap_or(3);
        let retrieval_top_k = std::env::var("RAG_RETRIEVAL_TOP_K")
            .ok()
            .and_then(|val| val.parse::<usize>().ok())
            .unwrap_or(4);
        let max_context_tokens = std::env::var("RAG_MAX_CONTEXT_TOKENS")
            .ok()
            .and_then(|val| val.parse::<usize>().ok())
            .unwrap_or(8000);
        let summary_model_openai = std::env::var("RAG_SUMMARY_MODEL_OPENAI").ok();
        let summary_model_anthropic = std::env::var("RAG_SUMMARY_MODEL_ANTHROPIC").ok();
        Self {
            enabled,
            recent_message_pairs,
            retrieval_top_k,
            max_context_tokens,
            summary_model_openai,
            summary_model_anthropic,
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("missing configuration variable: {0}")]
    Missing(&'static str),
    #[error("already initilized env variable: {0}")]
    AlreadyInitilized(&'static str),
    #[error("parsing error env variable: {0}")]
    ParseError(&'static str),
    #[error("db fetch error {0}")]
    DbError(String),
    #[error("DB error {0}")]
    NotConfigured(&'static str),
    #[error("{0}")]
    InvalidSSoProvider(String),
    #[error("{0}")]
    SsoClientBuildError(String),
    #[error("{0}")]
    ReqwestClientBuildError(String),
    #[error("{0}")]
    Custom(String),
}
