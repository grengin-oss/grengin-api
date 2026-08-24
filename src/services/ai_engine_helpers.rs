// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::error::AuthError,
    dto::models::ModelsResponse,
    models::ai_engines::{self, ApiKeyStatus},
    services::models_cache::{load_providers_cached, refresh_models_cache},
    state::SharedState,
};

pub async fn load_models_response(app_state: &SharedState) -> Result<ModelsResponse, AuthError> {
    let providers = load_providers_cached(&app_state.req_client)
        .await
        .map_err(|e| {
            eprintln!("providers cache error: {e}");
            AuthError::DbTimeout
        })?;
    Ok(ModelsResponse { providers })
}

pub async fn load_models_response_refreshed(
    app_state: &SharedState,
) -> Result<ModelsResponse, AuthError> {
    let providers = match refresh_models_cache(&app_state.req_client).await {
        Ok(cache) => cache.providers,
        Err(error) => {
            eprintln!("providers cache refresh error: {error}");
            load_providers_cached(&app_state.req_client)
                .await
                .map_err(|e| {
                    eprintln!("providers cache fallback error: {e}");
                    AuthError::DbTimeout
                })?
        }
    };
    Ok(ModelsResponse { providers })
}

// Ready engines lead the admin list: enabled first, then by how far the credential got,
// then newest. api_key_status is a DB enum whose stored strings sort alphabetically, which
// puts Invalid above Valid, so the rank is spelled out instead of ordered in SQL.
pub fn sort_engines_by_readiness(engines: &mut [ai_engines::Model]) {
    engines.sort_by(|a, b| {
        b.is_enabled
            .cmp(&a.is_enabled)
            .then_with(|| {
                credential_rank(&a.api_key_status).cmp(&credential_rank(&b.api_key_status))
            })
            .then_with(|| b.created_at.cmp(&a.created_at))
    });
}

fn credential_rank(status: &ApiKeyStatus) -> u8 {
    match status {
        ApiKeyStatus::Valid => 0,
        ApiKeyStatus::NotValidated => 1,
        ApiKeyStatus::Invalid => 2,
        ApiKeyStatus::NotConfigured => 3,
    }
}
