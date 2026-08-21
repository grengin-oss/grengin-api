// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use llm_plugin::ProviderPlugin;
use sea_orm::EntityTrait;

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
    },
    dto::admin_ai::{
        AIEngineConnectionTest, AIEngineCreate, AIEngineDetail, AIEnginePluginValidationRequest,
        AIEnginePluginValidationResponse, PluginModel,
    },
    models::ai_engines,
    services::{
        ai_plugin::{
            RESERVED_ENGINE_KEYS, create_engine, ensure_manage, ensure_view, find_engine,
            invalid_validation, test_engine_connection,
        },
        provider_runtime::{compile_provider, parse_manifest, unregister_provider},
    },
    state::SharedState,
};

#[utoipa::path(
    get,
    path = "/admin/ai-engines/plugin-schema",
    tag = "admin",
    responses(
        (status = 200, body = Object),
        (status = 401, body = Error),
        (status = 403, body = Error)
    )
)]
pub async fn get_ai_engine_plugin_schema(
    claims: Claims,
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, AuthError> {
    ensure_view(&claims, &state).await?;
    let schema = serde_json::from_str(include_str!(
        "../../llm-plugin/schema/provider-plugin-v1.schema.json"
    ))
    .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    Ok(Json(schema))
}

#[utoipa::path(
    post,
    path = "/admin/ai-engines/plugin-validate",
    tag = "admin",
    request_body = AIEnginePluginValidationRequest,
    responses(
        (status = 200, body = AIEnginePluginValidationResponse),
        (status = 401, body = Error),
        (status = 403, body = Error)
    )
)]
pub async fn validate_ai_engine_plugin(
    claims: Claims,
    State(state): State<SharedState>,
    Json(request): Json<AIEnginePluginValidationRequest>,
) -> Result<Json<AIEnginePluginValidationResponse>, AuthError> {
    ensure_manage(&claims, &state).await?;
    let manifest = match parse_manifest(&request.plugin_config) {
        Ok(manifest) => manifest,
        Err(error) => return Ok(Json(invalid_validation(error.to_string()))),
    };
    if RESERVED_ENGINE_KEYS.contains(&manifest.id.as_str()) {
        return Ok(Json(invalid_validation(
            "custom plugins cannot replace a built-in AI engine".to_string(),
        )));
    }
    let placeholder = manifest
        .credentials
        .first()
        .map(|_| "validation-placeholder".to_string());
    let provider = match compile_provider(request.plugin_config.clone(), &manifest.id, placeholder)
    {
        Ok(provider) => provider,
        Err(error) => return Ok(Json(invalid_validation(error.to_string()))),
    };
    Ok(Json(AIEnginePluginValidationResponse {
        valid: true,
        engine_key: Some(manifest.id),
        version: Some(manifest.version),
        name: Some(manifest.name),
        destination: Some(
            request
                .plugin_config
                .base_url_override
                .unwrap_or(manifest.base_url),
        ),
        capabilities: Some(
            serde_json::to_value(&provider.descriptor().capabilities)
                .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?,
        ),
        credential_required: manifest
            .credentials
            .first()
            .is_some_and(|credential| credential.required),
        models: manifest
            .models
            .iter()
            .map(|model| PluginModel {
                key: model.id.clone(),
                name: model.name.clone(),
            })
            .collect(),
        error: None,
    }))
}

#[utoipa::path(
    post,
    path = "/admin/ai-engines",
    tag = "admin",
    request_body = AIEngineCreate,
    responses(
        (status = 201, body = AIEngineDetail),
        (status = 400, body = Error),
        (status = 401, body = Error),
        (status = 403, body = Error),
        (status = 409, body = Error)
    )
)]
pub async fn create_ai_engine(
    claims: Claims,
    State(state): State<SharedState>,
    Json(request): Json<AIEngineCreate>,
) -> Result<(StatusCode, Json<AIEngineDetail>), AuthError> {
    ensure_manage(&claims, &state).await?;
    let engine = create_engine(&state, request).await?;
    Ok((StatusCode::CREATED, Json(engine)))
}

#[utoipa::path(
    delete,
    path = "/admin/ai-engines/{engine_key}",
    tag = "admin",
    params(("engine_key" = String, Path, description = "Custom AI engine key")),
    responses(
        (status = 204),
        (status = 400, body = Error),
        (status = 401, body = Error),
        (status = 403, body = Error),
        (status = 404, body = Error)
    )
)]
pub async fn delete_ai_engine(
    claims: Claims,
    State(state): State<SharedState>,
    Path(engine_key): Path<String>,
) -> Result<StatusCode, AuthError> {
    ensure_manage(&claims, &state).await?;
    let engine = find_engine(&state, &engine_key).await?;
    if engine.plugin_config.is_none() {
        return Err(AuthError::InvalidRequest {
            field: "engine_key",
        });
    }
    ai_engines::Entity::delete_by_id(engine.id)
        .exec(&state.database)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    unregister_provider(&state, &engine_key).await;
    state.live_models_cache.invalidate(&engine_key).await;
    state
        .settings
        .remove_ai_engine_from_state(&engine_key)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/admin/ai-engines/{engine_key}/test",
    tag = "admin",
    params(("engine_key" = String, Path, description = "AI engine key")),
    responses(
        (status = 200, body = AIEngineConnectionTest),
        (status = 401, body = Error),
        (status = 403, body = Error),
        (status = 404, body = Error)
    )
)]
pub async fn test_ai_engine_connection(
    claims: Claims,
    State(state): State<SharedState>,
    Path(engine_key): Path<String>,
) -> Result<Json<AIEngineConnectionTest>, AuthError> {
    ensure_manage(&claims, &state).await?;
    let engine = find_engine(&state, &engine_key).await?;
    Ok(Json(test_engine_connection(&state, engine).await?))
}
