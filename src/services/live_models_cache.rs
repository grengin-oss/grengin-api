// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use llm_plugin::{ProviderError, ProviderModel, ProviderPlugin};
use tokio::sync::RwLock;

const LIVE_MODELS_TTL: Duration = Duration::from_secs(300);

struct CacheEntry {
    models: Vec<ProviderModel>,
    fetched_at: Instant,
}

/// Caches live `listModels` results per engine so `GET /models` and per-message
/// pricing lookups don't hit the provider's real endpoint on every request.
/// Entries expire after `LIVE_MODELS_TTL`, and are also invalidated explicitly
/// whenever an engine's plugin_config/api_key changes or the engine is deleted.
#[derive(Default)]
pub struct LiveModelsCache {
    entries: RwLock<HashMap<String, CacheEntry>>,
}

impl LiveModelsCache {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    pub async fn get_or_fetch(
        &self,
        engine_key: &str,
        provider: &dyn ProviderPlugin,
    ) -> Result<Vec<ProviderModel>, ProviderError> {
        if let Some(entry) = self.entries.read().await.get(engine_key) {
            if entry.fetched_at.elapsed() < LIVE_MODELS_TTL {
                return Ok(entry.models.clone());
            }
        }
        let models = provider
            .models()
            .ok_or(ProviderError::UnsupportedCapability("models"))?
            .list_models()
            .await?;
        self.entries.write().await.insert(
            engine_key.to_string(),
            CacheEntry {
                models: models.clone(),
                fetched_at: Instant::now(),
            },
        );
        Ok(models)
    }

    pub async fn invalidate(&self, engine_key: &str) {
        self.entries.write().await.remove(engine_key);
    }
}
