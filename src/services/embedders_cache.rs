// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use anyhow::{Error, anyhow};
use std::collections::HashMap;
use tokio::sync::{OnceCell, RwLock};

use crate::dto::embeddings::{EmbedderModelMeta, EmbeddersResponse};

const EMBEDDERS_BASE_URLS: [&str; 2] = [
    "https://meta.grengin.com/ai-providers",
    "https://meta.grengin.com/providers",
];
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
        match fetch_embedders(req_client, provider).await {
            Ok(envelope) => {
                let models = envelope.models;
                any_ok = true;
                for model in &models {
                    dimensions.insert(format!("{provider}/{}", model.id), model.dimensions);
                }
                by_provider.insert(provider.to_string(), models);
            }
            Err(error) => {
                eprintln!("embedders_cache: failed to load {provider}: {error}");
            }
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

async fn fetch_embedders(
    req_client: &reqwest::Client,
    provider: &str,
) -> Result<EmbeddersResponse, Error> {
    fetch_embedders_from_bases(req_client, provider, &EMBEDDERS_BASE_URLS).await
}

async fn fetch_embedders_from_bases(
    req_client: &reqwest::Client,
    provider: &str,
    base_urls: &[&str],
) -> Result<EmbeddersResponse, Error> {
    let mut last_error = None;
    for base_url in base_urls {
        let url = format!("{base_url}/{provider}/text_embedders.json");
        let result = async {
            let response = req_client.get(&url).send().await?.error_for_status()?;
            Ok::<_, reqwest::Error>(response.json::<EmbeddersResponse>().await?)
        }
        .await;
        match result {
            Ok(envelope) => return Ok(envelope),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error
        .map(Error::from)
        .unwrap_or_else(|| anyhow!("no embedder catalog URLs configured")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, routing::get};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn falls_back_to_legacy_embedder_path_during_catalog_rollout() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind catalog fixture");
        let base_url = format!("http://{}", listener.local_addr().expect("fixture address"));
        let router = Router::new().route(
            "/providers/openai/text_embedders.json",
            get(|| async {
                Json(serde_json::json!({
                    "models": [{"id": "text-embedding-test", "dimensions": 1536}]
                }))
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("serve catalog fixture");
        });

        let primary = format!("{base_url}/ai-providers");
        let fallback = format!("{base_url}/providers");
        let response = fetch_embedders_from_bases(
            &reqwest::Client::new(),
            "openai",
            &[primary.as_str(), fallback.as_str()],
        )
        .await
        .expect("legacy embedder fallback");

        assert_eq!(response.models.len(), 1);
        assert_eq!(response.models[0].id, "text-embedding-test");
    }
}
