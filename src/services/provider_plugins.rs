// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{collections::BTreeMap, sync::Arc};

use llm_plugin::{
    DeclarativeProvider, ProviderError, ProviderId, ProviderManifestV1, ProviderRuntimeConfig,
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use thiserror::Error;

use crate::{
    auth::encryption::decrypt_key,
    models::{provider_credentials, provider_plugins},
    state::AppState,
};

#[derive(Debug, Error)]
pub enum ProviderPluginLoadError {
    #[error("database operation failed")]
    Database,
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error("provider credential could not be decrypted")]
    CredentialDecryption,
}

pub async fn build_provider(
    database: &DatabaseConnection,
    app_key: &[u8; 32],
    plugin: &provider_plugins::Model,
) -> Result<DeclarativeProvider, ProviderPluginLoadError> {
    let manifest_bytes = serde_json::to_vec(&plugin.manifest).map_err(|error| {
        ProviderError::InvalidManifest(format!("manifest serialization failed: {error}"))
    })?;
    let manifest = ProviderManifestV1::from_json(&manifest_bytes)?;
    let credential_rows = provider_credentials::Entity::find()
        .filter(provider_credentials::Column::PluginId.eq(plugin.id))
        .all(database)
        .await
        .map_err(|_| ProviderPluginLoadError::Database)?;
    let mut credentials = BTreeMap::new();
    for credential in credential_rows {
        let value = decrypt_key(app_key, &credential.encrypted_value)
            .map_err(|_| ProviderPluginLoadError::CredentialDecryption)?;
        credentials.insert(credential.slot_id, value);
    }
    DeclarativeProvider::new(
        manifest,
        ProviderRuntimeConfig {
            base_url_override: plugin.base_url_override.clone(),
            configuration: plugin.configuration.clone(),
            credentials,
            allow_insecure_http: plugin.allow_insecure_http,
            allow_private_network: plugin.allow_private_network,
            ..Default::default()
        },
    )
    .map_err(ProviderPluginLoadError::from)
}

pub async fn register_provider(
    state: &AppState,
    plugin: &provider_plugins::Model,
) -> Result<(), ProviderPluginLoadError> {
    let provider = build_provider(&state.database, &state.settings.auth.app_key, plugin).await?;
    state.provider_registry.register(Arc::new(provider)).await;
    Ok(())
}

pub async fn unregister_provider(state: &AppState, provider_key: &str) {
    state
        .provider_registry
        .remove(&ProviderId::new(provider_key))
        .await;
}

pub async fn load_enabled_providers(state: &AppState) -> Result<(), ProviderPluginLoadError> {
    let plugins = provider_plugins::Entity::find()
        .filter(
            provider_plugins::Column::Status.eq(provider_plugins::ProviderPluginStatus::Enabled),
        )
        .all(&state.database)
        .await
        .map_err(|_| ProviderPluginLoadError::Database)?;
    for plugin in plugins {
        if let Err(error) = register_provider(state, &plugin).await {
            eprintln!(
                "provider plugin {} was not loaded: {}",
                plugin.provider_key,
                error_class(&error)
            );
        }
    }
    Ok(())
}

fn error_class(error: &ProviderPluginLoadError) -> &'static str {
    match error {
        ProviderPluginLoadError::Database => "database",
        ProviderPluginLoadError::CredentialDecryption => "credential_decryption",
        ProviderPluginLoadError::Provider(_) => "provider_configuration",
    }
}
