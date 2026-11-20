// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use axum::{
    Json,
    extract::{Path, State},
    http::{StatusCode, header},
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
        AIEnginePluginValidationResponse, GrenginCatalog, GrenginStatus, PluginModel,
    },
    models::ai_engines,
    services::{
        ai_plugin::{
            RESERVED_ENGINE_KEYS, create_engine, ensure_manage, ensure_view, find_engine,
            invalid_validation, is_catalog_grengin, test_engine_connection,
        },
        grengin_consent::{
            catalog as grengin_catalog, delete_engine as delete_grengin_engine,
            status as grengin_status,
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
    description = "Install one discovery engine using engine_key, or one custom engine using plugin_config (exactly one). A first API key enables the engine unless is_enabled is explicit. Omitted whitelisted_models selects at most three recent generators; explicit lists, including [], are preserved. Catalog manifests are not stored in the database. A catalog install may set base_url_override (https, public host) to its own endpoint, such as an Azure OpenAI resource or a Bedrock region; PUT /admin/ai-engines/{engine_key} changes or clears it (empty string).",
    request_body = AIEngineCreate,
    responses(
        (status = 201, body = AIEngineDetail),
        (status = 202, body = AIEngineDetail, description = "Grengin installed; enabling waits for service confirmation (pending_operation)"),
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
    let status = if engine.pending_operation.is_some() {
        StatusCode::ACCEPTED
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(engine)))
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
    if is_catalog_grengin(&engine) {
        delete_grengin_engine(&state, engine.id).await?;
    } else {
        ai_engines::Entity::delete_by_id(engine.id)
            .exec(&state.database)
            .await
            .map_err(|_| AuthError::DbTimeout)?;
    }
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

#[utoipa::path(
    get,
    path = "/admin/ai-engines/grengin/status",
    tag = "admin",
    description = "Local and service status of the Grengin engine: whether it is installed and locally enabled, the workspace connection, model access and balance, and any enable/disable still waiting for service confirmation. Works without an identity or local engine; unknown values are null, never zero.",
    responses(
        (status = 200, body = GrenginStatus),
        (status = 401, body = Error),
        (status = 403, body = Error)
    )
)]
pub async fn get_grengin_status(
    claims: Claims,
    State(state): State<SharedState>,
) -> Result<([(header::HeaderName, &'static str); 1], Json<GrenginStatus>), AuthError> {
    ensure_view(&claims, &state).await?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(grengin_status(&state).await?),
    ))
}

#[utoipa::path(
    get,
    path = "/admin/ai-engines/grengin/catalog",
    tag = "admin",
    description = "Models the Grengin service offers now, with this workspace's policy: is_allowed, is_default, and which engine would fund each one (an enabled own-key engine that allows the same model takes precedence). Prices are Grengin tariffs in microdollars per million tokens. No generation is billed.",
    responses(
        (status = 200, body = GrenginCatalog),
        (status = 401, body = Error),
        (status = 403, body = Error),
        (status = 503, body = Error)
    )
)]
pub async fn get_grengin_catalog(
    claims: Claims,
    State(state): State<SharedState>,
) -> Result<
    (
        [(header::HeaderName, &'static str); 1],
        Json<GrenginCatalog>,
    ),
    AuthError,
> {
    ensure_view(&claims, &state).await?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(grengin_catalog(&state).await?),
    ))
}
