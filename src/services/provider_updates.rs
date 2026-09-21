// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use llm_plugin::{ProviderPlugin, ProviderRegistry};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::{
    models::ai_engines,
    services::{provider_manifests, provider_runtime},
    state::SharedState,
};

const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const REFRESH_INTERVAL_ENV: &str = "GRENGIN_PLUGIN_REFRESH_INTERVAL_SECONDS";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ProviderRefreshReport {
    pub checked: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub failed: usize,
}

pub fn spawn_provider_plugin_refresh(state: SharedState) {
    let Some(interval) = refresh_interval_from_env() else {
        eprintln!("provider plugin auto-update disabled");
        return;
    };
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let report = refresh_catalog_providers(&state).await;
            if report.updated > 0 || report.failed > 0 {
                eprintln!(
                    "provider plugin refresh: checked={}, updated={}, unchanged={}, skipped={}, failed={}",
                    report.checked, report.updated, report.unchanged, report.skipped, report.failed
                );
            }
        }
    });
}

pub async fn refresh_catalog_providers(state: &SharedState) -> ProviderRefreshReport {
    let engines = match ai_engines::Entity::find()
        .filter(ai_engines::Column::IsEnabled.eq(true))
        .filter(ai_engines::Column::PluginConfig.is_null())
        .all(&state.database)
        .await
    {
        Ok(engines) => engines,
        Err(error) => {
            eprintln!("provider plugin refresh database query failed: {error}");
            return ProviderRefreshReport {
                failed: 1,
                ..Default::default()
            };
        }
    };

    let mut report = ProviderRefreshReport::default();
    for engine in engines {
        if provider_runtime::is_embedded_provider(&engine.engine_key) {
            report.skipped += 1;
            continue;
        }
        report.checked += 1;
        match refresh_engine(state, &engine).await {
            Ok(RefreshOutcome::Updated) => report.updated += 1,
            Ok(RefreshOutcome::Unchanged) => report.unchanged += 1,
            Ok(RefreshOutcome::Skipped) => report.skipped += 1,
            Err(error) => {
                report.failed += 1;
                eprintln!(
                    "provider plugin refresh failed for {}: {error:#}",
                    engine.engine_key
                );
            }
        }
    }
    report
}

#[derive(Debug, PartialEq, Eq)]
enum RefreshOutcome {
    Updated,
    Unchanged,
    Skipped,
}

async fn refresh_engine(
    state: &SharedState,
    snapshot: &ai_engines::Model,
) -> Result<RefreshOutcome> {
    let candidate =
        provider_manifests::fetch_catalog_plugin(&state.discovery_catalog, &snapshot.engine_key)
            .await
            .context("fetch versioned catalog package")?;
    let installed = provider_manifests::cached_catalog_plugin(&snapshot.engine_key);
    let active = state
        .provider_registry
        .get_by_str(&snapshot.engine_key)
        .await;
    if !refresh_needed(
        installed.as_ref(),
        active
            .as_ref()
            .map(|provider| provider.descriptor().version.as_str()),
        &candidate,
    ) {
        return Ok(RefreshOutcome::Unchanged);
    }

    // Re-read after network I/O. An admin may have disabled, deleted, or
    // customized the engine while the catalog request was in flight.
    let Some(current) = ai_engines::Entity::find_by_id(snapshot.id)
        .one(&state.database)
        .await
        .context("recheck AI engine before plugin replacement")?
    else {
        return Ok(RefreshOutcome::Skipped);
    };
    if !is_still_refresh_target(snapshot, &current) {
        return Ok(RefreshOutcome::Skipped);
    }

    replace_provider(
        &state.provider_registry,
        &state.settings.auth.app_key,
        &current,
        candidate.config.clone(),
    )
    .await
    .context("compile and install catalog provider")?;
    provider_manifests::install_catalog_plugin(&current.engine_key, candidate);
    state
        .live_models_cache
        .invalidate(&current.engine_key)
        .await;
    Ok(RefreshOutcome::Updated)
}

fn is_still_refresh_target(snapshot: &ai_engines::Model, current: &ai_engines::Model) -> bool {
    current.is_enabled
        && current.plugin_config.is_none()
        && current.updated_at == snapshot.updated_at
        && current.engine_key == snapshot.engine_key
}

async fn replace_provider(
    registry: &ProviderRegistry,
    app_key: &[u8; 32],
    engine: &ai_engines::Model,
    config: ai_engines::PluginConfig,
) -> Result<()> {
    let provider = provider_runtime::compile_provider_for_engine(app_key, engine, config)?;
    let provider: Arc<dyn ProviderPlugin> = Arc::new(provider);
    registry.register(provider).await;
    Ok(())
}

fn refresh_needed(
    installed: Option<&provider_manifests::CatalogPlugin>,
    active_version: Option<&str>,
    candidate: &provider_manifests::CatalogPlugin,
) -> bool {
    let installed_matches = installed.is_some_and(|installed| {
        installed.version == candidate.version && installed.sha256 == candidate.sha256
    });
    !(installed_matches && active_version == Some(candidate.version.as_str()))
}

fn refresh_interval_from_env() -> Option<Duration> {
    parse_refresh_interval(std::env::var(REFRESH_INTERVAL_ENV).ok().as_deref())
}

fn parse_refresh_interval(value: Option<&str>) -> Option<Duration> {
    match value {
        Some(value) => match value.trim().parse::<u64>() {
            Ok(0) => None,
            Ok(seconds) => Some(Duration::from_secs(seconds)),
            Err(_) => {
                eprintln!(
                    "invalid {REFRESH_INTERVAL_ENV}; using {} seconds",
                    DEFAULT_REFRESH_INTERVAL.as_secs()
                );
                Some(DEFAULT_REFRESH_INTERVAL)
            }
        },
        None => Some(DEFAULT_REFRESH_INTERVAL),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;

    fn catalog_plugin(version: &str, sha256: &str) -> provider_manifests::CatalogPlugin {
        let mut config = provider_runtime::embedded_plugin_config("openai")
            .expect("embedded config")
            .expect("openai config");
        config.manifest["version"] = json!(version);
        provider_manifests::CatalogPlugin {
            config,
            version: version.to_string(),
            sha256: sha256.to_string(),
        }
    }

    fn engine(engine_key: &str) -> ai_engines::Model {
        let now = Utc::now();
        ai_engines::Model {
            id: Uuid::new_v4(),
            display_name: engine_key.to_string(),
            is_enabled: true,
            engine_key: engine_key.to_string(),
            api_key_status: ai_engines::ApiKeyStatus::NotValidated,
            api_key: None,
            whitelist_models: Vec::new(),
            default_model: "<empty>".to_string(),
            default_image_gen_model: None,
            plugin_config: None,
            api_key_validated_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn refreshes_for_version_digest_or_missing_runtime_changes() {
        let v1 = catalog_plugin("1.1", "a");
        let v1_repacked = catalog_plugin("1.1", "b");
        let v2 = catalog_plugin("1.2", "c");

        assert!(!refresh_needed(Some(&v1), Some("1.1"), &v1));
        assert!(refresh_needed(Some(&v1), Some("1.1"), &v1_repacked));
        assert!(refresh_needed(Some(&v1), Some("1.1"), &v2));
        assert!(refresh_needed(Some(&v1), None, &v1));
        assert!(refresh_needed(None, Some("1.1"), &v1));
    }

    #[tokio::test]
    async fn failed_candidate_does_not_replace_the_active_provider() {
        let registry = ProviderRegistry::new();
        let current_config = provider_runtime::embedded_plugin_config("openai")
            .expect("embedded config")
            .expect("openai config");
        let current = provider_runtime::compile_provider(
            current_config,
            "openai",
            Some("test-key".to_string()),
        )
        .expect("compile current provider");
        registry.register(Arc::new(current)).await;

        let mut invalid = provider_runtime::embedded_plugin_config("openai")
            .expect("embedded config")
            .expect("openai config");
        invalid.manifest["id"] = json!("different-provider");
        assert!(
            replace_provider(&registry, &[7; 32], &engine("openai"), invalid)
                .await
                .is_err()
        );
        assert_eq!(
            registry
                .get_by_str("openai")
                .await
                .expect("active provider retained")
                .descriptor()
                .id
                .as_str(),
            "openai"
        );
    }

    #[test]
    fn embedded_providers_are_not_catalog_update_targets() {
        for provider in ["openai", "anthropic", "mistral", "gemini"] {
            assert!(provider_runtime::is_embedded_provider(provider));
        }
        assert!(!provider_runtime::is_embedded_provider("groq"));
    }

    #[test]
    fn concurrent_policy_changes_cancel_catalog_replacement() {
        let snapshot = engine("groq");
        assert!(is_still_refresh_target(&snapshot, &snapshot));

        let mut disabled = snapshot.clone();
        disabled.is_enabled = false;
        assert!(!is_still_refresh_target(&snapshot, &disabled));

        let mut customized = snapshot.clone();
        customized.plugin_config = Some(json!({"manifest": {}}));
        assert!(!is_still_refresh_target(&snapshot, &customized));

        let mut updated = snapshot.clone();
        updated.updated_at += chrono::Duration::seconds(1);
        assert!(!is_still_refresh_target(&snapshot, &updated));

        let mut renamed = snapshot.clone();
        renamed.engine_key = "another-provider".to_string();
        assert!(!is_still_refresh_target(&snapshot, &renamed));
    }

    #[test]
    fn refresh_interval_supports_default_override_and_disable() {
        assert_eq!(parse_refresh_interval(None), Some(DEFAULT_REFRESH_INTERVAL));
        assert_eq!(
            parse_refresh_interval(Some("30")),
            Some(Duration::from_secs(30))
        );
        assert_eq!(parse_refresh_interval(Some("0")), None);
        assert_eq!(
            parse_refresh_interval(Some("invalid")),
            Some(DEFAULT_REFRESH_INTERVAL)
        );
    }
}
