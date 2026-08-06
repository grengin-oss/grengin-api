// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use anyhow::{Error, anyhow};
use std::collections::HashMap;
use tokio::sync::{OnceCell, RwLock};

use crate::dto::embeddings::{EmbedderModelMeta, EmbeddersResponse};

const EMBEDDERS_BASE_URL: &str = "https://meta.grengin.com/providers";
const EMBEDDING_PROVIDERS: [&str; 3] = ["openai", "mistral", "gemini"];

#[derive(Clone)]
pub struct EmbeddersCache {
    pub by_provider: HashMap<String, Vec<EmbedderModelMeta>>,
    // "{provider}/{model_id}" -> native dimensions
    pub dimensions: HashMap<String, usize>,
}

static EMBEDDERS_CACHE: OnceCell<RwLock<Option<EmbeddersCache>>> = OnceCell::const_new();

async fn embedders_cache() -> &'static RwLock<Option<EmbeddersCache>> {
    EMBEDDERS_CACHE
        .get_or_init(|| async { RwLock::new(None) })
        .await
}

pub async fn load_embedders_cache(req_client: &reqwest::Client) -> Result<EmbeddersCache, Error> {
    if let Some(cached) = embedders_cache().await.read().await.as_ref() {
        return Ok(cached.clone());
    }

    let cache = build_embedders_cache(req_client).await?;
    let mut write_guard = embedders_cache().await.write().await;
    if let Some(cached) = write_guard.as_ref() {
        return Ok(cached.clone());
    }
    *write_guard = Some(cache.clone());
    Ok(cache)
}

pub async fn get_model_dimensions(
    req_client: &reqwest::Client,
    provider: &str,
    model: &str,
) -> Option<usize> {
    load_embedders_cache(req_client)
        .await
        .ok()?
        .dimensions
        .get(&format!("{provider}/{model}"))
        .copied()
}

async fn build_embedders_cache(req_client: &reqwest::Client) -> Result<EmbeddersCache, Error> {
    let mut by_provider: HashMap<String, Vec<EmbedderModelMeta>> = HashMap::new();
    let mut dimensions: HashMap<String, usize> = HashMap::new();
    let mut any_ok = false;

    for provider in EMBEDDING_PROVIDERS {
        let url = format!("{EMBEDDERS_BASE_URL}/{provider}/text_embedders.json");
        let result = req_client
            .get(&url)
            .send()
            .await
            .and_then(|r| r.error_for_status());
        match result {
            Err(e) => {
                eprintln!("embedders_cache: failed to fetch {provider}: {e}");
                continue;
            }
            Ok(resp) => match resp.json::<EmbeddersResponse>().await {
                Err(e) => {
                    eprintln!("embedders_cache: failed to parse {provider}: {e}");
                    continue;
                }
                Ok(envelope) => {
                    let models = envelope.models;
                    any_ok = true;
                    for model in &models {
                        dimensions.insert(format!("{provider}/{}", model.id), model.dimensions);
                    }
                    by_provider.insert(provider.to_string(), models);
                }
            },
        }
    }

    if !any_ok {
        return Err(anyhow!("embedders_cache: all providers failed to load"));
    }

    Ok(EmbeddersCache {
        by_provider,
        dimensions,
    })
}
