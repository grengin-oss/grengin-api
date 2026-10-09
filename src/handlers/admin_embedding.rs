// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use axum::{Json, extract::State};
use reqwest::StatusCode;

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
        permissions::PERMISSION_AI_PLATFORM_MANAGE,
    },
    dto::admin_embedding::{EmbeddingConfigResponse, EmbeddingConfigUpdateRequest},
    services::{
        authorization::{AuthorizationService, PermissionScopeMode},
        embedding_helpers::{
            apply_embedding_config_update, get_or_create_embedding_config, model_to_response,
        },
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
       (status = 400, content_type = "application/json", body = Error, description = "Dimensions do not match the embedding vector columns (code=6307)"),
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

    let updated = apply_embedding_config_update(&app_state, req).await?;
    Ok((
        StatusCode::OK,
        Json(model_to_response(&app_state, &updated).await),
    ))
}
