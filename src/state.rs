use crate::{
    auth::{azure::build_azure_client, encryption::decrypt_key, google::build_google_client},
    config::setting::{ConfigError, OidcClient, Settings},
    dto::oauth::AuthProvider,
    models::{mcp_servers, users},
    services::mcp_client::McpServerClient,
    services::notifications::NotificationEvent,
};
use anyhow::Error;
use reqwest::Client as ReqwestClient;
use sea_orm::{Database, DatabaseConnection, EntityTrait};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{Notify, RwLock, broadcast};
use uuid::Uuid;

pub struct AppState {
    pub database: DatabaseConnection,
    pub google_client: RwLock<Option<OidcClient>>,
    pub azure_client: RwLock<Option<OidcClient>>,
    pub req_client: ReqwestClient,
    pub settings: Settings,
    pub mcp_clients: RwLock<HashMap<Uuid, Arc<McpServerClient>>>,
    pub notification_hub: broadcast::Sender<NotificationEvent>,
    pub stream_cancellations: RwLock<HashMap<Uuid, Arc<StreamCancel>>>,
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
        let req_client = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| ConfigError::ReqwestClientBuildError(e.to_string()))?;
        let database = Database::connect(&settings.auth.database_url)
            .await
            .map_err(|e| ConfigError::DbError(e.to_string()))?;
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
            google_client: RwLock::new(None),
            azure_client: RwLock::new(None),
            req_client,
            settings,
            mcp_clients: RwLock::new(HashMap::new()),
            notification_hub,
            stream_cancellations: RwLock::new(HashMap::new()),
        };
        state.refresh_azure_client().await?;
        state.refresh_google_client().await?;
        let _ = state.load_mcp_servers_from_db().await;
        Ok(Arc::new(state))
    }

    pub async fn check_sso_provider_is_enabled(&self, provider: &AuthProvider) -> Option<bool> {
        match provider.to_lowercase().as_str() {
            "azure" => {
                let is_enabled = self
                    .settings
                    .azure
                    .read()
                    .await
                    .as_ref()
                    .map(|setting| setting.is_enabled);
                is_enabled
            }
            "google" => {
                let is_enabled = self
                    .settings
                    .google
                    .read()
                    .await
                    .as_ref()
                    .map(|setting| setting.is_enabled);
                is_enabled
            }
            _ => None,
        }
    }

    pub async fn is_email_domain_allowed(
        &self,
        email: &str,
        provider: &AuthProvider,
    ) -> (bool, Option<String>) {
        if let Some((_, domain)) = email.split_once('@') {
            match provider.to_lowercase().as_str() {
                "azure" => {
                    let allowed_domains = self
                        .settings
                        .azure
                        .read()
                        .await
                        .as_ref()
                        .map(|setting| setting.allowed_domains.clone())
                        .unwrap_or(Vec::new());
                    if allowed_domains.is_empty() {
                        return (true, None);
                    } else {
                        return (
                            allowed_domains.contains(&domain.to_string()),
                            Some(domain.to_string()),
                        );
                    }
                }
                "google" => {
                    let allowed_domains = self
                        .settings
                        .google
                        .read()
                        .await
                        .as_ref()
                        .map(|setting| setting.allowed_domains.clone())
                        .unwrap_or(Vec::new());
                    if allowed_domains.is_empty() {
                        return (true, None);
                    } else {
                        return (
                            allowed_domains.contains(&domain.to_string()),
                            Some(domain.to_string()),
                        );
                    }
                }
                _ => return (false, Some(domain.to_string())),
            }
        }
        (false, None)
    }

    pub async fn sso_jit_provisioning_enabled(&self, provider: &AuthProvider) -> bool {
        match provider.to_lowercase().as_str() {
            "azure" => self
                .settings
                .azure
                .read()
                .await
                .as_ref()
                .map(|s| s.jit_provisioning)
                .unwrap_or(true),
            "google" => self
                .settings
                .google
                .read()
                .await
                .as_ref()
                .map(|s| s.jit_provisioning)
                .unwrap_or(true),
            _ => true,
        }
    }

    pub async fn check_ai_engine_is_enabled(&self, ai_engine_key: &str) -> Option<bool> {
        match ai_engine_key.to_lowercase().as_str() {
            "openai" => {
                let is_enabled = self
                    .settings
                    .openai
                    .read()
                    .await
                    .as_ref()
                    .map(|setting| setting.is_enabled);
                is_enabled
            }
            "anthropic" => {
                let is_enabled = self
                    .settings
                    .anthropic
                    .read()
                    .await
                    .as_ref()
                    .map(|setting| setting.is_enabled);
                is_enabled
            }
            "mistral" => {
                let is_enabled = self
                    .settings
                    .mistral
                    .read()
                    .await
                    .as_ref()
                    .map(|setting| setting.is_enabled);
                is_enabled
            }
            "gemini" => {
                let is_enabled = self
                    .settings
                    .gemini
                    .read()
                    .await
                    .as_ref()
                    .map(|setting| setting.is_enabled);
                is_enabled
            }
            _ => None,
        }
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

    pub async fn get_oidc_client_and_column_and_redirect_uri(
        &self,
        provider: &AuthProvider,
    ) -> Result<(&RwLock<Option<OidcClient>>, users::Column, Option<String>), ConfigError> {
        match provider.to_lowercase().as_str() {
            "azure" => {
                let redirect_url = self
                    .settings
                    .azure
                    .read()
                    .await
                    .as_ref()
                    .map(|setting| setting.redirect_url.clone());
                return Ok((&self.azure_client, users::Column::AzureId, redirect_url));
            }
            "google" => {
                let redirect_url = self
                    .settings
                    .google
                    .read()
                    .await
                    .as_ref()
                    .map(|setting| setting.redirect_url.clone());
                return Ok((&self.google_client, users::Column::GoogleId, redirect_url));
            }
            _ => Err(ConfigError::InvalidSSoProvider(provider.into())),
        }
    }

    pub async fn refresh_oidc_client(&self, provider: &AuthProvider) -> Result<(), Error> {
        match provider.to_lowercase().as_str() {
            "azure" => self.refresh_azure_client().await?,
            "google" => self.refresh_google_client().await?,
            _ => return Err(anyhow::anyhow!("Unknown provider: {}", provider)),
        }
        Ok(())
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

    async fn refresh_google_client(&self) -> Result<(), ConfigError> {
        let google = self.settings.google.read().await.clone();
        let Some(google) = google else {
            *self.google_client.write().await = None;
            return Ok(());
        };
        let google_client = build_google_client(
            &self.req_client,
            google.client_id,
            google.client_secret,
            google.redirect_url,
        )
        .await;
        *self.google_client.write().await = google_client.ok();
        Ok(())
    }

    async fn refresh_azure_client(&self) -> Result<(), ConfigError> {
        let azure = self.settings.azure.read().await.clone();
        let Some(azure) = azure else {
            *self.azure_client.write().await = None;
            return Ok(());
        };
        let azure_client = build_azure_client(
            &self.req_client,
            azure.client_id,
            azure.client_secret,
            azure.redirect_url,
            azure.tenant_id,
        )
        .await;
        *self.azure_client.write().await = azure_client.ok();
        Ok(())
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


