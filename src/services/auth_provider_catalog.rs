// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::dto::auth_provider_catalog::AuthProviderCatalog;
use anyhow::{Context, Error, anyhow};
use reqwest::Url;
use std::{collections::HashSet, time::Duration};
use tokio::{
    sync::{Mutex, OnceCell},
    time::Instant,
};

const AUTH_PROVIDER_CATALOG_URL: &str = "https://meta.grengin.com/auth-providers/index.json";
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CATALOG_BYTES: usize = 256 * 1024;

#[derive(Clone)]
struct CachedCatalog {
    catalog: AuthProviderCatalog,
    fetched_at: Instant,
}

static AUTH_PROVIDER_CATALOG: OnceCell<Mutex<Option<CachedCatalog>>> = OnceCell::const_new();

async fn cache() -> &'static Mutex<Option<CachedCatalog>> {
    AUTH_PROVIDER_CATALOG
        .get_or_init(|| async { Mutex::new(None) })
        .await
}

pub async fn load_auth_provider_catalog(
    req_client: &reqwest::Client,
) -> Result<AuthProviderCatalog, Error> {
    let mut cached = cache().await.lock().await;
    if let Some(entry) = cached.as_ref()
        && entry.fetched_at.elapsed() < CACHE_TTL
    {
        return Ok(entry.catalog.clone());
    }

    match fetch_auth_provider_catalog(req_client, AUTH_PROVIDER_CATALOG_URL).await {
        Ok(catalog) => {
            *cached = Some(CachedCatalog {
                catalog: catalog.clone(),
                fetched_at: Instant::now(),
            });
            Ok(catalog)
        }
        Err(error) => {
            if let Some(entry) = cached.as_ref() {
                eprintln!("auth provider catalog refresh failed; serving stale data: {error:#}");
                return Ok(entry.catalog.clone());
            }
            Err(error)
        }
    }
}

async fn fetch_auth_provider_catalog(
    req_client: &reqwest::Client,
    url: &str,
) -> Result<AuthProviderCatalog, Error> {
    let response = req_client
        .get(url)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .context("auth provider catalog request failed")?
        .error_for_status()
        .context("auth provider catalog returned an error status")?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_CATALOG_BYTES as u64)
    {
        return Err(anyhow!("auth provider catalog is too large"));
    }
    let bytes = response
        .bytes()
        .await
        .context("auth provider catalog response could not be read")?;
    if bytes.len() > MAX_CATALOG_BYTES {
        return Err(anyhow!("auth provider catalog is too large"));
    }
    let catalog: AuthProviderCatalog =
        serde_json::from_slice(&bytes).context("auth provider catalog response is invalid")?;
    validate_auth_provider_catalog(&catalog)?;
    Ok(catalog)
}

fn validate_auth_provider_catalog(catalog: &AuthProviderCatalog) -> Result<(), Error> {
    if catalog.schema_version != "1.0" || !is_semantic_version(&catalog.catalog_version) {
        return Err(anyhow!("unsupported auth provider catalog version"));
    }
    if catalog.providers.is_empty() || catalog.providers.len() > 64 {
        return Err(anyhow!("auth provider catalog size is invalid"));
    }

    let mut provider_ids = HashSet::new();
    for provider in &catalog.providers {
        if !valid_provider_id(&provider.id) || !provider_ids.insert(provider.id.as_str()) {
            return Err(anyhow!(
                "auth provider catalog contains an invalid provider id"
            ));
        }
        if provider.name.trim().is_empty()
            || provider.name.len() > 80
            || provider.description.trim().is_empty()
            || provider.description.len() > 240
        {
            return Err(anyhow!(
                "auth provider catalog contains invalid display metadata"
            ));
        }
        validate_catalog_url(&provider.template_url, &provider.id, "provider.json")?;
        validate_catalog_url(&provider.icon, &provider.id, "icon.svg")?;
        validate_catalog_url(&provider.icon_dark, &provider.id, "icon-dark.svg")?;
    }
    Ok(())
}

fn validate_catalog_url(value: &str, provider: &str, file: &str) -> Result<(), Error> {
    let url = Url::parse(value).context("auth provider catalog URL is invalid")?;
    let expected_path = format!("/auth-providers/{provider}/{file}");
    if url.scheme() != "https"
        || url.host_str() != Some("meta.grengin.com")
        || url.path() != expected_path
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(anyhow!("auth provider catalog URL is not canonical"));
    }
    Ok(())
}

fn valid_provider_id(value: &str) -> bool {
    value.len() <= 63
        && value
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_lowercase())
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
        && !value.ends_with('-')
}

fn is_semantic_version(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::auth_provider_catalog::{
        AuthProviderProtocol, AuthProviderTemplateStatus, AuthProviderTemplateSummary,
    };

    fn catalog() -> AuthProviderCatalog {
        AuthProviderCatalog {
            schema_version: "1.0".to_string(),
            catalog_version: "1.0.0".to_string(),
            providers: vec![AuthProviderTemplateSummary {
                id: "keycloak".to_string(),
                name: "Keycloak".to_string(),
                description: "OIDC through a Keycloak realm.".to_string(),
                protocol: AuthProviderProtocol::Oidc,
                status: AuthProviderTemplateStatus::Stable,
                template_url: "https://meta.grengin.com/auth-providers/keycloak/provider.json"
                    .to_string(),
                icon: "https://meta.grengin.com/auth-providers/keycloak/icon.svg".to_string(),
                icon_dark: "https://meta.grengin.com/auth-providers/keycloak/icon-dark.svg"
                    .to_string(),
            }],
        }
    }

    #[test]
    fn accepts_valid_catalog() {
        validate_auth_provider_catalog(&catalog()).unwrap();
    }

    #[test]
    fn rejects_duplicate_providers_and_noncanonical_urls() {
        let mut duplicate = catalog();
        duplicate.providers.push(duplicate.providers[0].clone());
        assert!(validate_auth_provider_catalog(&duplicate).is_err());

        let mut external_icon = catalog();
        external_icon.providers[0].icon = "https://example.com/keycloak.svg".to_string();
        assert!(validate_auth_provider_catalog(&external_icon).is_err());
    }

    #[test]
    fn rejects_unknown_or_secret_shaped_catalog_fields() {
        let with_secret = serde_json::json!({
            "schemaVersion": "1.0",
            "catalogVersion": "1.0.0",
            "providers": [{
                "id": "keycloak",
                "name": "Keycloak",
                "description": "OIDC through a Keycloak realm.",
                "protocol": "oidc",
                "status": "stable",
                "templateUrl": "https://meta.grengin.com/auth-providers/keycloak/provider.json",
                "icon": "https://meta.grengin.com/auth-providers/keycloak/icon.svg",
                "iconDark": "https://meta.grengin.com/auth-providers/keycloak/icon-dark.svg",
                "clientSecret": "must-not-be-accepted"
            }]
        });
        assert!(serde_json::from_value::<AuthProviderCatalog>(with_secret).is_err());
    }
}
