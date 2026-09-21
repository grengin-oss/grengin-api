// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::{
        azure::{AzureMultitenantValidation, build_azure_client},
        encryption::decrypt_key,
        github::GitHubOAuthAdapter,
        google::build_google_client,
        provider_config::{
            OidcProviderConfiguration, build_discovered_oidc_client,
            validate_issuer_url_for_provider, validate_redirect_url_for_provider,
        },
    },
    config::setting::{AzureSettings, ConfigError, GoogleSettings, OidcClient, Settings},
    dto::oauth::AuthProvider,
    models::{mcp_servers, sso_providers},
    services::ai_engine_catalog::reconcile_catalog_ai_engines,
    services::discovery_catalog::DiscoveryCatalog,
    services::live_models_cache::LiveModelsCache,
    services::mcp_client::McpServerClient,
    services::notifications::NotificationEvent,
};
use anyhow::Error;
use llm_plugin::ProviderRegistry;
use reqwest::Client as ReqwestClient;
use sea_orm::{ColumnTrait, Database, DatabaseConnection, EntityTrait, QueryFilter};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, RwLock, broadcast};
use uuid::Uuid;

pub struct AppState {
    pub database: DatabaseConnection,
    pub oidc_providers: RwLock<HashMap<String, OidcProviderRuntime>>,
    pub req_client: ReqwestClient,
    pub settings: Settings,
    pub mcp_clients: RwLock<HashMap<Uuid, Arc<McpServerClient>>>,
    pub notification_hub: broadcast::Sender<NotificationEvent>,
    pub stream_cancellations: RwLock<HashMap<Uuid, Arc<StreamCancel>>>,
    pub provider_registry: ProviderRegistry,
    pub live_models_cache: LiveModelsCache,
    pub discovery_catalog: DiscoveryCatalog,
    deployment_google: Option<GoogleSettings>,
    deployment_azure: Option<AzureSettings>,
}

#[derive(Clone)]
pub struct OidcProviderRuntime {
    pub client: Option<AuthProtocolClient>,
    pub azure_multitenant_validation: Option<AzureMultitenantValidation>,
    pub redirect_url: String,
    pub allowed_domains: Vec<String>,
    pub is_enabled: bool,
    pub use_grengin_proxy: bool,
    pub jit_provisioning: bool,
    pub configuration: OidcProviderConfiguration,
    pub azure_public_client: Option<AzurePublicClientConfig>,
}

#[derive(Clone)]
pub struct AzurePublicClientConfig {
    pub client_id: String,
    pub tenant_id: String,
}

#[derive(Clone)]
pub enum AuthProtocolClient {
    Oidc(OidcClient),
    GitHub(GitHubOAuthAdapter),
}

pub type SharedState = Arc<AppState>;

pub struct StreamCancel {
    cancelled: AtomicBool,
    notify: Notify,
}

impl StreamCancel {
    pub fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            notify: Notify::new(),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub async fn cancelled(&self) {
        self.notify.notified().await;
    }
}

impl AppState {
    pub async fn from_settings(mut settings: Settings) -> Result<SharedState, ConfigError> {
        let deployment_google = settings
            .google
            .read()
            .await
            .clone()
            .filter(|settings| settings.is_enabled && !settings.use_grengin_proxy);
        let deployment_azure = settings
            .azure
            .read()
            .await
            .clone()
            .filter(|settings| settings.is_enabled && !settings.use_grengin_proxy);
        let req_client = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| ConfigError::ReqwestClientBuildError(e.to_string()))?;
        let database = Database::connect(&settings.auth.database_url)
            .await
            .map_err(|e| ConfigError::DbError(e.to_string()))?;
        let discovery_catalog = DiscoveryCatalog::from_env(req_client.clone())
            .map_err(|error| ConfigError::Custom(error.to_string()))?;
        let _ = reconcile_catalog_ai_engines(&database, &discovery_catalog)
            .await
            .map_err(|error| eprintln!("Catalog AI engine reconciliation failed: {error:#}"));
        let _ = settings
            .load_ai_engines_from_db(&database)
            .await
            .map_err(|e| eprintln!("Loading ai engines from db error: {e}"));
        let _ = settings
            .load_sso_providers_from_db(&database)
            .await
            .map_err(|e| eprintln!("Loading sso providers from db error: {e}"));
        let _ = settings
            .load_embedding_config_from_db(&database)
            .await
            .map_err(|e| eprintln!("Loading embedding config from db error: {e}"));
        let (notification_hub, _) = broadcast::channel(256);
        let state = Self {
            database,
            oidc_providers: RwLock::new(HashMap::new()),
            req_client,
            settings,
            mcp_clients: RwLock::new(HashMap::new()),
            notification_hub,
            stream_cancellations: RwLock::new(HashMap::new()),
            provider_registry: ProviderRegistry::new(),
            live_models_cache: LiveModelsCache::new(),
            discovery_catalog,
            deployment_google,
            deployment_azure,
        };
        state.reload_oidc_providers().await?;
        let _ = state.load_mcp_servers_from_db().await;
        let _ = crate::services::provider_runtime::load_enabled_providers(&state)
            .await
            .map_err(|error| eprintln!("Loading custom AI engines failed: {error}"));
        Ok(Arc::new(state))
    }

    pub async fn check_sso_provider_is_enabled(&self, provider: &AuthProvider) -> Option<bool> {
        self.oidc_provider(provider)
            .await
            .map(|runtime| runtime.is_enabled)
    }

    pub async fn is_email_domain_allowed(
        &self,
        email: &str,
        provider: &AuthProvider,
    ) -> (bool, Option<String>) {
        if let Some((_, domain)) = email.split_once('@') {
            let Some(runtime) = self.oidc_provider(provider).await else {
                return (false, Some(domain.to_string()));
            };
            if runtime.allowed_domains.is_empty() {
                return (true, None);
            }
            let domain = domain.to_ascii_lowercase();
            return (
                runtime
                    .allowed_domains
                    .iter()
                    .any(|allowed| allowed.eq_ignore_ascii_case(&domain)),
                Some(domain),
            );
        }
        (false, None)
    }

    pub async fn sso_jit_provisioning_enabled(&self, provider: &AuthProvider) -> bool {
        self.oidc_provider(provider)
            .await
            .map(|runtime| runtime.jit_provisioning)
            .unwrap_or(false)
    }

    pub async fn check_ai_engine_is_enabled(&self, ai_engine_key: &str) -> Option<bool> {
        let key = ai_engine_key.to_lowercase();
        self.settings
            .ai_engines_cache
            .read()
            .await
            .get(&key)
            .map(|engine| engine.is_enabled)
    }

    pub async fn register_stream_cancel(&self, message_id: Uuid) -> Arc<StreamCancel> {
        let mut guard = self.stream_cancellations.write().await;
        let handle = Arc::new(StreamCancel::new());
        guard.insert(message_id, handle.clone());
        handle
    }

    pub async fn cancel_stream(&self, message_id: Uuid) -> bool {
        let guard = self.stream_cancellations.read().await;
        if let Some(handle) = guard.get(&message_id) {
            handle.cancel();
            true
        } else {
            false
        }
    }

    pub async fn clear_stream_cancel(&self, message_id: Uuid) {
        let mut guard = self.stream_cancellations.write().await;
        guard.remove(&message_id);
    }

    pub async fn get_oidc_provider_runtime(
        &self,
        provider: &AuthProvider,
    ) -> Result<OidcProviderRuntime, ConfigError> {
        self.oidc_provider(provider)
            .await
            .ok_or_else(|| ConfigError::InvalidSSoProvider(provider.into()))
    }

    pub async fn refresh_oidc_client(&self, provider: &AuthProvider) -> Result<(), Error> {
        let provider = provider.trim().to_ascii_lowercase();
        let model = sso_providers::Entity::find()
            .filter(sso_providers::Column::Provider.eq(&provider))
            .one(&self.database)
            .await?;
        let runtime = self
            .build_effective_oidc_provider_runtime(&provider, model.as_ref())
            .await;
        let mut runtimes = self.oidc_providers.write().await;
        match runtime {
            Ok(Some(runtime)) => {
                runtimes.insert(provider, runtime);
            }
            Ok(None) => {
                runtimes.remove(&provider);
            }
            Err(error) => {
                runtimes.remove(&provider);
                return Err(error);
            }
        }
        Ok(())
    }

    pub async fn oidc_provider(&self, provider: &str) -> Option<OidcProviderRuntime> {
        self.oidc_providers
            .read()
            .await
            .get(&provider.trim().to_ascii_lowercase())
            .cloned()
    }

    async fn reload_oidc_providers(&self) -> Result<(), ConfigError> {
        let models = sso_providers::Entity::find()
            .all(&self.database)
            .await
            .map_err(|error| ConfigError::DbError(error.to_string()))?;
        let mut runtimes = HashMap::new();
        let mut database_providers = HashSet::new();
        for model in models {
            let provider = model.provider.trim().to_ascii_lowercase();
            database_providers.insert(provider.clone());
            match self
                .build_effective_oidc_provider_runtime(&provider, Some(&model))
                .await
            {
                Ok(Some(runtime)) => {
                    runtimes.insert(provider, runtime);
                }
                Ok(None) => {}
                Err(error) => {
                    eprintln!(
                        "Skipping invalid OIDC provider '{}': {error:?}",
                        model.provider
                    );
                }
            }
        }
        for provider in ["google", "azure"] {
            if database_providers.contains(provider) {
                continue;
            }
            match self
                .build_effective_oidc_provider_runtime(provider, None)
                .await
            {
                Ok(Some(runtime)) => {
                    runtimes.insert(provider.to_string(), runtime);
                }
                Ok(None) => {}
                Err(error) => {
                    eprintln!("Skipping invalid deployment OIDC provider '{provider}': {error:?}");
                }
            }
        }
        *self.oidc_providers.write().await = runtimes;
        Ok(())
    }

    async fn build_effective_oidc_provider_runtime(
        &self,
        provider: &str,
        model: Option<&sso_providers::Model>,
    ) -> Result<Option<OidcProviderRuntime>, Error> {
        if let Some(model) = model.filter(|model| model.is_enabled) {
            return self.build_oidc_provider_runtime(model).await.map(Some);
        }

        if provider == "google"
            && let Some(settings) = self.deployment_google.clone()
        {
            return self
                .build_google_environment_runtime(settings, model)
                .await
                .map(Some);
        }
        if provider == "azure"
            && let Some(settings) = self.deployment_azure.clone()
        {
            return self
                .build_azure_environment_runtime(settings, model)
                .await
                .map(Some);
        }
        match model {
            Some(model) => self.build_oidc_provider_runtime(model).await.map(Some),
            None => Ok(None),
        }
    }

    async fn build_google_environment_runtime(
        &self,
        settings: GoogleSettings,
        policy: Option<&sso_providers::Model>,
    ) -> Result<OidcProviderRuntime, Error> {
        let configuration = OidcProviderConfiguration::from_value_for_provider(
            policy.and_then(|model| model.configuration.as_ref()),
            "google",
        )?;
        let client = build_google_client(
            &self.req_client,
            settings.client_id,
            settings.client_secret,
            settings.redirect_url.clone(),
        )
        .await?;
        Ok(OidcProviderRuntime {
            client: Some(AuthProtocolClient::Oidc(client)),
            azure_multitenant_validation: None,
            redirect_url: settings.redirect_url,
            allowed_domains: policy
                .map(|model| model.allowed_domains.clone())
                .unwrap_or(settings.allowed_domains),
            is_enabled: true,
            use_grengin_proxy: false,
            jit_provisioning: policy
                .map(|model| model.jit_provisioning)
                .unwrap_or(settings.jit_provisioning),
            configuration,
            azure_public_client: None,
        })
    }

    async fn build_azure_environment_runtime(
        &self,
        settings: AzureSettings,
        policy: Option<&sso_providers::Model>,
    ) -> Result<OidcProviderRuntime, Error> {
        let configuration = OidcProviderConfiguration::from_value_for_provider(
            policy.and_then(|model| model.configuration.as_ref()),
            "azure",
        )?;
        let public_client = AzurePublicClientConfig {
            client_id: settings.client_id.clone(),
            tenant_id: settings.tenant_id.clone(),
        };
        let azure = build_azure_client(
            &self.req_client,
            settings.client_id,
            settings.client_secret,
            settings.redirect_url.clone(),
            settings.tenant_id,
        )
        .await?;
        Ok(OidcProviderRuntime {
            client: Some(AuthProtocolClient::Oidc(azure.client)),
            azure_multitenant_validation: azure.multitenant_validation,
            redirect_url: settings.redirect_url,
            allowed_domains: policy
                .map(|model| model.allowed_domains.clone())
                .unwrap_or(settings.allowed_domains),
            is_enabled: true,
            use_grengin_proxy: false,
            jit_provisioning: policy
                .map(|model| model.jit_provisioning)
                .unwrap_or(settings.jit_provisioning),
            configuration,
            azure_public_client: Some(public_client),
        })
    }

    async fn build_oidc_provider_runtime(
        &self,
        model: &sso_providers::Model,
    ) -> Result<OidcProviderRuntime, Error> {
        let configuration = OidcProviderConfiguration::from_value_for_provider(
            model.configuration.as_ref(),
            &model.provider,
        )?;
        validate_issuer_url_for_provider(&model.provider, &model.issuer_url)?;
        validate_redirect_url_for_provider(&model.provider, &model.redirect_url)?;
        let (client, azure_multitenant_validation, azure_public_client) = if !model.is_enabled
            || model.use_grengin_proxy
        {
            (None, None, None)
        } else {
            let client_secret = decrypt_key(&self.settings.auth.app_key, &model.client_secret)
                .map_err(|error| anyhow::anyhow!("OIDC client secret decrypt failed: {error:?}"))?;
            match model.provider.as_str() {
                "azure" => {
                    let tenant_id = model
                        .tenant_id
                        .clone()
                        .unwrap_or_else(|| "common".to_string());
                    let azure = build_azure_client(
                        &self.req_client,
                        model.client_id.clone(),
                        client_secret,
                        model.redirect_url.clone(),
                        tenant_id.clone(),
                    )
                    .await?;
                    (
                        Some(AuthProtocolClient::Oidc(azure.client)),
                        azure.multitenant_validation,
                        Some(AzurePublicClientConfig {
                            client_id: model.client_id.clone(),
                            tenant_id,
                        }),
                    )
                }
                "google" => {
                    let client = build_google_client(
                        &self.req_client,
                        model.client_id.clone(),
                        client_secret,
                        model.redirect_url.clone(),
                    )
                    .await?;
                    (Some(AuthProtocolClient::Oidc(client)), None, None)
                }
                "github" => {
                    if !GitHubOAuthAdapter::supports_issuer(&model.issuer_url) {
                        return Err(anyhow::anyhow!("GitHub issuer must be https://github.com"));
                    }
                    (
                        Some(AuthProtocolClient::GitHub(GitHubOAuthAdapter::new(
                            model.client_id.clone(),
                            client_secret,
                            model.redirect_url.clone(),
                        )?)),
                        None,
                        None,
                    )
                }
                _ => {
                    let client = build_discovered_oidc_client(
                        &self.req_client,
                        &model.issuer_url,
                        model.client_id.clone(),
                        client_secret,
                        model.redirect_url.clone(),
                    )
                    .await?;
                    (Some(AuthProtocolClient::Oidc(client)), None, None)
                }
            }
        };
        Ok(OidcProviderRuntime {
            client,
            azure_multitenant_validation,
            redirect_url: model.redirect_url.clone(),
            allowed_domains: model.allowed_domains.clone(),
            is_enabled: model.is_enabled,
            use_grengin_proxy: model.use_grengin_proxy,
            jit_provisioning: model.jit_provisioning,
            configuration,
            azure_public_client,
        })
    }

    pub async fn load_mcp_servers_from_db(&self) -> Result<(), Error> {
        let servers = mcp_servers::Entity::find()
            .all(&self.database)
            .await
            .map_err(|e| Error::msg(e.to_string()))?;
        for server in servers {
            self.load_mcp_server_in_state(&server).await;
        }
        Ok(())
    }

    async fn load_mcp_server_in_state(&self, server: &mcp_servers::Model) {
        if !server.enabled {
            self.remove_mcp_client(&server.id).await;
            return;
        }
        let db_url = self.decrypt_mcp_db_url(&server.connection_config);
        let client = McpServerClient::new(
            server.id,
            server.name.clone(),
            server.transport_type,
            server.url.clone(),
            server.connection_config.clone(),
            db_url,
        );
        let mut clients = self.mcp_clients.write().await;
        clients.insert(server.id, Arc::new(client));
        println!("{} mcp server loaded", server.name);
    }

    fn decrypt_mcp_db_url(&self, connection_config: &serde_json::Value) -> Option<String> {
        let encrypted = connection_config.get("db_url")?.as_str()?;
        match decrypt_key(&self.settings.auth.app_key, encrypted) {
            Ok(value) => Some(value),
            Err(error) => {
                eprintln!("mcp db_url decrypt failed: {:?}", error);
                None
            }
        }
    }

    pub fn get_decrypted_api_key_preview(&self, api_key: &Option<String>) -> Option<String> {
        let api_key_preview = if let Some(api_key_encrypted) = api_key {
            let key = decrypt_key(&self.settings.auth.app_key, api_key_encrypted)
                .ok()
                .unwrap_or(String::new());
            if key.is_empty() {
                Some("<empty>".to_string())
            } else {
                let keep = 4;
                let chars: Vec<char> = key.chars().collect();
                let len = chars.len();
                if len <= keep * 2 {
                    Some(key.to_string())
                } else {
                    let start: String = chars.iter().take(keep).collect();
                    let end: String = chars.iter().skip(len - keep).collect();
                    Some(format!("{start}...{end}"))
                }
            }
        } else {
            Some("<empty>".to_string())
        };
        return api_key_preview;
    }

    /// Update MCP client cache when server metadata changes.
    pub async fn upsert_mcp_client(&self, server: &mcp_servers::Model) {
        self.load_mcp_server_in_state(server).await;
    }

    /// Drop MCP client cache entry.
    pub async fn remove_mcp_client(&self, server_id: &Uuid) {
        let mut clients = self.mcp_clients.write().await;
        clients.remove(server_id);
    }
}
