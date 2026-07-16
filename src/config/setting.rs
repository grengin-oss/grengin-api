use crate::{
    auth::{
        encryption::{decrypt_key, key_from_b64},
        jwt::{KEYS, Keys},
    },
    models::{ai_engines, embedding_configs, sso_providers},
};
use openidconnect::{EndpointMaybeSet, EndpointNotSet, EndpointSet, core::CoreClient};
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
    pub use_grengin_proxy: bool,
    pub jit_provisioning: bool,
}

#[derive(Clone)]
pub struct AzureSettings {
    pub client_id: String,
    pub client_secret: String,
    pub tenant_id: String,
    pub redirect_url: String,
    pub is_enabled: bool,
    pub allowed_domains: Vec<String>,
    pub use_grengin_proxy: bool,
    pub jit_provisioning: bool,
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
    pub summary_llm_provider: Option<String>,
    pub summary_llm_model: Option<String>,
}

#[derive(Clone, Default)]
pub struct AiEngineStateCache {
    pub api_key: Option<String>,
    pub is_enabled: bool,
    pub whitelist_models: Vec<String>,
}

fn read_non_empty_env(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn env_flag_any(names: &[&str]) -> bool {
    names.iter().any(|name| {
        std::env::var(name)
            .ok()
            .map(|raw| {
                matches!(
                    raw.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "y" | "on"
                )
            })
            .unwrap_or(false)
    })
}

fn csv_env(name: &str) -> Vec<String> {
    std::env::var(name)
        .ok()
        .map(|raw| {
            raw.split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default()
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
        if let Some(config) = config {
            let embedding_config = EmbeddingSettings {
                provider: config.provider,
                model: config.model,
                dimensions: config.dimensions,
                is_enabled: config.is_enabled,
            };
            *self.embedding.write().await = Some(embedding_config);
        }
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
        // Trim whitespace from API keys — copy-paste often adds trailing newlines which
        // make HeaderValue::try_from fail and cause CannotCloneRequestError downstream.
        let api_key = api_key.map(|k| k.trim().to_string());
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
                sso_provider.use_grengin_proxy,
                sso_provider.jit_provisioning,
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
        use_grengin_proxy: bool,
        jit_provisioning: bool,
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
                    use_grengin_proxy,
                    jit_provisioning,
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
                    use_grengin_proxy,
                    jit_provisioning,
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
            embedding: RwLock::new(EmbeddingSettings::from_env()),
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
        let google_client_id_local = read_non_empty_env(&["GOOGLE_CLIENT_ID", "GOOGLE_CLIENT"]);
        let google_client_secret_local = read_non_empty_env(&["GOOGLE_CLIENT_SECRET"]);
        let has_local_google_credentials =
            google_client_id_local.is_some() && google_client_secret_local.is_some();
        let use_proxy =
            env_flag_any(&["SSO_PROXY_AUTO_ENABLE", "SSO_PROXY_ENABLED"])
                || !has_local_google_credentials;

        let (client_id, client_secret, redirect_url, allowed_domains, use_grengin_proxy) =
            if use_proxy {
                let app_redirect_url = std::env::var("REDIRECT_URL")
                    .map_err(|_| ConfigError::Missing("REDIRECT_URL"))?;
                let client_id = read_non_empty_env(&[
                    "GRENGIN_PROXY_GOOGLE_CLIENT_ID",
                    "GOOGLE_CLIENT_ID",
                    "GOOGLE_CLIENT",
                ])
                .unwrap_or_else(|| "managed-by-grengin-proxy".to_string());
                let client_secret = read_non_empty_env(&[
                    "GRENGIN_PROXY_GOOGLE_CLIENT_SECRET",
                    "GOOGLE_CLIENT_SECRET",
                ])
                .unwrap_or_else(|| "managed-by-grengin-proxy".to_string());
                (
                    client_id,
                    client_secret,
                    format!("{}/auth/google/callback", app_redirect_url),
                    csv_env("GRENGIN_PROXY_ALLOWED_DOMAINS"),
                    true,
                )
            } else {
                let client_id = google_client_id_local.ok_or(ConfigError::Missing("GOOGLE_CLIENT_ID"))?;
                let client_secret =
                    google_client_secret_local.ok_or(ConfigError::Missing("GOOGLE_CLIENT_SECRET"))?;
                let app_redirect_url = std::env::var("REDIRECT_URL")
                    .map_err(|_| ConfigError::Missing("REDIRECT_URL"))?;
                (
                    client_id,
                    client_secret,
                    format!("{}/auth/google/callback", app_redirect_url),
                    Vec::new(),
                    false,
                )
            };

        Ok(Self {
            client_id,
            client_secret,
            redirect_url,
            is_enabled: true,
            allowed_domains,
            use_grengin_proxy,
            jit_provisioning: true,
        })
    }
}

impl AzureSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        let azure_client_id_local = read_non_empty_env(&["AZURE_CLIENT_ID"]);
        let azure_client_secret_local = read_non_empty_env(&["AZURE_CLIENT_SECRET"]);
        let has_local_azure_credentials =
            azure_client_id_local.is_some() && azure_client_secret_local.is_some();
        let use_proxy =
            env_flag_any(&["SSO_PROXY_AUTO_ENABLE", "SSO_PROXY_ENABLED"])
                || !has_local_azure_credentials;

        let (client_id, client_secret, tenant_id, redirect_url, allowed_domains, use_grengin_proxy) =
            if use_proxy {
                let app_redirect_url = std::env::var("REDIRECT_URL")
                    .map_err(|_| ConfigError::Missing("REDIRECT_URL"))?;
                let client_id =
                    read_non_empty_env(&["GRENGIN_PROXY_AZURE_CLIENT_ID"])
                        .unwrap_or_else(|| "managed-by-grengin-proxy".to_string());
                let client_secret = read_non_empty_env(&[
                    "GRENGIN_PROXY_AZURE_CLIENT_SECRET",
                ])
                .unwrap_or_else(|| "managed-by-grengin-proxy".to_string());
                let tenant_id =
                    read_non_empty_env(&["GRENGIN_PROXY_AZURE_TENANT_ID", "AZURE_TENANT_ID"])
                        .unwrap_or_else(|| "common".to_string());
                (
                    client_id,
                    client_secret,
                    tenant_id,
                    format!("{}/auth/azure/callback", app_redirect_url),
                    csv_env("GRENGIN_PROXY_ALLOWED_DOMAINS"),
                    true,
                )
            } else {
                let client_id = azure_client_id_local.ok_or(ConfigError::Missing("AZURE_CLIENT_ID"))?;
                let client_secret =
                    azure_client_secret_local.ok_or(ConfigError::Missing("AZURE_CLIENT_SECRET"))?;
                let tenant_id = std::env::var("AZURE_TENANT_ID")
                    .map_err(|_| ConfigError::Missing("AZURE_TENANT_ID"))?;
                let app_redirect_url = std::env::var("REDIRECT_URL")
                    .map_err(|_| ConfigError::Missing("REDIRECT_URL"))?;
                (
                    client_id,
                    client_secret,
                    tenant_id,
                    format!("{}/auth/azure/callback", app_redirect_url),
                    Vec::new(),
                    false,
                )
            };

        Ok(Self {
            client_id,
            client_secret,
            redirect_url,
            tenant_id,
            is_enabled: true,
            allowed_domains,
            use_grengin_proxy,
            jit_provisioning: true,
        })
    }
}

impl OpenaiSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| ConfigError::Missing("OPENAI_API_KEY"))?
            .split_whitespace()
            .collect::<String>();
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
            .map_err(|_| ConfigError::Missing("ANTHROPIC_API_KEY"))?
            .split_whitespace()
            .collect::<String>();
        Ok(Self {
            api_key,
            is_enabled: true,
        })
    }
}

impl MistralSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        let api_key = std::env::var("MISTRAL_API_KEY")
            .map_err(|_| ConfigError::Missing("MISTRAL_API_KEY"))?
            .split_whitespace()
            .collect::<String>();
        Ok(Self {
            api_key,
            is_enabled: true,
        })
    }
}

impl GeminiSettings {
    pub fn from_env() -> Result<Self, ConfigError> {
        let api_key = std::env::var("GEMINI_API_KEY")
            .map_err(|_| ConfigError::Missing("GEMINI_API_KEY"))?
            .split_whitespace()
            .collect::<String>();
        Ok(Self {
            api_key,
            is_enabled: true,
        })
    }
}

impl EmbeddingSettings {
    pub fn from_env() -> Option<Self> {
        let is_enabled = std::env::var("RAG_EMBEDDING_ENABLED")
            .ok()
            .and_then(|val| val.parse::<bool>().ok())
            .unwrap_or(false);
        let provider = std::env::var("RAG_EMBEDDING_LLM_PROVIDER").ok()?;
        let model = std::env::var("RAG_EMBEDDING_LLM_MODEL").ok()?;
        Some(Self {
            provider,
            model,
            dimensions: None,
            is_enabled,
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
        let summary_llm_provider = std::env::var("RAG_SUMMARY_LLM_PROVIDER").ok();
        let summary_llm_model = std::env::var("RAG_SUMMARY_LLM_MODEL").ok().or_else(|| {
            // Backward compatibility for existing deployments using provider-specific model env vars.
            std::env::var("RAG_SUMMARY_MODEL_OPENAI")
                .ok()
                .or_else(|| std::env::var("RAG_SUMMARY_MODEL_ANTHROPIC").ok())
        });
        Self {
            enabled,
            recent_message_pairs,
            retrieval_top_k,
            max_context_tokens,
            summary_llm_provider,
            summary_llm_model,
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

#[cfg(test)]
mod tests {
    use super::{EmbeddingSettings, RagSettings};
    use std::sync::{LazyLock, Mutex};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn clear_embedding_env() {
        // SAFETY: tests serialize env mutations with ENV_LOCK.
        unsafe {
            std::env::remove_var("RAG_EMBEDDING_ENABLED");
            std::env::remove_var("RAG_EMBEDDING_LLM_PROVIDER");
            std::env::remove_var("RAG_EMBEDDING_LLM_MODEL");
        }
    }

    fn clear_rag_summary_env() {
        // SAFETY: tests serialize env mutations with ENV_LOCK.
        unsafe {
            std::env::remove_var("RAG_SUMMARY_LLM_PROVIDER");
            std::env::remove_var("RAG_SUMMARY_LLM_MODEL");
            std::env::remove_var("RAG_SUMMARY_MODEL_OPENAI");
            std::env::remove_var("RAG_SUMMARY_MODEL_ANTHROPIC");
        }
    }

    #[test]
    fn embedding_from_env_none_when_provider_or_model_missing() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        clear_embedding_env();
        // SAFETY: tests serialize env mutations with ENV_LOCK.
        unsafe {
            std::env::set_var("RAG_EMBEDDING_ENABLED", "true");
        }
        assert!(EmbeddingSettings::from_env().is_none());

        // SAFETY: tests serialize env mutations with ENV_LOCK.
        unsafe {
            std::env::set_var("RAG_EMBEDDING_LLM_PROVIDER", "openai");
        }
        assert!(EmbeddingSettings::from_env().is_none());

        clear_embedding_env();
    }

    #[test]
    fn embedding_from_env_reads_provider_model_and_enabled() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        clear_embedding_env();
        // SAFETY: tests serialize env mutations with ENV_LOCK.
        unsafe {
            std::env::set_var("RAG_EMBEDDING_ENABLED", "true");
            std::env::set_var("RAG_EMBEDDING_LLM_PROVIDER", "openai");
            std::env::set_var("RAG_EMBEDDING_LLM_MODEL", "text-embedding-3-small");
        }

        let config = EmbeddingSettings::from_env().expect("embedding config from env");
        assert_eq!(config.provider, "openai");
        assert_eq!(config.model, "text-embedding-3-small");
        assert!(config.is_enabled);
        assert_eq!(config.dimensions, None);

        clear_embedding_env();
    }

    #[test]
    fn embedding_enabled_defaults_false_when_missing_or_invalid() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        clear_embedding_env();
        // SAFETY: tests serialize env mutations with ENV_LOCK.
        unsafe {
            std::env::set_var("RAG_EMBEDDING_LLM_PROVIDER", "openai");
            std::env::set_var("RAG_EMBEDDING_LLM_MODEL", "text-embedding-3-small");
        }
        let config = EmbeddingSettings::from_env().expect("embedding config from env");
        assert!(!config.is_enabled);

        // SAFETY: tests serialize env mutations with ENV_LOCK.
        unsafe {
            std::env::set_var("RAG_EMBEDDING_ENABLED", "not-a-bool");
        }
        let config = EmbeddingSettings::from_env().expect("embedding config from env");
        assert!(!config.is_enabled);

        clear_embedding_env();
    }

    #[test]
    fn rag_summary_model_prefers_new_env_and_falls_back_to_legacy() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        clear_rag_summary_env();

        // SAFETY: tests serialize env mutations with ENV_LOCK.
        unsafe {
            std::env::set_var("RAG_SUMMARY_MODEL_OPENAI", "legacy-openai-model");
        }
        let rag = RagSettings::from_env();
        assert_eq!(
            rag.summary_llm_model.as_deref(),
            Some("legacy-openai-model")
        );

        // SAFETY: tests serialize env mutations with ENV_LOCK.
        unsafe {
            std::env::set_var("RAG_SUMMARY_LLM_MODEL", "new-model");
        }
        let rag = RagSettings::from_env();
        assert_eq!(rag.summary_llm_model.as_deref(), Some("new-model"));

        clear_rag_summary_env();
    }

    #[test]
    fn rag_summary_provider_reads_new_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        clear_rag_summary_env();
        // SAFETY: tests serialize env mutations with ENV_LOCK.
        unsafe {
            std::env::set_var("RAG_SUMMARY_LLM_PROVIDER", "anthropic");
        }

        let rag = RagSettings::from_env();
        assert_eq!(rag.summary_llm_provider.as_deref(), Some("anthropic"));

        clear_rag_summary_env();
    }
}
