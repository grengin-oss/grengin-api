// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use llm_plugin::{ProviderManifestV1, SUPPORTED_MANIFEST_VERSION};
use openssl::sha::sha256;
use reqwest::{Client, StatusCode, Url, header::IF_NONE_MATCH};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};

use crate::{
    auth::provider_config::{OIDC_PROVIDER_CONFIG_VERSION, OidcProviderConfiguration},
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
const SUPPORTED_CATALOG_SCHEMA_VERSION: &str = "1.0";
const SUPPORTED_AUTH_TEMPLATE_SCHEMA_VERSION: &str = "1.0";
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
    documents: Arc<RwLock<HashMap<String, CachedDocument>>>,
    fetch_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogIndex {
    schema_version: String,
    catalog_version: String,
    providers: Vec<CatalogProvider>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
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
    #[serde(default)]
    versions: Vec<CatalogArtifactVersion>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
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

    fn index_path(&self) -> &'static str {
        match self {
            Self::Auth => "auth-providers/index.json",
            Self::Ai => "ai-providers/index.json",
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

    fn supports(&self, version: &CatalogArtifactVersion) -> bool {
        match self {
            Self::Auth => {
                version.schema_version == SUPPORTED_AUTH_TEMPLATE_SCHEMA_VERSION
                    && version
                        .configuration_version
                        .as_deref()
                        .is_some_and(|candidate| {
                            matches!(
                                candidate,
                                LEGACY_OIDC_CONFIGURATION_VERSION | OIDC_PROVIDER_CONFIG_VERSION
                            )
                        })
            }
            Self::Ai => {
                version.schema_version == SUPPORTED_CATALOG_SCHEMA_VERSION
                    && version.manifest_version.as_deref() == Some(SUPPORTED_MANIFEST_VERSION)
            }
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
        Self::new(client, &base_url)
    }

    pub fn new(client: Client, base_url: &str) -> Result<Self, DiscoveryError> {
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
        Ok(Self {
            client,
            base_url,
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
        let index = self.load_index(&kind).await?;
        let mut providers = Vec::new();
        for provider in index.providers {
            let compatible = compatible_versions(&kind, &provider, &selector)?;
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
        let index = self.load_index(kind).await?;
        let provider = index
            .providers
            .into_iter()
            .find(|provider| provider.id == provider_id)
            .ok_or(DiscoveryError::NotFound)?;
        let candidates = compatible_versions(kind, &provider, &selector)?;
        if candidates.is_empty() {
            return Err(DiscoveryError::NotFound);
        }
        Ok((provider, candidates))
    }

    async fn load_index(&self, kind: &CatalogKind) -> Result<CatalogIndex, DiscoveryError> {
        let bytes = self.fetch_document(kind.index_path()).await?;
        let index: CatalogIndex = serde_json::from_slice(&bytes)
            .map_err(|error| DiscoveryError::InvalidCatalog(error.to_string()))?;
        if index.schema_version != SUPPORTED_CATALOG_SCHEMA_VERSION {
            return Err(DiscoveryError::InvalidCatalog(format!(
                "unsupported catalog schema {}",
                index.schema_version
            )));
        }
        let mut ids = HashSet::new();
        if index.providers.iter().any(|provider| {
            validate_provider_id(&provider.id).is_err() || !ids.insert(provider.id.clone())
        }) {
            return Err(DiscoveryError::InvalidCatalog(
                "catalog contains an invalid or duplicate provider ID".to_string(),
            ));
        }
        Ok(index)
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
    kind: &CatalogKind,
    provider: &CatalogProvider,
    selector: &VersionSelector,
) -> Result<Vec<CatalogArtifactVersion>, DiscoveryError> {
    let mut versions = provider
        .versions
        .iter()
        .filter(|version| kind.supports(version))
        .map(|version| Ok((parse_release_version(&version.version)?, version.clone())))
        .collect::<Result<Vec<_>, DiscoveryError>>()?;
    versions.retain(|(version, _)| match selector {
        VersionSelector::Latest => true,
        VersionSelector::Major(major) => version.0[0] == *major,
        VersionSelector::Exact(expected) => version == expected,
    });
    versions.sort_by(|left, right| right.0.cmp(&left.0));
    if matches!(selector, VersionSelector::Latest)
        && let Some(latest_major) = versions.first().map(|(version, _)| version.0[0])
    {
        versions.retain(|(version, _)| version.0[0] == latest_major);
    }
    Ok(versions.into_iter().map(|(_, version)| version).collect())
}

fn verify_digest(bytes: &[u8], expected: &str) -> Result<(), DiscoveryError> {
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
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

    fn auth_index(id: &str, versions: Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion": "1.0",
            "catalogVersion": "1.0.0",
            "providers": [{
                "id": id,
                "name": "Test provider",
                "status": "stable",
                "versions": versions
            }]
        }))
        .expect("auth index")
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

    fn provider(versions: Vec<CatalogArtifactVersion>) -> CatalogProvider {
        CatalogProvider {
            id: "example".to_string(),
            name: "Example".to_string(),
            description: None,
            status: "stable".to_string(),
            icon: None,
            icon_dark: None,
            versions,
        }
    }

    #[test]
    fn major_selector_returns_newest_compatible_release_without_crossing_major() {
        let provider = provider(vec![
            version("1.0.0", "1.0", None, Some("1.0")),
            version("1.2.0", "1.0", None, Some("1.0")),
            version("2.0.0", "1.0", None, Some("2.0")),
        ]);
        let selected = compatible_versions(&CatalogKind::Ai, &provider, &VersionSelector::Major(1))
            .expect("compatible versions");
        assert_eq!(selected[0].version, "1.2.0");
        assert_eq!(selected[1].version, "1.0.0");
    }

    #[test]
    fn unsupported_contract_versions_are_filtered() {
        let provider = provider(vec![
            version("1.0.0", "1.0", Some("1.0"), None),
            version("1.1.0", "1.0", Some("1.1"), None),
            version("1.2.0", "1.0", Some("1.2"), None),
            version("2.0.0", "2.0", Some("1.1"), None),
        ]);
        let selected = compatible_versions(&CatalogKind::Auth, &provider, &VersionSelector::Latest)
            .expect("compatible versions");
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
        let provider = provider(vec![
            version("1.0.0", "1.0", None, Some("1.0")),
            version("1.1.0", "1.0", None, Some("1.0")),
        ]);
        let selected = compatible_versions(
            &CatalogKind::Ai,
            &provider,
            &VersionSelector::Exact(ReleaseVersion([1, 0, 0])),
        )
        .expect("compatible versions");
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].version, "1.0.0");
    }

    #[test]
    fn latest_selector_fallback_stays_within_latest_major() {
        let provider = provider(vec![
            version("1.9.0", "1.0", None, Some("1.0")),
            version("2.0.0", "1.0", None, Some("1.0")),
            version("2.1.0", "1.0", None, Some("1.0")),
        ]);
        let selected = compatible_versions(&CatalogKind::Ai, &provider, &VersionSelector::Latest)
            .expect("compatible versions");
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
        let index = auth_index(
            "example",
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
                    "/auth-providers/index.json",
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
        assert_eq!(resolved.version, "1.0.0");
    }

    #[tokio::test]
    async fn duplicate_provider_ids_and_upstream_failures_fail_closed() {
        let duplicate_index = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": "1.0",
            "catalogVersion": "1.0.0",
            "providers": [
                {"id": "duplicate", "name": "One", "status": "stable", "versions": []},
                {"id": "duplicate", "name": "Two", "status": "stable", "versions": []}
            ]
        }))
        .unwrap();
        let duplicate_base = serve(Router::new().route(
            "/auth-providers/index.json",
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
            "/auth-providers/index.json",
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
            "/auth-providers/index.json",
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
}
