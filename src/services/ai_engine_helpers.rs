// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::error::AuthError,
    dto::models::ModelsResponse,
    models::ai_engines::{self, ApiKeyStatus},
    services::{
        chat_attachments::global_attachments,
        models_cache::{ProvidersCache, load_models_cache, refresh_models_cache},
    },
    state::SharedState,
};
use sea_orm::{
    ColumnTrait, DatabaseTransaction, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};

// An update reads, checks and writes the row under this lock, so a concurrent update can never
// pair one request's key with another request's destination.
pub async fn find_engine_for_update(
    txn: &DatabaseTransaction,
    engine_key: &str,
) -> Result<ai_engines::Model, AuthError> {
    ai_engines::Entity::find()
        .filter(ai_engines::Column::EngineKey.eq(engine_key))
        .order_by_desc(ai_engines::Column::CreatedAt)
        .lock_exclusive()
        .one(txn)
        .await
        .map_err(|e| {
            eprintln!("db error lock ai engine {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)
}

pub async fn load_models_response(app_state: &SharedState) -> Result<ModelsResponse, AuthError> {
    let cache = load_models_cache(&app_state.req_client)
        .await
        .map_err(|e| {
            eprintln!("providers cache error: {e}");
            AuthError::DbTimeout
        })?;
    Ok(models_response(app_state, cache))
}

fn models_response(app_state: &SharedState, cache: ProvidersCache) -> ModelsResponse {
    ModelsResponse {
        specialties: cache.picker.specialties(),
        price_tiers: cache.picker.price_tiers(),
        providers: cache.providers,
        global_attachments: global_attachments(&app_state.settings.attachments),
    }
}

pub async fn load_models_response_refreshed(
    app_state: &SharedState,
) -> Result<ModelsResponse, AuthError> {
    let cache = match refresh_models_cache(&app_state.req_client).await {
        Ok(cache) => cache,
        Err(error) => {
            eprintln!("providers cache refresh error: {error}");
            load_models_cache(&app_state.req_client)
                .await
                .map_err(|e| {
                    eprintln!("providers cache fallback error: {e}");
                    AuthError::DbTimeout
                })?
        }
    };
    Ok(models_response(app_state, cache))
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
