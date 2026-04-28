use axum::{Json, extract::State};
use chrono::Utc;
use reqwest::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait, IntoActiveModel, QueryOrder};
use uuid::Uuid;

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
        permissions::PERMISSION_AI_PLATFORM_MANAGE,
    },
    dto::admin_embedding::{EmbeddingConfigResponse, EmbeddingConfigUpdateRequest},
    models::embedding_configs,
    services::authorization::{AuthorizationService, PermissionScopeMode},
    state::SharedState,
};

const DEFAULT_PROVIDER: &str = "openai";
const DEFAULT_MODEL: &str = "text-embedding-3-small";
const DEFAULT_DIMENSIONS: i32 = 1536;

async fn get_or_create_embedding_config(
    app_state: &SharedState,
) -> Result<embedding_configs::Model, AuthError> {
    if let Some(model) = embedding_configs::Entity::find()
        .order_by_desc(embedding_configs::Column::UpdatedAt)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("embedding config query error: {e}");
            AuthError::DbTimeout
        })?
    {
        return Ok(model);
    }

    let is_enabled = app_state
        .check_ai_engine_is_enabled(DEFAULT_PROVIDER)
        .await
        .unwrap_or(false);

    let model = embedding_configs::ActiveModel {
        id: Set(Uuid::new_v4()),
        provider: Set(DEFAULT_PROVIDER.to_string()),
        model: Set(DEFAULT_MODEL.to_string()),
        dimensions: Set(Some(DEFAULT_DIMENSIONS)),
        is_enabled: Set(is_enabled),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
    };

    model.insert(&app_state.database).await.map_err(|e| {
        eprintln!("embedding config insert error: {e}");
        AuthError::DbTimeout
    })
}

async fn model_to_response(
    app_state: &SharedState,
    model: &embedding_configs::Model,
) -> EmbeddingConfigResponse {
    let api_key_configured = app_state
        .settings
        .get_ai_engine_api_key(&model.provider)
        .await
        .is_some();
    let provider_enabled = app_state
        .check_ai_engine_is_enabled(&model.provider)
        .await
        .unwrap_or(false);
    EmbeddingConfigResponse {
        provider: model.provider.clone(),
        model: model.model.clone(),
        dimensions: model.dimensions,
        is_enabled: model.is_enabled,
        api_key_configured,
        provider_enabled,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

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

    // Provider and model are immutable after initial configuration.
    // Idempotent requests with the same values are allowed.
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
