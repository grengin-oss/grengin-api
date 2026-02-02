use std::collections::HashMap;

use axum::{extract::State, Json};
use anyhow::{anyhow, Error};
use reqwest::StatusCode;
use serde_json::Value;
use tokio::sync::{OnceCell, RwLock};

use crate::{dto::models::{ModelInfo, ModelsResponse, ProviderInfo}, state::SharedState};

const PROVIDERS_URL: &str = "https://meta.grengin.com/providers.json";

#[derive(Clone)]
pub struct ProvidersCache {
    pub providers: Vec<ProviderInfo>,
    pub models_by_key: HashMap<String, ModelInfo>,
}

static PROVIDERS_CACHE: OnceCell<RwLock<Option<ProvidersCache>>> = OnceCell::const_new();

async fn providers_cache() -> &'static RwLock<Option<ProvidersCache>> {
    PROVIDERS_CACHE
        .get_or_init(|| async { RwLock::new(None) })
        .await
}

pub async fn load_models_cache(req_client: &reqwest::Client) -> Result<ProvidersCache, Error> {
    if let Some(cached) = providers_cache().await.read().await.as_ref() {
        return Ok(cached.clone());
    }

    let providers = fetch_providers(req_client).await?;
    let mut models_by_key = HashMap::new();
    for provider in &providers {
        for model in &provider.models {
            models_by_key.entry(model.key.clone()).or_insert_with(|| model.clone());
            models_by_key
                .entry(model.name.clone())
                .or_insert_with(|| model.clone());
        }
    }
    let cache = ProvidersCache { providers, models_by_key };
    let mut write_guard = providers_cache().await.write().await;
    if let Some(cached) = write_guard.as_ref() {
        return Ok(cached.clone());
    }
    *write_guard = Some(cache.clone());
    Ok(cache)
}

pub async fn load_providers_cached(req_client: &reqwest::Client) -> Result<Vec<ProviderInfo>, Error> {
    Ok(load_models_cache(req_client).await?.providers)
}

pub async fn get_model_info_cached(
    req_client: &reqwest::Client,
    model_key: &str,
) -> Result<Option<ModelInfo>, Error> {
    let cache = load_models_cache(req_client).await?;
    Ok(cache.models_by_key.get(model_key).cloned())
}

async fn fetch_providers(req_client: &reqwest::Client) -> Result<Vec<ProviderInfo>, Error> {
    let providers_value = fetch_json(req_client, PROVIDERS_URL).await?;
    let providers_array = providers_value
        .as_array()
        .ok_or_else(|| anyhow!("providers.json root is not an array"))?;

    let mut providers = Vec::with_capacity(providers_array.len());
    for provider_value in providers_array {
        let (mut provider, text_models_url) = parse_provider_stub(provider_value)?;
        if let Some(text_models_url) = text_models_url {
            provider.models = fetch_models(req_client, &text_models_url).await?;
        }
        providers.push(provider);
    }

    Ok(providers)
}

async fn fetch_models(req_client: &reqwest::Client, url: &str) -> Result<Vec<ModelInfo>, Error> {
    let models_value = fetch_json(req_client, url).await?;
    let models_array = models_value
        .as_array()
        .ok_or_else(|| anyhow!("models json root is not an array for {url}"))?;

    let mut models = Vec::with_capacity(models_array.len());
    for model_value in models_array {
        models.push(parse_model(model_value)?);
    }

    Ok(models)
}

async fn fetch_json(req_client: &reqwest::Client, url: &str) -> Result<Value, Error> {
    let response = req_client.get(url).send().await?.error_for_status()?;
    let value = response.json::<Value>().await?;
    Ok(value)
}

fn parse_provider_stub(value: &Value) -> Result<(ProviderInfo, Option<String>), Error> {
    let key = get_str(value, "key")?;
    let name = get_str(value, "name")?;
    let icon = get_str(value, "icon")?;
    let status = get_str(value, "status")?;

    let text_models_url = value
        .get("models")
        .and_then(Value::as_object)
        .and_then(|models| models.get("text"))
        .and_then(Value::as_str)
        .map(str::to_string);

    Ok((
        ProviderInfo {
            key,
            name,
            icon,
            status,
            models: Vec::new(),
        },
        text_models_url,
    ))
}

fn parse_model(value: &Value) -> Result<ModelInfo, Error> {
    let key = get_str(value, "key")?;
    let name = get_str(value, "name")?;
    let engine = get_str(value, "engine")?;
    let (input_token_rate, output_token_rate) = pricing_rates(value);

    Ok(ModelInfo {
        key,
        name,
        engine,
        comment: None,
        input_token_rate,
        output_token_rate,
        supports_streaming: true,
        supports_tools: true,
        supports_vision: true,
        supports_pdf_native: true,
        supports_web_search: false,
        max_images: None,
    })
}

fn pricing_rates(value: &Value) -> (Option<f64>, Option<f64>) {
    let pricing = match value.get("pricingPer1M").and_then(Value::as_object) {
        Some(pricing) => pricing,
        None => return (None, None),
    };

    let input = pricing.get("input").and_then(Value::as_f64);
    let output = pricing.get("output").and_then(Value::as_f64);
    (input, output)
}

fn get_str(value: &Value, field: &str) -> Result<String, Error> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("missing or invalid field {field}"))
}

#[utoipa::path(
    get,
    path = "/models",
    tag = "models",
    responses(
        (status = 200, body = ModelsResponse, description = "List of providers and models"),
    )
)]
pub async fn get_list_models(
    State(app_state):State<SharedState>,
) -> (StatusCode, Json<ModelsResponse>) {
    let (status, providers) = match load_providers_cached(&app_state.req_client).await {
        Ok(providers) => (StatusCode::OK, providers),
        Err(error) => {
            eprintln!("providers cache error: {error}");
            (StatusCode::SERVICE_UNAVAILABLE, Vec::new())
        }
    };

    let mut filtered_providers = Vec::new();
    for provider in providers {
        let is_enabled = app_state
            .check_ai_engine_is_enabled(&provider.key)
            .await
            .unwrap_or(false);
        if !is_enabled {
            continue;
        }
        let whitelist = app_state
            .settings
            .get_ai_engine_whitelist(&provider.key)
            .await
            .unwrap_or_default();
        if whitelist.is_empty() {
            continue;
        }
        let models = provider
            .models
            .into_iter()
            .filter(|model| whitelist.contains(&model.name) || whitelist.contains(&model.key))
            .collect::<Vec<ModelInfo>>();
        if models.is_empty() {
            continue;
        }
        filtered_providers.push(ProviderInfo {
            key: provider.key,
            name: provider.name,
            icon: provider.icon,
            status: provider.status,
            models,
        });
    }

    (status, Json(ModelsResponse { providers: filtered_providers }))
}
