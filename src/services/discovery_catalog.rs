// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use llm_plugin::ProviderManifestV1;
use openssl::sha::sha256;
use reqwest::{Client, StatusCode, Url, header::IF_NONE_MATCH};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};

use crate::{
    auth::provider_config::OidcProviderConfiguration,
    dto::discovery::{
        AiProviderDiscoveryResponse, AuthProviderDiscoveryResponse, DiscoveryListResponse,
        DiscoveryProviderSummary, DiscoveryVersion,
    },
};

const DEFAULT_METADATA_BASE_URL: &str = "https://meta.grengin.com/";
const CATALOG_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
#[cfg(not(test))]
const CATALOG_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
const CATALOG_REQUEST_TIMEOUT: Duration = Duration::from_millis(200);
const MAX_CATALOG_BYTES: usize = 2 * 1024 * 1024;
const SUPPORTED_DISTRIBUTION_FORMAT_VERSION: &str = "1.0";
#[cfg(test)]
const LEGACY_OIDC_CONFIGURATION_VERSION: &str = "1.0";

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("invalid discovery version")]
    InvalidVersion,
    #[error("discovery resource not found")]
    NotFound,
    #[error("metadata service unavailable: {0}")]
    Unavailable(String),
    #[error("invalid metadata catalog: {0}")]
    InvalidCatalog(String),
}

#[derive(Clone)]
struct CachedDocument {
    body: Arc<Vec<u8>>,
    upstream_etag: Option<String>,
    fetched_at: Instant,
}

#[derive(Clone)]
pub struct DiscoveryCatalog {
    client: Client,
    base_url: Url,
    distribution_version: String,
    documents: Arc<RwLock<HashMap<String, CachedDocument>>>,
    fetch_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApiDistribution {
    format_version: String,
    grengin_api_version: String,
    grengin_api_commit: String,
    catalog_commit: String,
    auth_providers: CatalogIndex,
    ai_providers: CatalogIndex,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogIndex {
    catalog_version: String,
    providers: Vec<CatalogProvider>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogProvider {
    id: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    status: String,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    icon_dark: Option<String>,
    default_version: String,
    versions: Vec<CatalogArtifactVersion>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogArtifactVersion {
    version: String,
    schema_version: String,
    #[serde(default)]
    configuration_version: Option<String>,
    #[serde(default)]
    manifest_version: Option<String>,
    sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReleaseVersion([u64; 3]);

impl Ord for ReleaseVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

impl PartialOrd for ReleaseVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

enum VersionSelector {
    Latest,
    Major(u64),
    Exact(ReleaseVersion),
}

enum CatalogKind {
    Auth,
    Ai,
}

impl CatalogKind {
    fn label(&self) -> &'static str {
        match self {
            Self::Auth => "auth_providers",
            Self::Ai => "ai_providers",
        }
    }

    fn artifact_path(&self, provider: &str, version: &str) -> String {
        match self {
            Self::Auth => {
                format!("auth-providers/{provider}/versions/{version}/provider.json")
            }
            Self::Ai => format!("ai-providers/{provider}/versions/{version}/plugin.json"),
        }
    }

    fn index<'a>(&self, distribution: &'a ApiDistribution) -> &'a CatalogIndex {
        match self {
            Self::Auth => &distribution.auth_providers,
            Self::Ai => &distribution.ai_providers,
        }
    }

    fn contract_version<'a>(&self, version: &'a CatalogArtifactVersion) -> &'a str {
        match self {
            Self::Auth => version.configuration_version.as_deref().unwrap_or_default(),
            Self::Ai => version.manifest_version.as_deref().unwrap_or_default(),
        }
    }
}

impl DiscoveryCatalog {
    pub fn from_env(client: Client) -> Result<Self, DiscoveryError> {
        let base_url = std::env::var("GRENGIN_METADATA_BASE_URL")
            .unwrap_or_else(|_| DEFAULT_METADATA_BASE_URL.to_string());
        let distribution_version = configured_distribution_version();
        Self::new_for_version(client, &base_url, &distribution_version)
    }

    pub fn new(client: Client, base_url: &str) -> Result<Self, DiscoveryError> {
        Self::new_for_version(client, base_url, env!("CARGO_PKG_VERSION"))
    }

    pub fn new_for_version(
        client: Client,
        base_url: &str,
        distribution_version: &str,
    ) -> Result<Self, DiscoveryError> {
        let mut base_url = Url::parse(base_url)
            .map_err(|error| DiscoveryError::InvalidCatalog(error.to_string()))?;
        let host = base_url.host_str().ok_or_else(|| {
            DiscoveryError::InvalidCatalog("metadata base URL has no host".to_string())
        })?;
        let loopback = host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback());
        if base_url.scheme() != "https" && !(base_url.scheme() == "http" && loopback) {
            return Err(DiscoveryError::InvalidCatalog(
                "metadata base URL must use HTTPS or loopback HTTP".to_string(),
            ));
        }
        if !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err(DiscoveryError::InvalidCatalog(
                "metadata base URL contains unsupported components".to_string(),
            ));
        }
        if !base_url.path().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path()));
        }
        validate_distribution_version(distribution_version)?;
        Ok(Self {
            client,
            base_url,
            distribution_version: distribution_version.to_string(),
            documents: Arc::new(RwLock::new(HashMap::new())),
            fetch_locks: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub async fn list_auth(
        &self,
        requested_version: Option<&str>,
    ) -> Result<DiscoveryListResponse, DiscoveryError> {
        self.list(CatalogKind::Auth, requested_version).await
    }

    pub async fn list_ai(
        &self,
        requested_version: Option<&str>,
    ) -> Result<DiscoveryListResponse, DiscoveryError> {
        self.list(CatalogKind::Ai, requested_version).await
    }

    pub async fn auth_provider(
        &self,
        provider: &str,
        requested_version: Option<&str>,
    ) -> Result<AuthProviderDiscoveryResponse, DiscoveryError> {
        let kind = CatalogKind::Auth;
        let (provider, candidates) = self
            .provider_candidates(&kind, provider, requested_version)
            .await?;
        for version in candidates {
            let path = kind.artifact_path(&provider.id, &version.version);
            let bytes = match self.fetch_document(&path).await {
                Ok(bytes) => bytes,
                Err(DiscoveryError::NotFound) => continue,
                Err(error) => return Err(error),
            };
            verify_digest(&bytes, &version.sha256)?;
            let template: Value = serde_json::from_slice(&bytes)
                .map_err(|error| DiscoveryError::InvalidCatalog(error.to_string()))?;
            validate_auth_template(&provider.id, &version, &template)?;
            return Ok(AuthProviderDiscoveryResponse {
                distribution_version: self.distribution_version.clone(),
                id: provider.id,
                version: version.version,
                schema_version: version.schema_version,
                configuration_version: version.configuration_version.unwrap_or_default(),
                sha256: version.sha256,
                template,
            });
        }
        Err(DiscoveryError::NotFound)
    }

    pub async fn ai_provider(
        &self,
        provider: &str,
        requested_version: Option<&str>,
    ) -> Result<AiProviderDiscoveryResponse, DiscoveryError> {
        let kind = CatalogKind::Ai;
        let (provider, candidates) = self
            .provider_candidates(&kind, provider, requested_version)
            .await?;
        for version in candidates {
            let path = kind.artifact_path(&provider.id, &version.version);
            let bytes = match self.fetch_document(&path).await {
                Ok(bytes) => bytes,
                Err(DiscoveryError::NotFound) => continue,
                Err(error) => return Err(error),
            };
            verify_digest(&bytes, &version.sha256)?;
            let manifest = ProviderManifestV1::from_json(&bytes)
                .map_err(|error| DiscoveryError::InvalidCatalog(error.to_string()))?;
            if manifest.id != provider.id || manifest.version != version.version {
                return Err(DiscoveryError::InvalidCatalog(
                    "AI provider identity or version does not match its index".to_string(),
                ));
            }
            let plugin: Value = serde_json::from_slice(&bytes)
                .map_err(|error| DiscoveryError::InvalidCatalog(error.to_string()))?;
            return Ok(AiProviderDiscoveryResponse {
                distribution_version: self.distribution_version.clone(),
                id: provider.id,
                version: version.version,
                manifest_version: version.manifest_version.unwrap_or_default(),
                sha256: version.sha256,
                plugin,
            });
        }
        Err(DiscoveryError::NotFound)
    }

    async fn list(
        &self,
        kind: CatalogKind,
        requested_version: Option<&str>,
    ) -> Result<DiscoveryListResponse, DiscoveryError> {
        let selector = parse_selector(requested_version)?;
        let distribution = self.load_distribution().await?;
        let index = kind.index(&distribution).clone();
        let mut providers = Vec::new();
        for provider in index.providers {
            let compatible = compatible_versions(&provider, &selector)?;
            let Some(selected) = compatible.first() else {
                continue;
            };
            let available_versions = compatible
                .iter()
                .map(|version| DiscoveryVersion {
                    version: version.version.clone(),
                    schema_version: version.schema_version.clone(),
                    contract_version: kind.contract_version(version).to_string(),
                    sha256: version.sha256.clone(),
                })
                .collect::<Vec<_>>();
            providers.push(DiscoveryProviderSummary {
                id: provider.id,
                name: provider.name,
                description: provider.description,
                status: provider.status,
                icon: provider.icon,
                icon_dark: provider.icon_dark,
                selected_version: selected.version.clone(),
                latest_version: available_versions
                    .first()
                    .map(|version| version.version.clone())
                    .unwrap_or_default(),
                available_versions,
            });
        }
        providers.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(DiscoveryListResponse {
            distribution_version: self.distribution_version.clone(),
            catalog_type: kind.label().to_string(),
            catalog_version: index.catalog_version,
            providers,
        })
    }

    async fn provider_candidates(
        &self,
        kind: &CatalogKind,
        provider_id: &str,
        requested_version: Option<&str>,
    ) -> Result<(CatalogProvider, Vec<CatalogArtifactVersion>), DiscoveryError> {
        let provider_id = validate_provider_id(provider_id)?;
        let selector = parse_selector(requested_version)?;
        let distribution = self.load_distribution().await?;
        let index = kind.index(&distribution);
        let provider = index
            .providers
            .iter()
            .find(|provider| provider.id == provider_id)
            .cloned()
            .ok_or(DiscoveryError::NotFound)?;
        let candidates = compatible_versions(&provider, &selector)?;
        if candidates.is_empty() {
            return Err(DiscoveryError::NotFound);
        }
        Ok((provider, candidates))
    }

    async fn load_distribution(&self) -> Result<ApiDistribution, DiscoveryError> {
        let path = format!(
            "distributions/grengin-api/{}/index.json",
            self.distribution_version
        );
        let bytes = self.fetch_document(&path).await?;
        let distribution: ApiDistribution = serde_json::from_slice(&bytes)
            .map_err(|error| DiscoveryError::InvalidCatalog(error.to_string()))?;
        validate_distribution(&distribution, &self.distribution_version)?;
        Ok(distribution)
    }

    async fn fetch_document(&self, path: &str) -> Result<Arc<Vec<u8>>, DiscoveryError> {
        if let Some(cached) = self.documents.read().await.get(path).cloned()
            && cached.fetched_at.elapsed() < CATALOG_CACHE_TTL
        {
            return Ok(cached.body);
        }

        let fetch_lock = {
            let mut locks = self.fetch_locks.lock().await;
            locks
                .entry(path.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = fetch_lock.lock().await;

        if let Some(cached) = self.documents.read().await.get(path).cloned()
            && cached.fetched_at.elapsed() < CATALOG_CACHE_TTL
        {
            return Ok(cached.body);
        }
        let stale = self.documents.read().await.get(path).cloned();
        let url = self
            .base_url
            .join(path)
            .map_err(|error| DiscoveryError::InvalidCatalog(error.to_string()))?;
        let mut request = self.client.get(url).timeout(CATALOG_REQUEST_TIMEOUT);
        if let Some(etag) = stale
            .as_ref()
            .and_then(|document| document.upstream_etag.as_ref())
        {
            request = request.header(IF_NONE_MATCH, etag);
        }
        let mut response = request
            .send()
            .await
            .map_err(|error| DiscoveryError::Unavailable(error.to_string()))?;
        if response.status() == StatusCode::NOT_MODIFIED {
            let mut cached = stale.ok_or_else(|| {
                DiscoveryError::InvalidCatalog("upstream returned 304 without cached data".into())
            })?;
            cached.fetched_at = Instant::now();
            self.documents
                .write()
                .await
                .insert(path.to_string(), cached.clone());
            return Ok(cached.body);
        }
        if response.status() == StatusCode::NOT_FOUND {
            return Err(DiscoveryError::NotFound);
        }
        if !response.status().is_success() {
            return Err(DiscoveryError::Unavailable(format!(
                "metadata endpoint returned {}",
                response.status()
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_CATALOG_BYTES as u64)
        {
            return Err(DiscoveryError::InvalidCatalog(
                "metadata document exceeds size limit".to_string(),
            ));
        }
        let upstream_etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| DiscoveryError::Unavailable(error.to_string()))?
        {
            if body.len().saturating_add(chunk.len()) > MAX_CATALOG_BYTES {
                return Err(DiscoveryError::InvalidCatalog(
                    "metadata document exceeds size limit".to_string(),
                ));
            }
            body.extend_from_slice(&chunk);
        }
        let document = CachedDocument {
            body: Arc::new(body),
            upstream_etag,
            fetched_at: Instant::now(),
        };
        self.documents
            .write()
            .await
            .insert(path.to_string(), document.clone());
        Ok(document.body)
    }
}

fn configured_distribution_version() -> String {
    #[cfg(debug_assertions)]
    if let Ok(version) = std::env::var("GRENGIN_PROVIDER_DISTRIBUTION_VERSION") {
        return version;
    }
    env!("CARGO_PKG_VERSION").to_string()
}

fn validate_distribution_version(value: &str) -> Result<(), DiscoveryError> {
    if value.split('.').count() != 3 || parse_release_version(value).is_err() {
        return Err(DiscoveryError::InvalidCatalog(
            "provider distribution version must be MAJOR.MINOR.PATCH".to_string(),
        ));
    }
    Ok(())
}

fn is_full_git_commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_distribution(
    distribution: &ApiDistribution,
    expected_version: &str,
) -> Result<(), DiscoveryError> {
    if distribution.format_version != SUPPORTED_DISTRIBUTION_FORMAT_VERSION {
        return Err(DiscoveryError::InvalidCatalog(format!(
            "unsupported provider distribution format {}",
            distribution.format_version
        )));
    }
    if distribution.grengin_api_version != expected_version {
        return Err(DiscoveryError::InvalidCatalog(
            "provider distribution does not match this grengin-api version".to_string(),
        ));
    }
    if !is_full_git_commit(&distribution.grengin_api_commit)
        || !is_full_git_commit(&distribution.catalog_commit)
    {
        return Err(DiscoveryError::InvalidCatalog(
            "provider distribution contains invalid source commits".to_string(),
        ));
    }
    validate_catalog_index(&CatalogKind::Auth, &distribution.auth_providers)?;
    validate_catalog_index(&CatalogKind::Ai, &distribution.ai_providers)?;
    Ok(())
}

fn validate_catalog_index(kind: &CatalogKind, index: &CatalogIndex) -> Result<(), DiscoveryError> {
    if validate_distribution_version(&index.catalog_version).is_err() || index.providers.is_empty()
    {
        return Err(DiscoveryError::InvalidCatalog(
            "distribution contains an invalid or empty provider catalog".to_string(),
        ));
    }
    let mut provider_ids = HashSet::new();
    for provider in &index.providers {
        if validate_provider_id(&provider.id).is_err() || !provider_ids.insert(provider.id.clone())
        {
            return Err(DiscoveryError::InvalidCatalog(
                "distribution contains an invalid or duplicate provider ID".to_string(),
            ));
        }
        if provider.name.trim().is_empty()
            || provider.status.trim().is_empty()
            || provider.versions.is_empty()
        {
            return Err(DiscoveryError::InvalidCatalog(
                "distribution contains incomplete provider metadata".to_string(),
            ));
        }

        let mut versions = HashSet::new();
        for version in &provider.versions {
            parse_release_version(&version.version).map_err(|_| {
                DiscoveryError::InvalidCatalog(
                    "distribution contains an invalid package version".to_string(),
                )
            })?;
            parse_release_version(&version.schema_version).map_err(|_| {
                DiscoveryError::InvalidCatalog(
                    "distribution contains an invalid package schema version".to_string(),
                )
            })?;
            if !versions.insert(version.version.clone()) {
                return Err(DiscoveryError::InvalidCatalog(
                    "distribution contains a duplicate package version".to_string(),
                ));
            }
            if !is_sha256(&version.sha256) {
                return Err(DiscoveryError::InvalidCatalog(
                    "distribution contains an invalid SHA-256 digest".to_string(),
                ));
            }
            match kind {
                CatalogKind::Auth
                    if version.configuration_version.is_none()
                        || version.manifest_version.is_some() =>
                {
                    return Err(DiscoveryError::InvalidCatalog(
                        "auth package is missing its configuration contract".to_string(),
                    ));
                }
                CatalogKind::Ai
                    if version.manifest_version.is_none()
                        || version.configuration_version.is_some() =>
                {
                    return Err(DiscoveryError::InvalidCatalog(
                        "AI package is missing its manifest contract".to_string(),
                    ));
                }
                _ => {}
            }
            let contract_version = kind.contract_version(version);
            parse_release_version(contract_version).map_err(|_| {
                DiscoveryError::InvalidCatalog(
                    "distribution contains an invalid package contract version".to_string(),
                )
            })?;
        }
        if !versions.contains(&provider.default_version) {
            return Err(DiscoveryError::InvalidCatalog(
                "provider default version is not present in its package list".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_provider_id(value: &str) -> Result<String, DiscoveryError> {
    let valid = !value.is_empty()
        && value.len() <= 63
        && value
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_lowercase())
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
        && !value.ends_with('-');
    valid
        .then(|| value.to_string())
        .ok_or(DiscoveryError::NotFound)
}

fn parse_release_version(value: &str) -> Result<ReleaseVersion, DiscoveryError> {
    let parts = value.split('.').collect::<Vec<_>>();
    if !(2..=3).contains(&parts.len()) {
        return Err(DiscoveryError::InvalidVersion);
    }
    let mut parsed = [0_u64; 3];
    for (index, part) in parts.into_iter().enumerate() {
        if part.is_empty() || (part.len() > 1 && part.starts_with('0')) {
            return Err(DiscoveryError::InvalidVersion);
        }
        parsed[index] = part
            .parse::<u64>()
            .map_err(|_| DiscoveryError::InvalidVersion)?;
    }
    Ok(ReleaseVersion(parsed))
}

fn parse_selector(value: Option<&str>) -> Result<VersionSelector, DiscoveryError> {
    let Some(value) = value else {
        return Ok(VersionSelector::Latest);
    };
    let value = value.trim();
    if !value.contains('.') {
        return value
            .parse::<u64>()
            .map(VersionSelector::Major)
            .map_err(|_| DiscoveryError::InvalidVersion);
    }
    parse_release_version(value).map(VersionSelector::Exact)
}

fn compatible_versions(
    provider: &CatalogProvider,
    selector: &VersionSelector,
) -> Result<Vec<CatalogArtifactVersion>, DiscoveryError> {
    let default_version = match selector {
        VersionSelector::Latest => Some(parse_release_version(&provider.default_version)?),
        _ => None,
    };
    let mut versions = provider
        .versions
        .iter()
        .map(|version| Ok((parse_release_version(&version.version)?, version.clone())))
        .collect::<Result<Vec<_>, DiscoveryError>>()?;
    versions.retain(|(version, _)| match selector {
        VersionSelector::Latest => default_version
            .as_ref()
            .is_some_and(|default| version.0[0] == default.0[0] && version <= default),
        VersionSelector::Major(major) => version.0[0] == *major,
        VersionSelector::Exact(expected) => version == expected,
    });
    versions.sort_by(|left, right| right.0.cmp(&left.0));
    Ok(versions.into_iter().map(|(_, version)| version).collect())
}

fn verify_digest(bytes: &[u8], expected: &str) -> Result<(), DiscoveryError> {
    if !is_sha256(expected) {
        return Err(DiscoveryError::InvalidCatalog(
            "catalog contains an invalid SHA-256 digest".to_string(),
        ));
    }
    let actual = sha256_hex(bytes);
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(DiscoveryError::InvalidCatalog(
            "metadata artifact failed SHA-256 verification".to_string(),
        ));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    sha256(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn validate_auth_template(
    expected_provider: &str,
    expected_version: &CatalogArtifactVersion,
    template: &Value,
) -> Result<(), DiscoveryError> {
    let id = template.get("id").and_then(Value::as_str);
    let version = template.get("version").and_then(Value::as_str);
    let schema_version = template.get("schemaVersion").and_then(Value::as_str);
    if id != Some(expected_provider)
        || version != Some(expected_version.version.as_str())
        || schema_version != Some(expected_version.schema_version.as_str())
    {
        return Err(DiscoveryError::InvalidCatalog(
            "auth provider identity or version does not match its index".to_string(),
        ));
    }
    if let Some(config_version) = template
        .get("configuration")
        .and_then(|configuration| configuration.get("version"))
        .and_then(Value::as_str)
        && Some(config_version) != expected_version.configuration_version.as_deref()
    {
        return Err(DiscoveryError::InvalidCatalog(
            "auth configuration version does not match its index".to_string(),
        ));
    }
    OidcProviderConfiguration::from_value_for_provider(
        template.get("configuration"),
        expected_provider,
    )
    .map_err(|error| DiscoveryError::InvalidCatalog(error.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        extract::State,
        http::{HeaderMap, StatusCode, header},
        response::{IntoResponse, Response},
        routing::get,
    };
    use std::{
        fs,
        path::Path,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use tokio::{net::TcpListener, time::sleep};

    const TEST_DISTRIBUTION_PATH: &str = concat!(
        "/distributions/grengin-api/",
        env!("CARGO_PKG_VERSION"),
        "/index.json"
    );

    async fn serve(router: Router) -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind catalog server");
        let base_url = format!(
            "http://{}/",
            listener.local_addr().expect("catalog address")
        );
        tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("serve catalog fixture");
        });
        base_url
    }

    fn test_client() -> Client {
        Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("test HTTP client")
    }

    fn auth_template(id: &str, version: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion": "1.0",
            "id": id,
            "version": version,
            "configuration": {
                "version": "1.0",
                "scopes": ["openid", "email", "profile"],
                "authorizationParams": {},
                "pkce": "s256",
                "emailLinking": "verifiedEmail",
                "autoRedirect": false
            }
        }))
        .expect("auth template")
    }

    fn auth_distribution(id: &str, default_version: &str, versions: Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "formatVersion": "1.0",
            "grenginApiVersion": env!("CARGO_PKG_VERSION"),
            "grenginApiCommit": "a".repeat(40),
            "catalogCommit": "b".repeat(40),
            "authProviders": {
                "catalogVersion": "1.0.0",
                "providers": [{
                    "id": id,
                    "name": "Test provider",
                    "status": "stable",
                    "defaultVersion": default_version,
                    "versions": versions
                }]
            },
            "aiProviders": {
                "catalogVersion": "1.0.0",
                "providers": [{
                    "id": "test-ai",
                    "name": "Test AI",
                    "status": "stable",
                    "defaultVersion": "1.0",
                    "versions": [{
                        "version": "1.0",
                        "schemaVersion": "1.0",
                        "manifestVersion": "1.0",
                        "sha256": "0".repeat(64)
                    }]
                }]
            }
        }))
        .expect("provider distribution")
    }

    fn version(
        release: &str,
        schema: &str,
        configuration: Option<&str>,
        manifest: Option<&str>,
    ) -> CatalogArtifactVersion {
        CatalogArtifactVersion {
            version: release.to_string(),
            schema_version: schema.to_string(),
            configuration_version: configuration.map(str::to_string),
            manifest_version: manifest.map(str::to_string),
            sha256: "0".repeat(64),
        }
    }

    fn provider(default_version: &str, versions: Vec<CatalogArtifactVersion>) -> CatalogProvider {
        CatalogProvider {
            id: "example".to_string(),
            name: "Example".to_string(),
            description: None,
            status: "stable".to_string(),
            icon: None,
            icon_dark: None,
            default_version: default_version.to_string(),
            versions,
        }
    }

    #[test]
    fn major_selector_returns_newest_compatible_release_without_crossing_major() {
        let provider = provider(
            "1.2.0",
            vec![
                version("1.0.0", "1.0", None, Some("1.0")),
                version("1.2.0", "1.0", None, Some("1.0")),
                version("2.0.0", "1.0", None, Some("2.0")),
            ],
        );
        let selected = compatible_versions(&provider, &VersionSelector::Major(1))
            .expect("compatible versions");
        assert_eq!(selected[0].version, "1.2.0");
        assert_eq!(selected[1].version, "1.0.0");
    }

    #[test]
    fn distribution_membership_replaces_hardcoded_contract_filtering() {
        let provider = provider(
            "1.2.0",
            vec![
                version("1.0.0", "1.0", Some("1.0"), None),
                version("1.1.0", "1.0", Some("1.1"), None),
                version("1.2.0", "1.0", Some("1.2"), None),
                version("2.0.0", "2.0", Some("1.1"), None),
            ],
        );
        let selected = compatible_versions(&provider, &VersionSelector::Major(1))
            .expect("distribution versions");
        assert_eq!(
            selected
                .iter()
                .map(|version| version.version.as_str())
                .collect::<Vec<_>>(),
            vec!["1.2.0", "1.1.0", "1.0.0"]
        );
    }

    #[test]
    fn default_selector_never_moves_past_the_distribution_pin() {
        let provider = provider(
            "1.1.0",
            vec![
                version("1.0.0", "1.0", None, Some("1.0")),
                version("1.1.0", "1.0", None, Some("1.0")),
                version("1.2.0", "1.0", None, Some("1.0")),
            ],
        );
        let selected =
            compatible_versions(&provider, &VersionSelector::Latest).expect("pinned versions");
        assert_eq!(
            selected
                .iter()
                .map(|version| version.version.as_str())
                .collect::<Vec<_>>(),
            vec!["1.1.0", "1.0.0"]
        );
    }

    #[test]
    fn exact_version_does_not_fall_across_releases() {
        let provider = provider(
            "1.1.0",
            vec![
                version("1.0.0", "1.0", None, Some("1.0")),
                version("1.1.0", "1.0", None, Some("1.0")),
            ],
        );
        let selected = compatible_versions(
            &provider,
            &VersionSelector::Exact(ReleaseVersion([1, 0, 0])),
        )
        .expect("compatible versions");
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].version, "1.0.0");
    }

    #[test]
    fn latest_selector_fallback_stays_within_latest_major() {
        let provider = provider(
            "2.1.0",
            vec![
                version("1.9.0", "1.0", None, Some("1.0")),
                version("2.0.0", "1.0", None, Some("1.0")),
                version("2.1.0", "1.0", None, Some("1.0")),
            ],
        );
        let selected =
            compatible_versions(&provider, &VersionSelector::Latest).expect("compatible versions");
        assert_eq!(
            selected
                .iter()
                .map(|version| version.version.as_str())
                .collect::<Vec<_>>(),
            vec!["2.1.0", "2.0.0"]
        );
    }

    #[test]
    fn digest_mismatch_fails_closed() {
        assert!(verify_digest(b"artifact", &"0".repeat(64)).is_err());
        assert!(verify_digest(b"artifact", &sha256_hex(b"artifact")).is_ok());
    }

    #[test]
    fn distribution_metadata_must_match_the_backend_release() {
        let versions = serde_json::json!([{
            "version": "1.0.0",
            "schemaVersion": "1.0",
            "configurationVersion": "1.0",
            "sha256": "0".repeat(64)
        }]);
        let valid: ApiDistribution =
            serde_json::from_slice(&auth_distribution("example", "1.0.0", versions))
                .expect("distribution fixture");

        let mut wrong_format = valid.clone();
        wrong_format.format_version = "2.0".to_string();
        assert!(matches!(
            validate_distribution(&wrong_format, env!("CARGO_PKG_VERSION")),
            Err(DiscoveryError::InvalidCatalog(_))
        ));

        let mut wrong_release = valid.clone();
        wrong_release.grengin_api_version = "9.9.9".to_string();
        assert!(matches!(
            validate_distribution(&wrong_release, env!("CARGO_PKG_VERSION")),
            Err(DiscoveryError::InvalidCatalog(_))
        ));

        let mut abbreviated_commit = valid;
        abbreviated_commit.catalog_commit = "deadbeef".to_string();
        assert!(matches!(
            validate_distribution(&abbreviated_commit, env!("CARGO_PKG_VERSION")),
            Err(DiscoveryError::InvalidCatalog(_))
        ));
    }

    #[test]
    fn malformed_package_lists_fail_closed() {
        let versions = serde_json::json!([{
            "version": "1.0.0",
            "schemaVersion": "1.0",
            "configurationVersion": "1.0",
            "sha256": "0".repeat(64)
        }]);
        let valid: ApiDistribution =
            serde_json::from_slice(&auth_distribution("example", "1.0.0", versions))
                .expect("distribution fixture");

        let mut missing_default = valid.clone();
        missing_default.auth_providers.providers[0].default_version = "1.1.0".to_string();
        assert!(matches!(
            validate_distribution(&missing_default, env!("CARGO_PKG_VERSION")),
            Err(DiscoveryError::InvalidCatalog(_))
        ));

        let mut duplicate_version = valid.clone();
        let package = duplicate_version.auth_providers.providers[0].versions[0].clone();
        duplicate_version.auth_providers.providers[0]
            .versions
            .push(package);
        assert!(matches!(
            validate_distribution(&duplicate_version, env!("CARGO_PKG_VERSION")),
            Err(DiscoveryError::InvalidCatalog(_))
        ));

        let mut malformed_digest = valid;
        malformed_digest.auth_providers.providers[0].versions[0].sha256 = "ABC".repeat(21);
        assert!(matches!(
            validate_distribution(&malformed_digest, env!("CARGO_PKG_VERSION")),
            Err(DiscoveryError::InvalidCatalog(_))
        ));
    }

    #[test]
    fn provider_ids_cannot_escape_catalog_paths() {
        for value in ["../apple", "Apple", "a/b", "-apple", "apple-"] {
            assert!(validate_provider_id(value).is_err(), "accepted {value}");
        }
        assert_eq!(
            validate_provider_id("azure-openai").unwrap(),
            "azure-openai"
        );
    }

    #[tokio::test]
    async fn cached_documents_revalidate_with_etag_and_accept_304() {
        #[derive(Clone)]
        struct Fixture {
            requests: Arc<AtomicUsize>,
        }
        async fn document(State(state): State<Fixture>, headers: HeaderMap) -> Response {
            state.requests.fetch_add(1, Ordering::SeqCst);
            if headers
                .get(header::IF_NONE_MATCH)
                .is_some_and(|value| value == "\"catalog-v1\"")
            {
                return StatusCode::NOT_MODIFIED.into_response();
            }
            ([(header::ETAG, "\"catalog-v1\"")], "{\"value\":true}").into_response()
        }

        let requests = Arc::new(AtomicUsize::new(0));
        let base_url = serve(
            Router::new()
                .route("/document.json", get(document))
                .with_state(Fixture {
                    requests: requests.clone(),
                }),
        )
        .await;
        let catalog = DiscoveryCatalog::new(test_client(), &base_url).expect("catalog");
        let first = catalog
            .fetch_document("document.json")
            .await
            .expect("initial document");
        {
            let mut documents = catalog.documents.write().await;
            documents.get_mut("document.json").unwrap().fetched_at =
                Instant::now() - CATALOG_CACHE_TTL;
        }
        let second = catalog
            .fetch_document("document.json")
            .await
            .expect("revalidated document");

        assert_eq!(first.as_slice(), second.as_slice());
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn concurrent_fetches_are_single_flight_per_document() {
        async fn document(State(requests): State<Arc<AtomicUsize>>) -> &'static str {
            requests.fetch_add(1, Ordering::SeqCst);
            sleep(Duration::from_millis(50)).await;
            "{\"value\":true}"
        }

        let requests = Arc::new(AtomicUsize::new(0));
        let base_url = serve(
            Router::new()
                .route("/document.json", get(document))
                .with_state(requests.clone()),
        )
        .await;
        let catalog = DiscoveryCatalog::new(test_client(), &base_url).expect("catalog");
        let (first, second) = tokio::join!(
            catalog.fetch_document("document.json"),
            catalog.fetch_document("document.json")
        );

        assert!(first.is_ok());
        assert!(second.is_ok());
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn missing_latest_artifact_falls_back_within_requested_major() {
        let fallback = auth_template("example", "1.0.0");
        let index = auth_distribution(
            "example",
            "1.1.0",
            serde_json::json!([
                {
                    "version": "1.1.0",
                    "schemaVersion": "1.0",
                    "configurationVersion": "1.0",
                    "sha256": "0".repeat(64)
                },
                {
                    "version": "1.0.0",
                    "schemaVersion": "1.0",
                    "configurationVersion": "1.0",
                    "sha256": sha256_hex(&fallback)
                }
            ]),
        );
        let base_url = serve(
            Router::new()
                .route(
                    TEST_DISTRIBUTION_PATH,
                    get({
                        let index = index.clone();
                        move || {
                            let index = index.clone();
                            async move { index }
                        }
                    }),
                )
                .route(
                    "/auth-providers/example/versions/1.0.0/provider.json",
                    get(move || {
                        let fallback = fallback.clone();
                        async move { fallback }
                    }),
                ),
        )
        .await;
        let catalog = DiscoveryCatalog::new(test_client(), &base_url).expect("catalog");

        let resolved = catalog
            .auth_provider("example", Some("1"))
            .await
            .expect("same-major fallback");
        assert_eq!(resolved.distribution_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(resolved.version, "1.0.0");
    }

    #[tokio::test]
    async fn missing_distribution_never_falls_back_to_another_backend_release() {
        let old_path = "/distributions/grengin-api/9.9.8/index.json";
        let base_url = serve(Router::new().route(old_path, get(|| async { "{}" }))).await;
        let catalog =
            DiscoveryCatalog::new_for_version(test_client(), &base_url, "9.9.9").expect("catalog");

        assert!(matches!(
            catalog.list_auth(None).await,
            Err(DiscoveryError::NotFound)
        ));
    }

    #[tokio::test]
    async fn digest_valid_but_unparseable_ai_package_fails_closed() {
        let invalid_plugin = b"{}".to_vec();
        let versions = serde_json::json!([{
            "version": "1.0.0",
            "schemaVersion": "1.0",
            "configurationVersion": "1.0",
            "sha256": "0".repeat(64)
        }]);
        let mut distribution: Value =
            serde_json::from_slice(&auth_distribution("example", "1.0.0", versions))
                .expect("distribution fixture");
        distribution["aiProviders"]["providers"][0]["versions"][0]["sha256"] =
            Value::String(sha256_hex(&invalid_plugin));
        let distribution = serde_json::to_vec(&distribution).expect("serialize distribution");

        let base_url = serve(
            Router::new()
                .route(
                    TEST_DISTRIBUTION_PATH,
                    get(move || {
                        let distribution = distribution.clone();
                        async move { distribution }
                    }),
                )
                .route(
                    "/ai-providers/test-ai/versions/1.0/plugin.json",
                    get(move || {
                        let invalid_plugin = invalid_plugin.clone();
                        async move { invalid_plugin }
                    }),
                ),
        )
        .await;
        let catalog = DiscoveryCatalog::new(test_client(), &base_url).expect("catalog");

        assert!(matches!(
            catalog.ai_provider("test-ai", None).await,
            Err(DiscoveryError::InvalidCatalog(_))
        ));
    }

    #[tokio::test]
    async fn duplicate_provider_ids_and_upstream_failures_fail_closed() {
        let versions = serde_json::json!([{
            "version": "1.0.0",
            "schemaVersion": "1.0",
            "configurationVersion": "1.0",
            "sha256": "0".repeat(64)
        }]);
        let mut duplicate_index: Value =
            serde_json::from_slice(&auth_distribution("duplicate", "1.0.0", versions)).unwrap();
        let provider = duplicate_index["authProviders"]["providers"][0].clone();
        duplicate_index["authProviders"]["providers"] =
            serde_json::json!([provider.clone(), provider]);
        let duplicate_index = serde_json::to_vec(&duplicate_index).unwrap();
        let duplicate_base = serve(Router::new().route(
            TEST_DISTRIBUTION_PATH,
            get(move || {
                let duplicate_index = duplicate_index.clone();
                async move { duplicate_index }
            }),
        ))
        .await;
        let duplicate_catalog =
            DiscoveryCatalog::new(test_client(), &duplicate_base).expect("catalog");
        assert!(matches!(
            duplicate_catalog.list_auth(None).await,
            Err(DiscoveryError::InvalidCatalog(_))
        ));

        let failure_base = serve(Router::new().route(
            TEST_DISTRIBUTION_PATH,
            get(|| async { StatusCode::BAD_GATEWAY }),
        ))
        .await;
        let failure_catalog = DiscoveryCatalog::new(test_client(), &failure_base).expect("catalog");
        assert!(matches!(
            failure_catalog.list_auth(None).await,
            Err(DiscoveryError::Unavailable(_))
        ));
    }

    #[tokio::test]
    async fn stalled_upstream_is_bounded_by_request_timeout() {
        let base_url = serve(Router::new().route(
            TEST_DISTRIBUTION_PATH,
            get(|| async {
                sleep(Duration::from_secs(1)).await;
                "{}"
            }),
        ))
        .await;
        let catalog = DiscoveryCatalog::new(test_client(), &base_url).expect("catalog");
        let started = Instant::now();
        let result = catalog.list_auth(None).await;

        assert!(matches!(result, Err(DiscoveryError::Unavailable(_))));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    #[ignore = "cross-repository contract test; run from grengin-list CI"]
    fn every_catalog_auth_template_matches_the_runtime_contract() {
        let root = std::env::var("GRENGIN_AUTH_PROVIDER_CATALOG_DIR")
            .expect("GRENGIN_AUTH_PROVIDER_CATALOG_DIR must point to auth-providers");
        let mut checked = 0;
        for entry in fs::read_dir(&root).expect("read auth provider catalog") {
            let path = entry.expect("catalog entry").path().join("provider.json");
            if !path.is_file() {
                continue;
            }
            let bytes = fs::read(&path).expect("read provider template");
            let template: Value = serde_json::from_slice(&bytes).expect("parse provider template");
            let provider_id = template["id"].as_str().expect("template id");
            let configuration_version = template
                .get("configuration")
                .and_then(|configuration| configuration.get("version"))
                .and_then(Value::as_str)
                .unwrap_or(LEGACY_OIDC_CONFIGURATION_VERSION);
            let indexed = CatalogArtifactVersion {
                version: template["version"]
                    .as_str()
                    .expect("template version")
                    .to_string(),
                schema_version: template["schemaVersion"]
                    .as_str()
                    .expect("template schema version")
                    .to_string(),
                configuration_version: Some(configuration_version.to_string()),
                manifest_version: None,
                sha256: sha256_hex(&bytes),
            };
            validate_auth_template(provider_id, &indexed, &template)
                .unwrap_or_else(|error| panic!("{}: {error}", Path::new(&path).display()));
            checked += 1;
        }
        assert!(checked > 0, "no auth templates were checked");
    }

    #[test]
    #[ignore = "cross-repository contract test; run from grengin-list CI"]
    fn every_distribution_package_matches_the_runtime_contract() {
        let root = std::env::var("GRENGIN_PROVIDER_CATALOG_ROOT")
            .expect("GRENGIN_PROVIDER_CATALOG_ROOT must point to master-data");
        let path = Path::new(&root)
            .join("distributions/grengin-api")
            .join(env!("CARGO_PKG_VERSION"))
            .join("index.json");
        let mut checked_packages = 0;

        let bytes = fs::read(&path).expect("read matching grengin-api distribution");
        let distribution: ApiDistribution =
            serde_json::from_slice(&bytes).expect("parse distribution");
        validate_distribution(&distribution, env!("CARGO_PKG_VERSION"))
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));

        for provider in &distribution.auth_providers.providers {
            for version in &provider.versions {
                let artifact = Path::new(&root)
                    .join(CatalogKind::Auth.artifact_path(&provider.id, &version.version));
                let bytes = fs::read(&artifact).expect("read auth package");
                verify_digest(&bytes, &version.sha256)
                    .unwrap_or_else(|error| panic!("{}: {error}", artifact.display()));
                let template: Value = serde_json::from_slice(&bytes).expect("parse auth package");
                validate_auth_template(&provider.id, version, &template)
                    .unwrap_or_else(|error| panic!("{}: {error}", artifact.display()));
                checked_packages += 1;
            }
        }

        for provider in &distribution.ai_providers.providers {
            for version in &provider.versions {
                let artifact = Path::new(&root)
                    .join(CatalogKind::Ai.artifact_path(&provider.id, &version.version));
                let bytes = fs::read(&artifact).expect("read AI package");
                verify_digest(&bytes, &version.sha256)
                    .unwrap_or_else(|error| panic!("{}: {error}", artifact.display()));
                let manifest = ProviderManifestV1::from_json(&bytes)
                    .unwrap_or_else(|error| panic!("{}: {error}", artifact.display()));
                assert_eq!(manifest.id, provider.id, "{}", artifact.display());
                assert_eq!(manifest.version, version.version, "{}", artifact.display());
                checked_packages += 1;
            }
        }

        assert!(
            checked_packages > 0,
            "no distribution packages were checked"
        );
    }
}
