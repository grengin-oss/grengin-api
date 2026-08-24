// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{
    collections::HashMap,
    sync::{RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use anyhow::{Error, anyhow};
use llm_plugin::ProviderManifestV1;

use crate::{models::ai_engines::PluginConfig, services::models_cache::load_plugin_urls_cached};

// std RwLock, not tokio: the read side is called from sync fns such as
// provider_plugin_version that sit inside non-async response mapping.
static MANIFEST_CACHE: RwLock<Option<HashMap<String, PluginConfig>>> = RwLock::new(None);

fn read_cache() -> RwLockReadGuard<'static, Option<HashMap<String, PluginConfig>>> {
    MANIFEST_CACHE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_cache() -> RwLockWriteGuard<'static, Option<HashMap<String, PluginConfig>>> {
    MANIFEST_CACHE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn cached_plugin_config(engine_key: &str) -> Option<PluginConfig> {
    read_cache().as_ref()?.get(engine_key).cloned()
}

pub fn invalidate(engine_key: &str) {
    if let Some(cache) = write_cache().as_mut() {
        cache.remove(engine_key);
    }
}

pub async fn catalog_plugin_config(
    req_client: &reqwest::Client,
    engine_key: &str,
) -> Result<PluginConfig, Error> {
    if let Some(config) = cached_plugin_config(engine_key) {
        return Ok(config);
    }
    let config = fetch_plugin_config(req_client, engine_key).await?;
    write_cache()
        .get_or_insert_with(HashMap::new)
        .insert(engine_key.to_string(), config.clone());
    Ok(config)
}

pub async fn prefetch(req_client: &reqwest::Client, engine_keys: &[String]) {
    let pending: Vec<&String> = engine_keys
        .iter()
        .filter(|key| cached_plugin_config(key).is_none())
        .collect();
    if pending.is_empty() {
        return;
    }
    let fetched =
        futures_util::future::join_all(pending.iter().map(|key| async move {
            (key.to_string(), fetch_plugin_config(req_client, key).await)
        }))
        .await;

    let mut guard = write_cache();
    let cache = guard.get_or_insert_with(HashMap::new);
    for (key, result) in fetched {
        match result {
            Ok(config) => {
                cache.insert(key, config);
            }
            Err(error) => eprintln!("catalog manifest for AI engine {key} was not loaded: {error}"),
        }
    }
}

async fn fetch_plugin_config(
    req_client: &reqwest::Client,
    engine_key: &str,
) -> Result<PluginConfig, Error> {
    let url = load_plugin_urls_cached(req_client)
        .await?
        .remove(engine_key)
        .ok_or_else(|| anyhow!("provider catalog declares no plugin url for {engine_key}"))?;
    let bytes = req_client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let manifest = ProviderManifestV1::from_json(&bytes)
        .map_err(|error| anyhow!("catalog manifest at {url} is invalid: {error}"))?;
    if manifest.id != engine_key {
        return Err(anyhow!(
            "catalog manifest at {url} declares id {} but is served for {engine_key}",
            manifest.id
        ));
    }
    Ok(PluginConfig {
        manifest: serde_json::to_value(&manifest)?,
        configuration: serde_json::json!({}),
        base_url_override: None,
        allow_insecure_http: false,
        allow_private_network: false,
    })
}
