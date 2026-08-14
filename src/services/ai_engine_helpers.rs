// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::error::AuthError,
    dto::models::ModelsResponse,
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
