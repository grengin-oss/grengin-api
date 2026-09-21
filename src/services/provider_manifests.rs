// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{
    collections::HashMap,
    sync::{RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use anyhow::{Error, anyhow};
use llm_plugin::ProviderManifestV1;

use crate::{models::ai_engines::PluginConfig, services::discovery_catalog::DiscoveryCatalog};

// std RwLock, not tokio: the read side is called from sync fns such as
// provider_plugin_version that sit inside non-async response mapping.
#[derive(Clone)]
pub struct CatalogPlugin {
    pub config: PluginConfig,
    pub version: String,
    pub sha256: String,
}

#[derive(Clone)]
struct CacheEntry {
    plugin: CatalogPlugin,
}

static MANIFEST_CACHE: RwLock<Option<HashMap<String, CacheEntry>>> = RwLock::new(None);

fn read_cache() -> RwLockReadGuard<'static, Option<HashMap<String, CacheEntry>>> {
    MANIFEST_CACHE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_cache() -> RwLockWriteGuard<'static, Option<HashMap<String, CacheEntry>>> {
    MANIFEST_CACHE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn cached_plugin_config(engine_key: &str) -> Option<PluginConfig> {
    cached_catalog_plugin(engine_key).map(|plugin| plugin.config)
}

pub fn cached_catalog_plugin(engine_key: &str) -> Option<CatalogPlugin> {
    read_cache()
        .as_ref()?
        .get(&engine_key.to_ascii_lowercase())
        .map(|entry| entry.plugin.clone())
}

pub fn install_catalog_plugin(engine_key: &str, plugin: CatalogPlugin) {
    write_cache()
        .get_or_insert_with(HashMap::new)
        .insert(engine_key.to_ascii_lowercase(), CacheEntry { plugin });
}

pub fn invalidate(engine_key: &str) {
    if let Some(cache) = write_cache().as_mut() {
        cache.remove(&engine_key.to_ascii_lowercase());
    }
}

pub async fn catalog_plugin_config(
    catalog: &DiscoveryCatalog,
    engine_key: &str,
) -> Result<PluginConfig, Error> {
    if let Some(config) = cached_plugin_config(engine_key) {
        return Ok(config);
    }
    let plugin = fetch_catalog_plugin(catalog, engine_key).await?;
    let config = plugin.config.clone();
    install_catalog_plugin(engine_key, plugin);
    Ok(config)
}

pub async fn prefetch(catalog: &DiscoveryCatalog, engine_keys: &[String]) {
    let pending: Vec<&String> = engine_keys
        .iter()
        .filter(|key| cached_plugin_config(key).is_none())
        .collect();
    if pending.is_empty() {
        return;
    }
    let fetched = futures_util::future::join_all(
        pending
            .iter()
            .map(|key| async move { (key.to_string(), fetch_catalog_plugin(catalog, key).await) }),
    )
    .await;

    for (key, result) in fetched {
        match result {
            Ok(plugin) => install_catalog_plugin(&key, plugin),
            Err(error) => eprintln!("catalog manifest for AI engine {key} was not loaded: {error}"),
        }
    }
}

pub async fn fetch_catalog_plugin(
    catalog: &DiscoveryCatalog,
    engine_key: &str,
) -> Result<CatalogPlugin, Error> {
    let package = catalog.ai_provider(engine_key, None).await?;
    let bytes = serde_json::to_vec(&package.plugin)?;
    let manifest = ProviderManifestV1::from_json(&bytes)
        .map_err(|error| anyhow!("catalog manifest for {engine_key} is invalid: {error}"))?;
    if manifest.id != engine_key {
        return Err(anyhow!(
            "catalog manifest for {engine_key} declares id {}",
            manifest.id
        ));
    }
    Ok(CatalogPlugin {
        config: PluginConfig {
            manifest: package.plugin,
            configuration: serde_json::json!({}),
            base_url_override: None,
            allow_insecure_http: false,
            allow_private_network: false,
        },
        version: package.version,
        sha256: package.sha256,
    })
}
