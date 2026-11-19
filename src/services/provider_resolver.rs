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
    let direct = ai_engines::Entity::find()
        .filter(ai_engines::Column::EngineKey.eq(upstream))
        .filter(ai_engines::Column::IsEnabled.eq(true))
        .filter(ai_engines::Column::ApiKey.is_not_null())
        .one(&state.database)
        .await
        .map_err(|_| AppError::DbTimeout)?;
    if let Some(direct) = direct {
        let allowed = &direct.whitelist_models;
        if allowed.iter().any(|id| id == native) {
            return Ok((upstream.into(), native.into(), Some(engine.id)));
        }
        // Older installations can whitelist a catalog display name.
        if !allowed.is_empty() {
            let unavailable = || AppError::LlmProviderNotConfigured {
                provider: upstream.into(),
            };
            let plugin = resolve_provider(state, upstream)
                .await
                .map_err(|_| unavailable())?;
            let matched =
                find_provider_model(plugin.as_ref(), upstream, &state.live_models_cache, native)
                    .await
                    .map_err(|_| unavailable())?;
            if matched.is_some_and(|m| allowed.contains(&m.name)) {
                return Ok((upstream.into(), native.into(), Some(engine.id)));
            }
        }
    }
    Ok(("grengin".into(), model.into(), Some(engine.id)))
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
