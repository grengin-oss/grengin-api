// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use axum::{Json, extract::State};
use chrono::Utc;
use reqwest::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, IntoActiveModel};

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
        permissions::PERMISSION_AI_PLATFORM_MANAGE,
    },
    dto::admin_embedding::{EmbeddingConfigResponse, EmbeddingConfigUpdateRequest},
    services::{
        authorization::{AuthorizationService, PermissionScopeMode},
        embedding_helpers::{get_or_create_embedding_config, model_to_response},
    },
    state::SharedState,
};

#[utoipa::path(
    get,
    path = "/admin/embedding-config",
    tag = "admin",
    responses(
       (status = 200, body = EmbeddingConfigResponse),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 403, content_type = "application/json", body = Error, description = "Permission denied"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000)"),
    )
)]
pub async fn get_embedding_config(
    claims: Claims,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<EmbeddingConfigResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AI_PLATFORM_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let model = get_or_create_embedding_config(&app_state).await?;
    Ok((
        StatusCode::OK,
        Json(model_to_response(&app_state, &model).await),
    ))
}

#[utoipa::path(
    put,
    path = "/admin/embedding-config",
    tag = "admin",
    request_body = EmbeddingConfigUpdateRequest,
    responses(
       (status = 200, body = EmbeddingConfigResponse),
       (status = 409, content_type = "application/json", body = Error, description = "Embedding provider/model cannot be changed once configured"),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 403, content_type = "application/json", body = Error, description = "Permission denied"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000)"),
    )
)]
pub async fn update_embedding_config(
    claims: Claims,
    State(app_state): State<SharedState>,
    Json(req): Json<EmbeddingConfigUpdateRequest>,
) -> Result<(StatusCode, Json<EmbeddingConfigResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AI_PLATFORM_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let config = get_or_create_embedding_config(&app_state).await?;

    if let Some(provider) = req.provider.as_ref() {
        if provider != &config.provider {
            return Err(AuthError::DbConflict);
        }
    }
    if let Some(model) = req.model.as_ref() {
        if model != &config.model {
            return Err(AuthError::DbConflict);
        }
    }

    let mut active = config.into_active_model();

    if let Some(dimensions) = req.dimensions {
        active.dimensions = Set(Some(dimensions));
    }
    if let Some(is_enabled) = req.is_enabled {
        active.is_enabled = Set(is_enabled);
    }
    active.updated_at = Set(Utc::now());

    let updated = active.update(&app_state.database).await.map_err(|e| {
        eprintln!("embedding config update error: {e}");
        AuthError::DbTimeout
    })?;

    app_state
        .settings
        .set_embedding_config_in_state(crate::config::setting::EmbeddingSettings {
            provider: updated.provider.clone(),
            model: updated.model.clone(),
            dimensions: updated.dimensions,
            is_enabled: updated.is_enabled,
        })
        .await;

    Ok((
        StatusCode::OK,
        Json(model_to_response(&app_state, &updated).await),
    ))
}
