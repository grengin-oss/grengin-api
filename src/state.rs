// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::{
        azure::{AzureMultitenantValidation, build_azure_client},
        encryption::decrypt_key,
        google::build_google_client,
        oauth2_identity::OAuth2IdentityAdapter,
        provider_config::{
            OidcProviderConfiguration, build_discovered_oidc_client,
            validate_issuer_url_for_provider, validate_redirect_url_for_provider,
        },
    },
    config::setting::{AzureSettings, ConfigError, GoogleSettings, OidcClient, Settings},
    dto::oauth::AuthProvider,
    models::{mcp_servers, messages::StopReason, sso_providers},
    services::discovery_catalog::DiscoveryCatalog,
    services::http_client::short_call_client,
    services::live_models_cache::LiveModelsCache,
    services::mcp_client::McpServerClient,
    services::notifications::NotificationEvent,
    services::owner_bootstrap::ensure_bootstrap_super_admin,
    services::reply_feed::ReplyFeed,
};
use anyhow::Error;
use chrono::{DateTime, Utc};
use llm_plugin::ProviderRegistry;
use reqwest::Client as ReqwestClient;
use sea_orm::{ColumnTrait, Database, DatabaseConnection, EntityTrait, QueryFilter};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{Notify, RwLock, broadcast};
use uuid::Uuid;

pub type StreamCancellations = Arc<RwLock<StreamRegistry>>;

// What an edit's reply shows while it generates: its new turn in place of the conversation from
// the edited message on. The turn is still hidden in the database until the edit commits.
#[derive(Clone, Debug, PartialEq)]
pub struct EditView {
    pub request: i32,
    pub cutoff: DateTime<Utc>,
    pub message_ids: Vec<Uuid>,
}

#[derive(Default)]
pub struct StreamRegistry {
    streams: HashMap<Uuid, Arc<StreamCancel>>,
    edit_views: HashMap<Uuid, EditView>,
    feeds: HashMap<Uuid, Arc<ReplyFeed>>,
    // Counts every reply that started or stopped generating, so a reader can tell whether the
    // set changed while it read the messages.
    changes: u64,
}

impl StreamRegistry {
    pub fn insert(&mut self, message_id: Uuid, handle: Arc<StreamCancel>) {
        self.changes += 1;
        self.streams.insert(message_id, handle);
    }

    pub fn remove(&mut self, message_id: &Uuid) {
        self.edit_views.remove(message_id);
        self.feeds.remove(message_id);
        if self.streams.remove(message_id).is_some() {
            self.changes += 1;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.streams.is_empty()
    }

    fn register(
        &mut self,
        message_id: Uuid,
        conversation_id: Uuid,
        edit_request: Option<i32>,
    ) -> Arc<StreamCancel> {
        self.changes += 1;
        register_stream(&mut self.streams, message_id, conversation_id, edit_request)
    }

    fn register_edit(
        &mut self,
        message_id: Uuid,
        conversation_id: Uuid,
        view: EditView,
    ) -> Arc<StreamCancel> {
        let handle = self.register(message_id, conversation_id, Some(view.request));
        // An older edit stopped by this one keeps its stream entry until it has saved, but must
        // never be shown again, even after this edit's reply leaves the registry.
        let streams = &self.streams;
        self.edit_views.retain(|other, other_view| {
            other_view.request >= view.request
                || streams
                    .get(other)
                    .is_none_or(|handle| handle.conversation_id != conversation_id)
        });
        let superseded = self
            .newest_edit(conversation_id)
            .is_some_and(|newest| newest.request > view.request);
        if !superseded {
            self.edit_views.insert(message_id, view);
        }
        handle
    }

    fn generating(&self, conversation_id: Uuid) -> GeneratingSnapshot {
        GeneratingSnapshot {
            changes: self.changes,
            replies: generating_in(&self.streams, conversation_id),
            edit: self.newest_edit(conversation_id),
        }
    }

    // The newest edit replying in this conversation is the one the user sees.
    fn newest_edit(&self, conversation_id: Uuid) -> Option<EditView> {
        self.edit_views
            .iter()
            .filter(|(message_id, _)| {
                self.streams
                    .get(message_id)
                    .is_some_and(|handle| handle.conversation_id == conversation_id)
            })
            .map(|(_, view)| view)
            .max_by_key(|view| view.request)
            .cloned()
    }
}

pub struct GeneratingSnapshot {
    changes: u64,
    pub replies: HashSet<Uuid>,
    pub edit: Option<EditView>,
}

impl GeneratingSnapshot {
    /// Messages read between `self` and `after` match `self.replies` when no reply started or
    /// stopped in between: a reply registers before its row exists and leaves only after its
    /// final text is saved. Otherwise `None`, unless this is the last attempt: a reply that
    /// started and finished in between is the newest one and may have been read half-written,
    /// so it is reported as generating too and the next read settles it.
    pub fn settle(
        self,
        after: GeneratingSnapshot,
        newest_reply: Option<Uuid>,
        last_attempt: bool,
    ) -> Option<HashSet<Uuid>> {
        if self.changes == after.changes {
            return Some(self.replies);
        }
        last_attempt.then(|| {
            let mut replies = self.replies;
            replies.extend(after.replies);
            replies.extend(newest_reply);
            replies
        })
    }
}

pub struct AppState {
    pub database: DatabaseConnection,
    pub share_rate_limiter: crate::services::share_access::ShareRateLimiter,
    pub share_fork_slots: Arc<tokio::sync::Semaphore>,
    pub share_index_slots: Arc<tokio::sync::Semaphore>,
    pub oidc_providers: RwLock<HashMap<String, OidcProviderRuntime>>,
    pub req_client: ReqwestClient,
    pub settings: Settings,
    pub mcp_clients: RwLock<HashMap<Uuid, Arc<McpServerClient>>>,
    pub mcp_refresh_locks: crate::services::mcp_oauth::OAuthRefreshLocks,
    pub notification_hub: broadcast::Sender<NotificationEvent>,
    pub stream_cancellations: StreamCancellations,
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
    OAuth2(OAuth2IdentityAdapter),
}

pub type SharedState = Arc<AppState>;

pub struct StreamCancel {
    conversation_id: Uuid,
    edit_request: Option<i32>,
    cancelled: AtomicBool,
    reason: std::sync::Mutex<Option<StopReason>>,
    notify: Notify,
}

impl StreamCancel {
    pub fn new(conversation_id: Uuid) -> Self {
        Self {
            conversation_id,
            edit_request: None,
            cancelled: AtomicBool::new(false),
            reason: std::sync::Mutex::new(None),
            notify: Notify::new(),
        }
    }

    fn for_edit(conversation_id: Uuid, request: i32) -> Self {
        Self {
            edit_request: Some(request),
            ..Self::new(conversation_id)
        }
    }

    fn is_older_than(&self, request: i32) -> bool {
        self.edit_request.is_none_or(|own| own < request)
    }

    // The first reason wins, so a user stop that races an edit is still reported as a stop.
    pub fn cancel(&self, reason: StopReason) {
        if let Ok(mut current) = self.reason.lock() {
            current.get_or_insert(reason);
        }
        self.cancelled.store(true, Ordering::Relaxed);
        self.notify.notify_waiters();
    }

    pub fn reason(&self) -> Option<StopReason> {
        self.reason.lock().ok().and_then(|reason| *reason)
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub async fn cancelled(&self) {
        // notify_waiters keeps no permit, so a cancel sent while nobody was waiting
        // (e.g. during MCP tool execution) is only visible through the flag.
        let notified = self.notify.notified();
        if self.is_cancelled() {
            return;
        }
        notified.await;
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
        let req_client =
            short_call_client().map_err(|e| ConfigError::ReqwestClientBuildError(e.to_string()))?;
        let database = Database::connect(&settings.auth.database_url)
            .await
            .map_err(|e| ConfigError::DbError(e.to_string()))?;
        ensure_bootstrap_super_admin(
            &database,
            settings.auth.bootstrap_super_admin_email.as_deref(),
        )
        .await
        .map_err(ConfigError::Custom)?;
        let discovery_catalog = DiscoveryCatalog::from_env(req_client.clone())
            .map_err(|error| ConfigError::Custom(error.to_string()))?;
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
        crate::services::grengin_grant::GrenginGrant::attach_database(database.clone());
        let state = Self {
            database,
            oidc_providers: RwLock::new(HashMap::new()),
            req_client,
            settings,
            mcp_clients: RwLock::new(HashMap::new()),
            share_rate_limiter: Default::default(),
            share_fork_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            share_index_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            mcp_refresh_locks: Default::default(),
            notification_hub,
            stream_cancellations: StreamCancellations::default(),
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

    pub async fn register_stream_cancel(
        &self,
        message_id: Uuid,
        conversation_id: Uuid,
        edit: Option<EditView>,
        feed: Arc<ReplyFeed>,
    ) -> Arc<StreamCancel> {
        let mut registry = self.stream_cancellations.write().await;
        let handle = match edit {
            Some(view) => registry.register_edit(message_id, conversation_id, view),
            None => registry.register(message_id, conversation_id, None),
        };
        registry.feeds.insert(message_id, feed);
        handle
    }

    // The recorded events of a reply still generating here, for a page that lost its stream.
    pub async fn reply_feed(&self, message_id: Uuid) -> Option<Arc<ReplyFeed>> {
        self.stream_cancellations
            .read()
            .await
            .feeds
            .get(&message_id)
            .cloned()
    }

    pub async fn release_stream_cancel(&self, message_id: Uuid) {
        self.stream_cancellations.write().await.remove(&message_id);
    }

    pub async fn cancel_stream(&self, message_id: Uuid) -> bool {
        let guard = self.stream_cancellations.read().await;
        if let Some(handle) = guard.streams.get(&message_id) {
            handle.cancel(StopReason::UserCancelled);
            true
        } else {
            false
        }
    }

    pub async fn generating_replies(&self, conversation_id: Uuid) -> GeneratingSnapshot {
        self.stream_cancellations
            .read()
            .await
            .generating(conversation_id)
    }

    pub async fn interrupt_older_streams(&self, conversation_id: Uuid, edit_request: i32) {
        let guard = self.stream_cancellations.read().await;
        interrupt_older_streams(&guard.streams, conversation_id, edit_request);
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
            match (configuration.oauth2.clone(), model.provider.as_str()) {
                (Some(profile), _) => {
                    if !profile.supports_issuer(&model.issuer_url) {
                        return Err(anyhow::anyhow!(
                            "OAuth2 issuer must be the origin of its authorization URL"
                        ));
                    }
                    (
                        Some(AuthProtocolClient::OAuth2(OAuth2IdentityAdapter::new(
                            profile,
                            model.client_id.clone(),
                            client_secret,
                            model.redirect_url.clone(),
                        )?)),
                        None,
                        None,
                    )
                }
                (None, "azure") => {
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
                (None, "google") => {
                    let client = build_google_client(
                        &self.req_client,
                        model.client_id.clone(),
                        client_secret,
                        model.redirect_url.clone(),
                    )
                    .await?;
                    (Some(AuthProtocolClient::Oidc(client)), None, None)
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

// Edits follow the same order as their claims: an edit stops the plain replies and older edits
// of its conversation, and one that finds a newer edit already replying stops itself. Deciding
// under the lock that inserts the stream makes the outcome independent of arrival order.
fn register_stream(
    streams: &mut HashMap<Uuid, Arc<StreamCancel>>,
    message_id: Uuid,
    conversation_id: Uuid,
    edit_request: Option<i32>,
) -> Arc<StreamCancel> {
    let handle = Arc::new(match edit_request {
        Some(request) => StreamCancel::for_edit(conversation_id, request),
        None => StreamCancel::new(conversation_id),
    });
    if let Some(request) = edit_request {
        let newer_edit_running = streams
            .values()
            .any(|other| other.conversation_id == conversation_id && !other.is_older_than(request));
        if newer_edit_running {
            handle.cancel(StopReason::UserEdited);
        } else {
            interrupt_older_streams(streams, conversation_id, request);
        }
    }
    streams.insert(message_id, handle.clone());
    handle
}

fn generating_in(
    streams: &HashMap<Uuid, Arc<StreamCancel>>,
    conversation_id: Uuid,
) -> HashSet<Uuid> {
    streams
        .iter()
        .filter(|(_, handle)| handle.conversation_id == conversation_id)
        .map(|(message_id, _)| *message_id)
        .collect()
}

fn interrupt_older_streams(
    streams: &HashMap<Uuid, Arc<StreamCancel>>,
    conversation_id: Uuid,
    edit_request: i32,
) -> Vec<Uuid> {
    streams
        .iter()
        .filter(|(_, handle)| {
            handle.conversation_id == conversation_id && handle.is_older_than(edit_request)
        })
        .map(|(message_id, handle)| {
            handle.cancel(StopReason::UserEdited);
            *message_id
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        StreamCancel, StreamRegistry, generating_in, interrupt_older_streams, register_stream,
    };
    use crate::models::messages::StopReason;
    use std::{collections::HashMap, sync::Arc};
    use uuid::Uuid;

    #[test]
    fn only_replies_still_registered_in_that_conversation_are_generating() {
        let mut streams = HashMap::new();
        let conversation = Uuid::new_v4();
        let reply = Uuid::new_v4();
        let elsewhere = Uuid::new_v4();
        register_stream(&mut streams, reply, conversation, None);
        register_stream(&mut streams, elsewhere, Uuid::new_v4(), None);
        assert_eq!(generating_in(&streams, conversation), [reply].into());
        // A stopped reply still settles its final text before it leaves the registry.
        streams[&reply].cancel(StopReason::UserCancelled);
        assert!(generating_in(&streams, conversation).contains(&reply));
        streams.remove(&reply);
        assert!(generating_in(&streams, conversation).is_empty());
    }

    #[test]
    fn a_reply_that_starts_while_messages_are_read_is_never_reported_final() {
        let mut registry = StreamRegistry::default();
        let conversation = Uuid::new_v4();
        let reply = Uuid::new_v4();
        let before = registry.generating(conversation);
        // The reply registers and its row is written after the first snapshot, before the read.
        registry.register(reply, conversation, None);
        let after = registry.generating(conversation);
        let retried = registry.generating(conversation);
        assert!(!before.replies.contains(&reply));
        assert_eq!(before.settle(after, Some(reply), false), None);
        // The retry sees no change while it reads, and finds the reply generating.
        let settled = retried.settle(registry.generating(conversation), Some(reply), false);
        assert_eq!(settled, Some([reply].into()));
    }

    #[test]
    fn a_reply_that_starts_and_finishes_while_messages_are_read_stays_generating() {
        let mut registry = StreamRegistry::default();
        let conversation = Uuid::new_v4();
        let reply = Uuid::new_v4();
        let before = registry.generating(conversation);
        registry.register(reply, conversation, None);
        registry.remove(&reply);
        let after = registry.generating(conversation);
        assert!(before.replies.is_empty() && after.replies.is_empty());
        // Out of attempts, the half-read newest reply is reported generating; the next poll settles it.
        assert_eq!(
            before.settle(after, Some(reply), true),
            Some([reply].into())
        );
    }

    #[test]
    fn a_reply_that_finishes_while_messages_are_read_is_read_again() {
        let mut registry = StreamRegistry::default();
        let conversation = Uuid::new_v4();
        let reply = Uuid::new_v4();
        registry.register(reply, conversation, None);
        let before = registry.generating(conversation);
        registry.remove(&reply);
        let after = registry.generating(conversation);
        assert!(before.replies.contains(&reply));
        assert_eq!(before.settle(after, Some(reply), false), None);
        let settled = registry.generating(conversation).settle(
            registry.generating(conversation),
            Some(reply),
            false,
        );
        assert_eq!(settled, Some([].into()));
    }

    #[test]
    fn other_conversations_and_unknown_releases_do_not_hide_changes() {
        let mut registry = StreamRegistry::default();
        let conversation = Uuid::new_v4();
        let before = registry.generating(conversation);
        registry.remove(&Uuid::new_v4());
        assert!(
            before
                .settle(registry.generating(conversation), None, false)
                .is_some()
        );
        let before = registry.generating(conversation);
        registry.register(Uuid::new_v4(), Uuid::new_v4(), None);
        // A change anywhere forces a re-read: cheap, and it never reports a partial reply as final.
        assert!(
            before
                .settle(registry.generating(conversation), None, false)
                .is_none()
        );
    }

    #[test]
    fn cancel_records_why_the_stream_stopped() {
        let handle = StreamCancel::new(Uuid::new_v4());
        assert_eq!(handle.reason(), None);
        handle.cancel(StopReason::UserCancelled);
        assert!(handle.is_cancelled());
        assert_eq!(handle.reason(), Some(StopReason::UserCancelled));
    }

    #[test]
    fn first_cancel_reason_wins() {
        let handle = StreamCancel::new(Uuid::new_v4());
        handle.cancel(StopReason::UserCancelled);
        handle.cancel(StopReason::UserEdited);
        assert_eq!(handle.reason(), Some(StopReason::UserCancelled));
    }

    #[test]
    fn editing_cancels_only_streams_in_that_conversation() {
        let edited = Uuid::new_v4();
        let other = Uuid::new_v4();
        let in_edited = Uuid::new_v4();
        let in_other = Uuid::new_v4();
        let streams = HashMap::from([
            (in_edited, Arc::new(StreamCancel::new(edited))),
            (in_other, Arc::new(StreamCancel::new(other))),
        ]);

        let cancelled = interrupt_older_streams(&streams, edited, 1);

        assert_eq!(cancelled, vec![in_edited]);
        assert_eq!(streams[&in_edited].reason(), Some(StopReason::UserEdited));
        assert!(!streams[&in_other].is_cancelled());
    }

    #[test]
    fn an_edit_starting_to_reply_interrupts_its_conversation_but_not_itself() {
        let edited = Uuid::new_v4();
        let running = Uuid::new_v4();
        let elsewhere = Uuid::new_v4();
        let mut streams = HashMap::from([
            (running, Arc::new(StreamCancel::new(edited))),
            (elsewhere, Arc::new(StreamCancel::new(Uuid::new_v4()))),
        ]);

        let edit = register_stream(&mut streams, Uuid::new_v4(), edited, Some(1));

        assert_eq!(streams[&running].reason(), Some(StopReason::UserEdited));
        assert!(!streams[&elsewhere].is_cancelled());
        assert!(!edit.is_cancelled());
        assert_eq!(streams.len(), 3);
    }

    #[test]
    fn a_plain_reply_interrupts_nothing() {
        let conversation = Uuid::new_v4();
        let running = Uuid::new_v4();
        let mut streams = HashMap::from([(running, Arc::new(StreamCancel::new(conversation)))]);

        register_stream(&mut streams, Uuid::new_v4(), conversation, None);

        assert!(!streams[&running].is_cancelled());
    }

    #[test]
    fn a_newer_edit_registering_second_stops_the_older_one() {
        let conversation = Uuid::new_v4();
        let mut streams = HashMap::new();

        let older = register_stream(&mut streams, Uuid::new_v4(), conversation, Some(1));
        let newer = register_stream(&mut streams, Uuid::new_v4(), conversation, Some(2));

        assert_eq!(older.reason(), Some(StopReason::UserEdited));
        assert!(!newer.is_cancelled());
    }

    #[test]
    fn an_older_edit_registering_second_stops_itself_and_leaves_the_newer_one() {
        let conversation = Uuid::new_v4();
        let mut streams = HashMap::new();

        let newer = register_stream(&mut streams, Uuid::new_v4(), conversation, Some(2));
        let older = register_stream(&mut streams, Uuid::new_v4(), conversation, Some(1));

        assert_eq!(older.reason(), Some(StopReason::UserEdited));
        assert!(!newer.is_cancelled());
    }

    #[test]
    fn edits_of_other_conversations_never_interact() {
        let mut streams = HashMap::new();

        let newer = register_stream(&mut streams, Uuid::new_v4(), Uuid::new_v4(), Some(2));
        let older = register_stream(&mut streams, Uuid::new_v4(), Uuid::new_v4(), Some(1));

        assert!(!newer.is_cancelled());
        assert!(!older.is_cancelled());
    }

    #[test]
    fn an_older_image_edit_does_not_stop_a_newer_edit() {
        let conversation = Uuid::new_v4();
        let plain = Uuid::new_v4();
        let newer = Uuid::new_v4();
        let streams = HashMap::from([
            (plain, Arc::new(StreamCancel::new(conversation))),
            (newer, Arc::new(StreamCancel::for_edit(conversation, 3))),
        ]);

        let stopped = interrupt_older_streams(&streams, conversation, 2);

        assert_eq!(stopped, vec![plain]);
        assert!(!streams[&newer].is_cancelled());
    }

    #[test]
    fn the_newest_running_edit_is_the_one_shown_and_leaves_with_its_reply() {
        let conversation = Uuid::new_v4();
        let view = |request| super::EditView {
            request,
            cutoff: chrono::Utc::now(),
            message_ids: vec![Uuid::new_v4()],
        };
        let mut registry = StreamRegistry::default();
        assert_eq!(registry.generating(conversation).edit, None);
        let older = Uuid::new_v4();
        let newer = Uuid::new_v4();
        registry.register_edit(older, conversation, view(1));
        registry.register_edit(newer, conversation, view(2));
        registry.register_edit(Uuid::new_v4(), Uuid::new_v4(), view(9));
        let snapshot = registry.generating(conversation);
        assert_eq!(snapshot.edit.map(|edit| edit.request), Some(2));
        assert_eq!(snapshot.replies.len(), 2);

        registry.remove(&newer);
        let snapshot = registry.generating(conversation);
        assert_eq!(
            snapshot.edit, None,
            "the stopped older edit is never shown again"
        );
        assert_eq!(
            snapshot.replies.len(),
            1,
            "it still counts as generating until saved"
        );
        registry.remove(&older);
        assert_eq!(registry.generating(conversation).edit, None);
        assert!(
            registry.edit_views.len() == 1,
            "only the other conversation's edit remains"
        );
    }

    #[test]
    fn an_older_edit_registering_late_is_not_shown_over_a_newer_one() {
        let conversation = Uuid::new_v4();
        let view = |request| super::EditView {
            request,
            cutoff: chrono::Utc::now(),
            message_ids: vec![Uuid::new_v4()],
        };
        let mut registry = StreamRegistry::default();
        let newer = Uuid::new_v4();
        registry.register_edit(newer, conversation, view(3));
        registry.register_edit(Uuid::new_v4(), conversation, view(2));
        assert_eq!(
            registry
                .generating(conversation)
                .edit
                .map(|edit| edit.request),
            Some(3)
        );
        registry.remove(&newer);
        assert_eq!(registry.generating(conversation).edit, None);
    }
}
