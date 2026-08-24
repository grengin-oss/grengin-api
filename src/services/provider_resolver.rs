// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use llm_plugin::ProviderPlugin;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use thiserror::Error;

use crate::{models::ai_engines, services::provider_runtime, state::SharedState};

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
    let provider =
        provider_runtime::build_provider(&state.settings.auth.app_key, &state.req_client, &engine)
            .await
            .map_err(|_| ResolveProviderError::NotConfigured)?;
    let provider: Arc<dyn ProviderPlugin> = Arc::new(provider);
    state.provider_registry.register(provider.clone()).await;
    Ok(provider)
}
