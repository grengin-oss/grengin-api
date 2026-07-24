use anyhow::{Error, anyhow};
use serde_json::Value;
use std::collections::HashMap;
use tokio::sync::{OnceCell, RwLock};

use crate::dto::models::{ModelInfo, ModelType, ProviderInfo};

const PROVIDERS_URL: &str = "https://meta.grengin.com/providers.json";
const TITLE_GENERATORS_URL: &str = "https://meta.grengin.com/common/title_generators.json";

#[derive(Clone)]
pub struct ProvidersCache {
    pub providers: Vec<ProviderInfo>,
    pub models_by_key: HashMap<String, ModelInfo>,
    pub title_model_by_engine: HashMap<String, String>,
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

    let cache = build_providers_cache(fetch_all(req_client).await?);
    let mut write_guard = providers_cache().await.write().await;
    if let Some(cached) = write_guard.as_ref() {
        return Ok(cached.clone());
    }
    *write_guard = Some(cache.clone());
    Ok(cache)
}

pub async fn refresh_models_cache(req_client: &reqwest::Client) -> Result<ProvidersCache, Error> {
    let refreshed = build_providers_cache(fetch_all(req_client).await?);
    let mut write_guard = providers_cache().await.write().await;
    *write_guard = Some(refreshed.clone());
    Ok(refreshed)
}

pub async fn load_providers_cached(
    req_client: &reqwest::Client,
) -> Result<Vec<ProviderInfo>, Error> {
    Ok(load_models_cache(req_client).await?.providers)
}

pub async fn get_model_info_cached(
    req_client: &reqwest::Client,
    model_key: &str,
) -> Result<Option<ModelInfo>, Error> {
    let cache = load_models_cache(req_client).await?;
    Ok(cache.models_by_key.get(model_key).cloned())
}

struct FetchedData {
    providers: Vec<ProviderInfo>,
    title_model_by_engine: HashMap<String, String>,
}

fn build_providers_cache(data: FetchedData) -> ProvidersCache {
    let mut models_by_key = HashMap::new();
    for provider in &data.providers {
        for model in &provider.models {
            models_by_key
                .entry(model.key.clone())
                .or_insert_with(|| model.clone());
            if model.key != model.name {
                models_by_key
                    .entry(model.name.clone())
                    .or_insert_with(|| model.clone());
            }
        }
    }
    ProvidersCache {
        providers: data.providers,
        models_by_key,
        title_model_by_engine: data.title_model_by_engine,
    }
}

async fn fetch_all(req_client: &reqwest::Client) -> Result<FetchedData, Error> {
    let (providers, title_model_by_engine) = tokio::join!(
        fetch_providers(req_client),
        fetch_title_models(req_client),
    );
    Ok(FetchedData {
        providers: providers?,
        title_model_by_engine: title_model_by_engine.unwrap_or_default(),
    })
}

async fn fetch_providers(req_client: &reqwest::Client) -> Result<Vec<ProviderInfo>, Error> {
    let providers_value = fetch_json(req_client, PROVIDERS_URL).await?;
    let providers_array = providers_value
        .as_array()
        .ok_or_else(|| anyhow!("providers.json root is not an array"))?;

    let mut providers = Vec::with_capacity(providers_array.len());
    for provider_value in providers_array {
        let (mut provider, text_url, image_url, embed_url) = parse_provider_stub(provider_value)?;

        let mut all_models = Vec::new();
        if let Some(url) = text_url {
            let mut models = fetch_text_models(req_client, &url).await?;
            all_models.append(&mut models);
        }
        if let Some(url) = image_url {
            let mut models = fetch_image_models(req_client, &url).await?;
            all_models.append(&mut models);
        }
        if let Some(url) = embed_url {
            let mut models = fetch_embed_models(req_client, &url, &provider.key).await?;
            all_models.append(&mut models);
        }
        provider.models = all_models;
        providers.push(provider);
    }

    Ok(providers)
}

async fn fetch_title_models(
    req_client: &reqwest::Client,
) -> Result<HashMap<String, String>, Error> {
    let value = fetch_json(req_client, TITLE_GENERATORS_URL).await?;
    let arr = value
        .as_array()
        .ok_or_else(|| anyhow!("title_generators.json root is not an array"))?;

    let mut map = HashMap::new();
    for item in arr {
        if let (Some(engine), Some(key)) = (
            item.get("engine").and_then(Value::as_str),
            item.get("key").and_then(Value::as_str),
        ) {
            map.insert(engine.to_string(), key.to_string());
        }
    }
    Ok(map)
}

async fn fetch_text_models(
    req_client: &reqwest::Client,
    url: &str,
) -> Result<Vec<ModelInfo>, Error> {
    let models_value = fetch_json(req_client, url).await?;
    let arr = models_value
        .as_array()
        .ok_or_else(|| anyhow!("models json root is not an array for {url}"))?;
    arr.iter().map(parse_text_model).collect()
}

async fn fetch_image_models(
    req_client: &reqwest::Client,
    url: &str,
) -> Result<Vec<ModelInfo>, Error> {
    let models_value = fetch_json(req_client, url).await?;
    let arr = models_value
        .as_array()
        .ok_or_else(|| anyhow!("image models json root is not an array for {url}"))?;
    arr.iter().map(parse_image_model).collect()
}

async fn fetch_embed_models(
    req_client: &reqwest::Client,
    url: &str,
    provider_key: &str,
) -> Result<Vec<ModelInfo>, Error> {
    let value = fetch_json(req_client, url).await?;
    let arr = value
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("embed json has no 'models' array for {url}"))?;

    arr.iter()
        .map(|m| parse_embed_model(m, provider_key))
        .collect()
}

async fn fetch_json(req_client: &reqwest::Client, url: &str) -> Result<Value, Error> {
    let response = req_client.get(url).send().await?.error_for_status()?;
    let value = response.json::<Value>().await?;
    Ok(value)
}

fn parse_provider_stub(
    value: &Value,
) -> Result<(ProviderInfo, Option<String>, Option<String>, Option<String>), Error> {
    let key = get_str(value, "key")?;
    let name = get_str(value, "name")?;
    let icon = get_str(value, "icon")?;
    let icon_dark = get_str(value, "iconDark")?;
    let status = get_str(value, "status")?;

    let models_obj = value.get("models").and_then(Value::as_object);
    let get_url = |field: &str| -> Option<String> {
        models_obj
            .and_then(|m| m.get(field))
            .and_then(Value::as_str)
            .map(str::to_string)
    };

    Ok((
        ProviderInfo {
            key,
            name,
            icon,
            icon_dark,
            status,
            models: Vec::new(),
        },
        get_url("text"),
        get_url("image"),
        get_url("embed"),
    ))
}

fn parse_text_model(value: &Value) -> Result<ModelInfo, Error> {
    let key = get_str(value, "key")?;
    let name = get_str(value, "name")?;
    let engine = get_str(value, "engine")?;
    let (input_token_rate, output_token_rate, cached_input_token_rate, cache_creation_token_rate) =
        pricing_rates(value);
    let max_output_tokens = value
        .pointer("/contextWindow/output")
        .and_then(Value::as_i64)
        .map(|v| v as i32);

    Ok(ModelInfo {
        key,
        name,
        engine,
        model_type: ModelType::TextGenerator,
        comment: None,
        input_token_rate,
        output_token_rate,
        image_input_token_rate: None,
        image_output_token_rate: None,
        cached_input_token_rate,
        cache_creation_token_rate,
        max_output_tokens,
        supports_streaming: true,
        supports_tools: true,
        supports_vision: true,
        supports_pdf_native: true,
        supports_web_search: false,
        supports_multiple_images: false,
        max_images: None,
        dimensions: None,
        price_per_image: None,
    })
}

fn parse_image_model(value: &Value) -> Result<ModelInfo, Error> {
    let key = get_str(value, "key")?;
    let name = get_str(value, "name")?;
    let engine = get_str(value, "engine")?;
    let pricing = value.get("pricingPer1M");
    let input_token_rate = pricing.and_then(|p| p.get("input")).and_then(Value::as_f64);
    let output_token_rate = pricing.and_then(|p| p.get("output")).and_then(Value::as_f64);
    let image_input_token_rate = pricing.and_then(|p| p.get("image_input")).and_then(Value::as_f64);
    let image_output_token_rate = pricing.and_then(|p| p.get("image_output")).and_then(Value::as_f64);
    let supports_multiple_images = value.get("supportsMultipleImages").and_then(Value::as_bool).unwrap_or(false);

    Ok(ModelInfo {
        key,
        name,
        engine,
        model_type: ModelType::ImageGenerator,
        comment: None,
        input_token_rate,
        output_token_rate,
        image_input_token_rate,
        image_output_token_rate,
        cached_input_token_rate: None,
        cache_creation_token_rate: None,
        max_output_tokens: None,
        supports_streaming: false,
        supports_tools: false,
        supports_vision: false,
        supports_pdf_native: false,
        supports_web_search: false,
        supports_multiple_images,
        max_images: None,
        dimensions: None,
        price_per_image: None,
    })
}

fn parse_embed_model(value: &Value, provider_key: &str) -> Result<ModelInfo, Error> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("embed model missing 'id'"))?;
    let dimensions = value
        .get("dimensions")
        .and_then(Value::as_i64)
        .map(|d| d as i32);
    let per_1m = value
        .get("pricing")
        .and_then(|p| p.get("per_1M_tokens"))
        .and_then(Value::as_f64);

    Ok(ModelInfo {
        key: id.to_string(),
        name: id.to_string(),
        engine: provider_key.to_string(),
        model_type: ModelType::TextEmbedder,
        comment: None,
        input_token_rate: per_1m,
        output_token_rate: None,
        image_input_token_rate: None,
        image_output_token_rate: None,
        cached_input_token_rate: None,
        cache_creation_token_rate: None,
        max_output_tokens: None,
        supports_streaming: false,
        supports_tools: false,
        supports_vision: false,
        supports_pdf_native: false,
        supports_web_search: false,
        supports_multiple_images: false,
        max_images: None,
        dimensions,
        price_per_image: None,
    })
}

fn pricing_rates(value: &Value) -> (Option<f64>, Option<f64>, Option<f64>, Option<f64>) {
    let pricing = match value.get("pricingPer1M").and_then(Value::as_object) {
        Some(pricing) => pricing,
        None => return (None, None, None, None),
    };

    let input = pricing.get("input").and_then(Value::as_f64);
    let output = pricing.get("output").and_then(Value::as_f64);
    let cached_input = pricing.get("cached_input").and_then(Value::as_f64);
    let cache_creation = pricing.get("cached_input_creation").and_then(Value::as_f64);
    (input, output, cached_input, cache_creation)
}

fn get_str(value: &Value, field: &str) -> Result<String, Error> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("missing or invalid field {field}"))
}
