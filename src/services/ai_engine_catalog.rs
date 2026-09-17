// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashSet;

use anyhow::{Context, Result};
use chrono::Utc;
use llm_plugin::ProviderManifestV1;
use sea_orm::{ActiveValue::Set, ActiveModelTrait, EntityTrait, IntoActiveModel, QueryOrder};
use uuid::Uuid;

use crate::{
    dto::discovery::DiscoveryProviderSummary,
    models::ai_engines::{self, ApiKeyStatus, PluginConfigSource},
    services::{
        ai_plugin::RESERVED_ENGINE_KEYS, discovery_catalog::DiscoveryCatalog,
        provider_manifests, provider_runtime::parse_plugin_config,
    },
};

/// Adds policy rows for discovery-backed providers that have no local row yet, and
/// pins/upgrades the manifest of existing rows this reconciliation itself put in
/// catalog-tracking mode (see `upgrade_catalog_tracked_engines`). A row an admin
/// customized (`PluginConfigSource::Custom`) is never touched by either half.
pub async fn reconcile_catalog_ai_engines(
    database: &sea_orm::DatabaseConnection,
    catalog: &DiscoveryCatalog,
) -> Result<usize> {
    let existing = ai_engines::Entity::find()
        .order_by_desc(ai_engines::Column::CreatedAt)
        .all(database)
        .await
        .context("load existing AI engines before catalog reconciliation")?;
    let existing_keys = existing
        .iter()
        .map(|engine| engine.engine_key.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let providers = catalog
        .list_ai(None)
        .await
        .context("load the AI provider discovery distribution")?
        .providers;
    let missing = missing_providers(providers, &existing_keys);

    let packages = futures_util::future::try_join_all(missing.into_iter().map(|provider| {
        let catalog = catalog.clone();
        async move {
            let package = catalog
                .ai_provider(&provider.id, Some(&provider.selected_version))
                .await
                .with_context(|| format!("load discovery package for {}", provider.id))?;
            let manifest_bytes = serde_json::to_vec(&package.plugin)
                .context("serialize validated discovery provider manifest")?;
            let manifest = ProviderManifestV1::from_json(&manifest_bytes)
                .with_context(|| format!("validate discovery package for {}", provider.id))?;
            Ok::<_, anyhow::Error>((provider, package, manifest))
        }
    }))
    .await?;

    let inserted = if packages.is_empty() {
        0
    } else {
        // Recheck after network I/O to reduce duplicate inserts when replicas start together.
        let current_keys = ai_engines::Entity::find()
            .all(database)
            .await
            .context("reload AI engines before catalog reconciliation insert")?
            .into_iter()
            .map(|engine| engine.engine_key.to_ascii_lowercase())
            .collect::<HashSet<_>>();
        let now = Utc::now();
        let engines = packages
            .into_iter()
            .filter(|(provider, _, _)| !current_keys.contains(&provider.id.to_ascii_lowercase()))
            .map(|(provider, package, manifest)| {
                // Pinned at insert time to the exact version fetched here, rather than
                // left as None to be re-resolved live on every cache miss: that was the
                // silent-drift bug (a running provider could float onto whatever the CDN
                // mirror served next, ignoring this release's distribution pin).
                let plugin_config = ai_engines::PluginConfig {
                    manifest: package.plugin,
                    configuration: serde_json::json!({}),
                    base_url_override: None,
                    allow_insecure_http: false,
                    allow_private_network: false,
                    source: PluginConfigSource::Catalog,
                    version: Some(package.version),
                    sha256: Some(package.sha256),
                };
                ai_engines::ActiveModel {
                    id: Set(Uuid::new_v4()),
                    display_name: Set(provider.name),
                    is_enabled: Set(false),
                    engine_key: Set(provider.id),
                    api_key_status: Set(ApiKeyStatus::NotConfigured),
                    api_key: Set(None),
                    whitelist_models: Set(manifest
                        .models
                        .into_iter()
                        .map(|model| model.id.to_string())
                        .collect()),
                    default_model: Set("<empty>".to_string()),
                    default_image_gen_model: Set(None),
                    plugin_config: Set(serde_json::to_value(&plugin_config).ok()),
                    api_key_validated_at: Set(None),
                    created_at: Set(now),
                    updated_at: Set(now),
                }
            })
            .collect::<Vec<_>>();

        let inserted = engines.len();
        if inserted > 0 {
            ai_engines::Entity::insert_many(engines)
                .exec(database)
                .await
                .context("insert missing discovery-backed AI engines")?;
        }
        inserted
    };

    if let Err(error) = upgrade_catalog_tracked_engines(database, catalog, &existing).await {
        eprintln!("catalog AI engine version upgrade failed: {error:#}");
    }

    Ok(inserted)
}

/// Upgrades every existing row this reconciliation itself put in catalog-tracking
/// mode (`plugin_config` unset, or previously pinned with `source: Catalog`) to
/// the distribution's current pin for that provider. Rows an admin customized
/// (`source: Custom`) and the four engines whose manifest is compiled into this
/// binary (`RESERVED_ENGINE_KEYS` — a catalog version bump for these is a no-op
/// since the embedded manifest always wins at resolution time) are left alone.
async fn upgrade_catalog_tracked_engines(
    database: &sea_orm::DatabaseConnection,
    catalog: &DiscoveryCatalog,
    existing: &[ai_engines::Model],
) -> Result<usize> {
    let providers = catalog
        .list_ai(None)
        .await
        .context("load the AI provider discovery distribution for upgrade check")?
        .providers
        .into_iter()
        .map(|provider| (provider.id.to_ascii_lowercase(), provider.selected_version))
        .collect::<std::collections::HashMap<_, _>>();

    let mut upgraded = 0usize;
    for engine in existing {
        if RESERVED_ENGINE_KEYS.contains(&engine.engine_key.as_str()) {
            continue;
        }
        let Some(pinned_version) = providers.get(&engine.engine_key.to_ascii_lowercase()) else {
            continue;
        };
        let stored_source_and_version = engine
            .plugin_config
            .as_ref()
            .and_then(|value| parse_plugin_config(value).ok())
            .map(|config| (config.source, config.version));
        match upgrade_decision(stored_source_and_version.as_ref(), pinned_version) {
            UpgradeDecision::Skip => continue,
            UpgradeDecision::UpToDate => continue,
            UpgradeDecision::Upgrade => {}
        }

        let package = match catalog.ai_provider(&engine.engine_key, Some(pinned_version)).await {
            Ok(package) => package,
            Err(error) => {
                eprintln!(
                    "skipping plugin upgrade for {}: {error}",
                    engine.engine_key
                );
                continue;
            }
        };
        let manifest = match ProviderManifestV1::from_json(
            &serde_json::to_vec(&package.plugin).unwrap_or_default(),
        ) {
            Ok(manifest) => manifest,
            Err(error) => {
                eprintln!(
                    "skipping plugin upgrade for {}: pinned version {} failed validation: {error}",
                    engine.engine_key, package.version
                );
                continue;
            }
        };
        if manifest.id != engine.engine_key {
            eprintln!(
                "skipping plugin upgrade for {}: distribution manifest declares id {}",
                engine.engine_key, manifest.id
            );
            continue;
        }

        let new_plugin_config = ai_engines::PluginConfig {
            manifest: package.plugin,
            configuration: serde_json::json!({}),
            base_url_override: None,
            allow_insecure_http: false,
            allow_private_network: false,
            source: PluginConfigSource::Catalog,
            version: Some(package.version),
            sha256: Some(package.sha256),
        };
        let Ok(serialized) = serde_json::to_value(&new_plugin_config) else {
            continue;
        };

        // Additive only: a model the new manifest dropped stays whitelisted (it's a
        // ceiling, not a mirror of upstream) rather than risking an admin's chosen
        // default_model or chat history referencing a now-unlisted id.
        let mut whitelist_models = engine.whitelist_models.clone();
        for model in &manifest.models {
            if !whitelist_models.iter().any(|id| id == model.id.as_str()) {
                whitelist_models.push(model.id.to_string());
            }
        }

        let mut active = engine.clone().into_active_model();
        active.plugin_config = Set(Some(serialized));
        active.whitelist_models = Set(whitelist_models);
        active.updated_at = Set(Utc::now());
        if let Err(error) = active.update(database).await {
            eprintln!(
                "failed to persist plugin upgrade for {}: {error}",
                engine.engine_key
            );
            continue;
        }
        provider_manifests::invalidate(&engine.engine_key);
        upgraded += 1;
    }
    Ok(upgraded)
}

#[derive(Debug, PartialEq, Eq)]
enum UpgradeDecision {
    /// Admin-customized or compiled-in; reconciliation must never touch it.
    Skip,
    /// Already pinned to the distribution's current version for this provider.
    UpToDate,
    /// Never pinned (legacy row), or pinned to a version the distribution moved past.
    Upgrade,
}

fn upgrade_decision(
    stored: Option<&(PluginConfigSource, Option<String>)>,
    pinned_version: &str,
) -> UpgradeDecision {
    match stored {
        None => UpgradeDecision::Upgrade,
        Some((PluginConfigSource::Custom | PluginConfigSource::Embedded, _)) => {
            UpgradeDecision::Skip
        }
        Some((PluginConfigSource::Catalog, version)) => {
            if version.as_deref() == Some(pinned_version) {
                UpgradeDecision::UpToDate
            } else {
                UpgradeDecision::Upgrade
            }
        }
    }
}

fn missing_providers(
    providers: Vec<DiscoveryProviderSummary>,
    existing_keys: &HashSet<String>,
) -> Vec<DiscoveryProviderSummary> {
    providers
        .into_iter()
        .filter(|provider| !existing_keys.contains(&provider.id.to_ascii_lowercase()))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::dto::discovery::{DiscoveryProviderSummary, DiscoveryVersion};

    use super::missing_providers;

    fn provider(id: &str) -> DiscoveryProviderSummary {
        DiscoveryProviderSummary {
            id: id.to_string(),
            name: id.to_string(),
            description: None,
            status: "active".to_string(),
            icon: None,
            icon_dark: None,
            selected_version: "1.0".to_string(),
            latest_version: "1.0".to_string(),
            available_versions: vec![DiscoveryVersion {
                version: "1.0".to_string(),
                schema_version: "1.0".to_string(),
                contract_version: "1.0".to_string(),
                sha256: "a".repeat(64),
            }],
        }
    }

    #[test]
    fn reconciliation_selects_only_missing_catalog_providers() {
        let existing = HashSet::from(["openai".to_string(), "sakana".to_string()]);
        let missing = missing_providers(
            vec![provider("openai"), provider("Sakana"), provider("tinker")],
            &existing,
        );

        assert_eq!(
            missing
                .into_iter()
                .map(|provider| provider.id)
                .collect::<Vec<_>>(),
            vec!["tinker"]
        );
    }

    #[test]
    fn upgrade_decision_pins_legacy_rows_and_follows_the_distribution() {
        use super::{PluginConfigSource, UpgradeDecision, upgrade_decision};

        // Never persisted (old insert-only reconcile): pin it now.
        assert_eq!(upgrade_decision(None, "1.4"), UpgradeDecision::Upgrade);

        // Pinned, distribution moved on: upgrade.
        assert_eq!(
            upgrade_decision(
                Some(&(PluginConfigSource::Catalog, Some("1.3".to_string()))),
                "1.4"
            ),
            UpgradeDecision::Upgrade
        );

        // Pinned, distribution unchanged: leave it, no spurious writes.
        assert_eq!(
            upgrade_decision(
                Some(&(PluginConfigSource::Catalog, Some("1.4".to_string()))),
                "1.4"
            ),
            UpgradeDecision::UpToDate
        );

        // Admin-customized manifest: never touched, regardless of version drift.
        assert_eq!(
            upgrade_decision(
                Some(&(PluginConfigSource::Custom, Some("1.3".to_string()))),
                "1.4"
            ),
            UpgradeDecision::Skip
        );

        // Compiled-in manifest: a catalog version bump is a no-op for it either way.
        assert_eq!(
            upgrade_decision(Some(&(PluginConfigSource::Embedded, None)), "1.4"),
            UpgradeDecision::Skip
        );
    }
}
