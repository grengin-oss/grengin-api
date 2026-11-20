// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use llm_plugin::ProviderPlugin;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use thiserror::Error;

use crate::{
    error::AppError,
    models::ai_engines,
    services::{
        grengin_provider::split_model, provider_models::find_provider_model, provider_runtime,
    },
    state::SharedState,
};

/// Resolve funding before any prompt or attachment leaves the workspace.
pub async fn model_route(
    state: &SharedState,
    provider: &str,
    model: &str,
) -> Result<(String, String, Option<uuid::Uuid>), AppError> {
    let requested = (provider.to_owned(), model.to_owned(), None);
    if !provider.eq_ignore_ascii_case("grengin") {
        return Ok(requested);
    }
    let engine = ai_engines::Entity::find()
        .filter(ai_engines::Column::EngineKey.eq("grengin"))
        .one(&state.database)
        .await
        .map_err(|_| AppError::DbTimeout)?
        .ok_or_else(|| AppError::InvalidLlmProvider {
            provider: provider.into(),
        })?;
    if !engine.is_enabled {
        return Err(AppError::LlmProviderDisabledByAdmin {
            provider: provider.into(),
        });
    }
    if engine.plugin_config.is_some() {
        return Ok(requested); // A pre-existing custom engine owns its own credentials.
    }
    if !engine.whitelist_models.iter().any(|id| id == model) {
        return Err(AppError::DepartmentModelNotAllowed {
            provider: provider.into(),
            model: model.into(),
        });
    }
    let (upstream, native) = split_model(model).map_err(|_| AppError::ValidationInvalidField {
        field: "model_name",
    })?;
    if own_key_covers(state, upstream, native).await? {
        return Ok((upstream.into(), native.into(), Some(engine.id)));
    }
    Ok(("grengin".into(), model.into(), Some(engine.id)))
}

/// Whether an enabled direct engine with its own key allows `upstream/native`. Such a model is
/// funded by that key, never by the workspace balance; an excluded model falls to Grengin.
pub async fn own_key_covers(
    state: &SharedState,
    upstream: &str,
    native: &str,
) -> Result<bool, AppError> {
    let direct = ai_engines::Entity::find()
        .filter(ai_engines::Column::EngineKey.eq(upstream))
        .filter(ai_engines::Column::IsEnabled.eq(true))
        .filter(ai_engines::Column::ApiKey.is_not_null())
        .one(&state.database)
        .await
        .map_err(|_| AppError::DbTimeout)?;
    if let Some(decided) = own_key_decision(direct.as_ref(), native) {
        return Ok(decided);
    }
    let Some(direct) = direct else {
        return Ok(false);
    };
    let allowed = &direct.whitelist_models;
    let unavailable = || AppError::LlmProviderNotConfigured {
        provider: upstream.into(),
    };
    let plugin = resolve_provider(state, upstream)
        .await
        .map_err(|_| unavailable())?;
    let matched = find_provider_model(plugin.as_ref(), upstream, &state.live_models_cache, native)
        .await
        .map_err(|_| unavailable())?;
    Ok(matched.is_some_and(|m| allowed.contains(&m.name)))
}

/// The decision from the direct engine row alone (it is already filtered to enabled engines with
/// a key); `None` means its whitelist may name the model by its catalog display name.
fn own_key_decision(direct: Option<&ai_engines::Model>, native: &str) -> Option<bool> {
    let Some(direct) = direct else {
        return Some(false);
    };
    let allowed = &direct.whitelist_models;
    if allowed.iter().any(|id| id == native) {
        Some(true)
    } else if allowed.is_empty() {
        Some(false)
    } else {
        None
    }
}

#[derive(Debug, Error)]
pub enum ResolveProviderError {
    #[error("provider is not configured")]
    NotConfigured,
    #[error("provider is disabled")]
    Disabled,
    #[error("provider does not exist")]
    Unknown,
}

pub async fn resolve_provider(
    state: &SharedState,
    provider_key: &str,
) -> Result<Arc<dyn ProviderPlugin>, ResolveProviderError> {
    if let Some(provider) = state.provider_registry.get_by_str(provider_key).await {
        return Ok(provider);
    }
    let engine = ai_engines::Entity::find()
        .filter(ai_engines::Column::EngineKey.eq(provider_key))
        .one(&state.database)
        .await
        .map_err(|_| ResolveProviderError::NotConfigured)?
        .ok_or(ResolveProviderError::Unknown)?;
    if !engine.is_enabled {
        return Err(ResolveProviderError::Disabled);
    }
    let provider = provider_runtime::build_provider(state, &engine)
        .await
        .map_err(|_| ResolveProviderError::NotConfigured)?;
    state.provider_registry.register(provider.clone()).await;
    Ok(provider)
}

#[cfg(test)]
mod own_key_tests {
    use super::own_key_decision;
    use crate::models::ai_engines;

    fn direct(whitelist: &[&str]) -> ai_engines::Model {
        serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(), "displayName": "OpenAI", "engineKey": "openai",
            "isEnabled": true, "apiKeyStatus": "valid", "apiKey": "encrypted",
            "apiKeyValidatedAt": null, "pluginConfig": null,
            "whitelistModels": whitelist, "defaultModel": "", "defaultImageGenModel": null,
            "createdAt": chrono::Utc::now(), "updatedAt": chrono::Utc::now()
        }))
        .unwrap()
    }

    #[test]
    fn an_own_key_funds_only_models_its_engine_allows() {
        assert_eq!(own_key_decision(None, "gpt-5"), Some(false));
        assert_eq!(
            own_key_decision(Some(&direct(&["gpt-5"])), "gpt-5"),
            Some(true)
        );
        assert_eq!(own_key_decision(Some(&direct(&[])), "gpt-5"), Some(false));
        // Another entry may be a display name for this model: resolved through the catalog.
        assert_eq!(
            own_key_decision(Some(&direct(&["GPT-5 mini"])), "gpt-5"),
            None
        );
    }
}
