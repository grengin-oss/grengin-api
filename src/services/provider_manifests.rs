// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{
    collections::HashMap,
    sync::{RwLock, RwLockReadGuard, RwLockWriteGuard},
    time::{Duration, Instant},
};

use anyhow::{Error, anyhow};
use llm_plugin::ProviderManifestV1;

use crate::{
    models::ai_engines::{PluginConfig, PluginConfigSource},
    services::discovery_catalog::DiscoveryCatalog,
};

// std RwLock, not tokio: the read side is called from sync fns such as
// provider_plugin_version that sit inside non-async response mapping.
const MANIFEST_TTL: Duration = Duration::from_secs(300);

#[derive(Clone)]
struct CacheEntry {
    config: PluginConfig,
    fetched_at: Instant,
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
    let cache = read_cache();
    let entry = cache.as_ref()?.get(engine_key)?;
    is_fresh(entry.fetched_at, Instant::now()).then(|| entry.config.clone())
}

fn is_fresh(fetched_at: Instant, now: Instant) -> bool {
    now.saturating_duration_since(fetched_at) < MANIFEST_TTL
}

pub fn invalidate(engine_key: &str) {
    if let Some(cache) = write_cache().as_mut() {
        cache.remove(engine_key);
    }
}

pub async fn catalog_plugin_config(
    catalog: &DiscoveryCatalog,
    engine_key: &str,
) -> Result<PluginConfig, Error> {
    if let Some(config) = cached_plugin_config(engine_key) {
        return Ok(config);
    }
    let config = fetch_plugin_config(catalog, engine_key, None).await?;
    write_cache().get_or_insert_with(HashMap::new).insert(
        engine_key.to_string(),
        CacheEntry {
            config: config.clone(),
            fetched_at: Instant::now(),
        },
    );
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
    let fetched = futures_util::future::join_all(pending.iter().map(|key| async move {
        (key.to_string(), fetch_plugin_config(catalog, key, None).await)
    }))
    .await;

    let mut guard = write_cache();
    let cache = guard.get_or_insert_with(HashMap::new);
    for (key, result) in fetched {
        match result {
            Ok(config) => {
                cache.insert(
                    key,
                    CacheEntry {
                        config,
                        fetched_at: Instant::now(),
                    },
                );
            }
            Err(error) => eprintln!("catalog manifest for AI engine {key} was not loaded: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{MANIFEST_TTL, is_fresh};

    #[test]
    fn catalog_manifest_cache_expires_at_the_ttl() {
        let fetched_at = Instant::now();
        assert!(is_fresh(
            fetched_at,
            fetched_at + MANIFEST_TTL - Duration::from_millis(1)
        ));
        assert!(!is_fresh(fetched_at, fetched_at + MANIFEST_TTL));
    }
}

// Resolves through the versioned, digest-verified distribution index rather than
// the unversioned `plugin.json` mirror: the mirror always serves whatever was
// last published, which can be newer than the major.minor this grengin-api
// release's distribution pins, silently drifting a running provider onto an
// unvetted manifest. `requested_version` is `None` for normal resolution (the
// distribution's pinned default) and `Some(v)` only for an explicit admin
// upgrade/rollback to a specific compatible version.
pub async fn fetch_plugin_config(
    catalog: &DiscoveryCatalog,
    engine_key: &str,
    requested_version: Option<&str>,
) -> Result<PluginConfig, Error> {
    let package = catalog
        .ai_provider(engine_key, requested_version)
        .await
        .map_err(|error| anyhow!("catalog manifest for {engine_key} unavailable: {error}"))?;
    let manifest = ProviderManifestV1::from_json(
        serde_json::to_string(&package.plugin)
            .map_err(|error| anyhow!("catalog manifest for {engine_key} is malformed: {error}"))?
            .as_bytes(),
    )
    .map_err(|error| anyhow!("catalog manifest for {engine_key} is invalid: {error}"))?;
    if manifest.id != engine_key {
        return Err(anyhow!(
            "catalog manifest for {engine_key} declares id {} instead",
            manifest.id
        ));
    }
    Ok(PluginConfig {
        manifest: package.plugin,
        configuration: serde_json::json!({}),
        base_url_override: None,
        allow_insecure_http: false,
        allow_private_network: false,
        source: PluginConfigSource::Catalog,
        version: Some(package.version),
        sha256: Some(package.sha256),
    })
}
