// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashSet;

use anyhow::{Context, Result};
use chrono::Utc;
use llm_plugin::ProviderManifestV1;
use sea_orm::{ActiveValue::Set, EntityTrait, QueryOrder};
use uuid::Uuid;

use crate::{
    dto::discovery::DiscoveryProviderSummary,
    models::ai_engines::{self, ApiKeyStatus},
    services::discovery_catalog::DiscoveryCatalog,
};

/// Adds policy rows for discovery-backed providers without persisting their manifests.
/// Existing rows are authoritative and are never updated by reconciliation.
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
        .into_iter()
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
            Ok::<_, anyhow::Error>((provider, manifest))
        }
    }))
    .await?;

    if packages.is_empty() {
        return Ok(0);
    }

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
        .filter(|(provider, _)| !current_keys.contains(&provider.id.to_ascii_lowercase()))
        .map(|(provider, manifest)| ai_engines::ActiveModel {
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
            plugin_config: Set(None),
            api_key_validated_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        })
        .collect::<Vec<_>>();

    let inserted = engines.len();
    if inserted > 0 {
        ai_engines::Entity::insert_many(engines)
            .exec(database)
            .await
            .context("insert missing discovery-backed AI engines")?;
    }
    Ok(inserted)
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
}
