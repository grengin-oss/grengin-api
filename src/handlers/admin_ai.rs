// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::{
        claims::Claims,
        encryption::{decrypt_key, encrypt_key},
        error::{AuthError, Error},
        permissions::{PERMISSION_AI_PLATFORM_MANAGE, PERMISSION_AI_PLATFORM_VIEW},
    },
    dto::{
        admin_ai::{
            AIEngineDetail, AIEngineModels, AIEngineUpdate, AIEngineValidation, AiModel,
            AiModelCapabilities,
        },
        models::{ModelInfo, ModelType},
    },
    models::ai_engines::{self, ApiKeyStatus},
    services::ai_engine_helpers::{load_models_response, load_models_response_refreshed},
    services::{
        authorization::{AuthorizationService, PermissionScopeMode},
        provider_models::to_model_info,
        provider_runtime::{
            ProviderLoadError, build_provider, compile_provider, parse_manifest,
            parse_plugin_config, provider_plugin_version, unregister_provider,
        },
    },
    state::SharedState,
};
use axum::{
    Json,
    extract::{Path, State},
};
use chrono::Utc;
use llm_plugin::{ProviderError, ProviderPlugin};
use reqwest::StatusCode;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, TryIntoModel,
};
use std::{collections::HashSet, sync::Arc};
use uuid::Uuid;

#[utoipa::path(
    get,
    path = "/admin/ai-engines",
    tag = "admin",
    responses(
       (status = 200, body = Vec<AIEngineDetail>),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),
    )
)]
pub async fn get_ai_engines(
    claims: Claims,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<Vec<AIEngineDetail>>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AI_PLATFORM_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;
    let ai_models = load_models_response_refreshed(&app_state).await?;
    let selector = ai_engines::Entity::find();
    let mut ai_engines = selector
        .order_by_desc(ai_engines::Column::CreatedAt)
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error get all {e}");
            AuthError::DbTimeout
        })?;
    let mut existing_keys: HashSet<String> = ai_engines
        .iter()
        .map(|engine| engine.engine_key.clone())
        .collect();
    let mut to_insert: Vec<(ai_engines::ActiveModel, Option<String>, Vec<String>)> = Vec::new();
    for provider in &ai_models.providers {
        if existing_keys.contains(&provider.key) {
            continue;
        }
        let api_key = app_state
            .settings
            .get_ai_engine_api_key(&provider.key)
            .await;
        let api_key_encrypted = api_key.clone().map(|k| {
            encrypt_key(&app_state.settings.auth.app_key, k.as_bytes())
                .expect("Failed to encrypt the api key")
        });
        let whitelist_models = provider
            .models
            .iter()
            .map(|model| model.key.clone())
            .collect::<Vec<String>>();
        to_insert.push((
            ai_engines::ActiveModel {
                id: Set(Uuid::new_v4()),
                display_name: Set(provider.name.clone()),
                is_enabled: Set(api_key_encrypted.is_some()),
                engine_key: Set(provider.key.clone()),
                api_key_status: Set(if api_key_encrypted.is_some() {
                    ApiKeyStatus::NotValidated
                } else {
                    ApiKeyStatus::NotConfigured
                }),
                api_key: Set(api_key_encrypted.clone()),
                whitelist_models: Set(whitelist_models.clone()),
                default_model: Set(String::from("<empty>")),
                default_image_gen_model: Set(None),
                plugin_config: Set(None),
                api_key_validated_at: Set(None),
                created_at: Set(Utc::now()),
                updated_at: Set(Utc::now()),
            },
            api_key,
            whitelist_models,
        ));
        existing_keys.insert(provider.key.clone());
    }

    if !to_insert.is_empty() {
        ai_engines::Entity::insert_many(to_insert.iter().map(|(model, _, _)| model.clone()))
            .exec(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("db insert many error {:?}", e);
                AuthError::DbTimeout
            })?;
        for (active_model, api_key, whitelist_models) in to_insert {
            let model = active_model.try_into_model().unwrap();
            let engine_key = model.engine_key.clone();
            let is_enabled = model.is_enabled.clone();
            let _ = app_state
                .settings
                .load_ai_engine_in_state(engine_key, api_key, is_enabled, whitelist_models)
                .await;
        }
        ai_engines = ai_engines::Entity::find()
            .order_by_desc(ai_engines::Column::CreatedAt)
            .all(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("db error get all {e}");
                AuthError::DbTimeout
            })?;
    }

    let response = ai_engines
        .into_iter()
        .map(|model| {
            let plugin_config = model
                .plugin_config
                .as_ref()
                .and_then(|value| serde_json::from_value(value.clone()).ok());
            AIEngineDetail {
                icon: ai_models.get_icons(&model.engine_key).0,
                icon_dark: ai_models.get_icons(&model.engine_key).1,
                plugin_version: provider_plugin_version(&model),
                engine_key: model.engine_key,
                display_name: model.display_name,
                is_enabled: model.is_enabled,
                api_key_configured: model.api_key.is_some(),
                api_key_status: model.api_key_status,
                api_key_preview: app_state.get_decrypted_api_key_preview(&model.api_key),
                api_key_last_validated_at: model.api_key_validated_at,
                whitelisted_models: model.whitelist_models,
                default_model: Some(model.default_model),
                default_image_gen_model: model.default_image_gen_model,
                plugin_config,
                created_at: model.created_at,
                updated_at: model.updated_at,
            }
        })
        .collect();
    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    get,
    path = "/admin/ai-engines/{engine_key}",
    tag = "admin",
    params(
        ("engine_key" = String, Path, description = "Engine key example 'openai','anthropic'")
    ),
    responses(
       (status = 200, body = AIEngineDetail),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 404, content_type = "application/json", body = Error, description = "Ai Engine not found (code=5003)"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),
    )
)]
pub async fn get_ai_engines_by_key(
    claims: Claims,
    Path(ai_engine_key): Path<String>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<AIEngineDetail>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AI_PLATFORM_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;
    let ai_models = load_models_response(&app_state).await?;
    let model = ai_engines::Entity::find()
        .filter(ai_engines::Column::EngineKey.eq(ai_engine_key))
        .order_by_desc(ai_engines::Column::CreatedAt)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error get all {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;
    let plugin_config = model
        .plugin_config
        .as_ref()
        .and_then(|value| serde_json::from_value(value.clone()).ok());
    let response = AIEngineDetail {
        icon: ai_models.get_icons(&model.engine_key).0,
        icon_dark: ai_models.get_icons(&model.engine_key).1,
        plugin_version: provider_plugin_version(&model),
        engine_key: model.engine_key,
        display_name: model.display_name,
        is_enabled: model.is_enabled,
        api_key_configured: model.api_key.is_some(),
        api_key_status: model.api_key_status,
        api_key_preview: app_state.get_decrypted_api_key_preview(&model.api_key),
        api_key_last_validated_at: model.api_key_validated_at,
        whitelisted_models: model.whitelist_models,
        default_model: Some(model.default_model),
        default_image_gen_model: model.default_image_gen_model,
        plugin_config,
        created_at: model.created_at,
        updated_at: model.updated_at,
    };
    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    get,
    path = "/admin/ai-engines/{engine_key}/models",
    tag = "admin",
    params(
        ("engine_key" = String, Path, description = "Engine key example 'openai','anthropic'")
    ),
    responses(
       (status = 200, body = AIEngineModels),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 404, content_type = "application/json", body = Error, description = "Ai Engine not found (code=5003)"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),

    )
)]
pub async fn get_ai_engine_models_by_key(
    claims: Claims,
    Path(ai_engine_key): Path<String>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<AIEngineModels>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AI_PLATFORM_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;
    let ai_engine = ai_engines::Entity::find()
        .filter(ai_engines::Column::EngineKey.eq(ai_engine_key.clone()))
        .order_by_desc(ai_engines::Column::CreatedAt)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error get all {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;
    let mut response = AIEngineModels { models: Vec::new() };
    if ai_engine.plugin_config.is_some() {
        let provider = build_provider(&app_state.settings.auth.app_key, &ai_engine)
            .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
        let model_provider = provider
            .models()
            .ok_or(AuthError::ServiceTemporarilyUnavailable)?;
        let models = model_provider
            .list_models()
            .await
            .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
        response.models = models
            .into_iter()
            .map(|model| {
                let is_whitelisted = ai_engine
                    .whitelist_models
                    .iter()
                    .any(|model_id| model_id == model.id.as_str() || model_id == &model.name);
                ai_model_from_info(to_model_info(&ai_engine.engine_key, model), is_whitelisted)
            })
            .collect();
        return Ok((StatusCode::OK, Json(response)));
    }
    let ai_models = load_models_response(&app_state).await?;
    for provider in ai_models.providers {
        if provider.key != ai_engine_key {
            continue;
        }
        for model in provider.models {
            let is_whitelisted = ai_engine.whitelist_models.contains(&model.key)
                || ai_engine.whitelist_models.contains(&model.name);
            response
                .models
                .push(ai_model_from_info(model, is_whitelisted));
        }
    }
    Ok((StatusCode::OK, Json(response)))
}

fn ai_model_from_info(model: ModelInfo, is_whitelisted: bool) -> AiModel {
    let embeddings = model.model_type == ModelType::TextEmbedder;
    let image_generation = model.model_type == ModelType::ImageGenerator;
    AiModel {
        model_id: model.key,
        display_name: model.name,
        model_type: model.model_type,
        is_whitelisted,
        capabilities: AiModelCapabilities {
            vision: model.supports_vision,
            function_calling: model.supports_tools,
            streaming: model.supports_streaming,
            reasoning: model.supports_reasoning,
            audio: model.supports_audio,
            pdf_native: model.supports_pdf_native,
            web_search: model.supports_web_search,
            multiple_images: model.supports_multiple_images,
            embeddings,
            image_generation,
        },
        comment: model.comment,
        input_token_rate: model.input_token_rate,
        output_token_rate: model.output_token_rate,
        image_input_token_rate: model.image_input_token_rate,
        image_cached_input_token_rate: model.image_cached_input_token_rate,
        image_output_token_rate: model.image_output_token_rate,
        cached_input_token_rate: model.cached_input_token_rate,
        cache_creation_token_rate: model.cache_creation_token_rate,
        max_input_tokens: model.max_input_tokens,
        max_output_tokens: model.max_output_tokens,
        max_images: model.max_images,
        dimensions: model.dimensions,
        price_per_image: model.price_per_image,
    }
}

#[utoipa::path(
    put,
    path = "/admin/ai-engines/{engine_key}",
    tag = "admin",
    params(
        ("engine_key" = String, Path, description = "Engine key example 'openai','anthropic'")
    ),
    responses(
        (status = 200, body = AIEngineDetail),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
        (status = 404, content_type = "application/json", body = Error, description = "Ai Engine not found (code=5003)"),
        (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),

    )
)]
pub async fn update_ai_engines_by_key(
    claims: Claims,
    Path(ai_engine_key): Path<String>,
    State(app_state): State<SharedState>,
    Json(req): Json<AIEngineUpdate>,
) -> Result<(StatusCode, Json<AIEngineDetail>), AuthError> {
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
    let ai_models = load_models_response(&app_state).await?;
    let ai_engine = ai_engines::Entity::find()
        .filter(ai_engines::Column::EngineKey.eq(ai_engine_key.clone()))
        .order_by_desc(ai_engines::Column::CreatedAt)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error get all {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;
    let mut active_model = ai_engine.clone().into_active_model();
    if let Some(display_name) = req.display_name {
        active_model.display_name = Set(display_name);
    }
    if let Some(api_key) = req.api_key {
        let encrypted_api_key = encrypt_key(&app_state.settings.auth.app_key, api_key.as_bytes())
            .map_err(|e| {
            eprintln!("Encryption error for api key: {:?}", e);
            AuthError::DbTimeout
        })?;
        active_model.api_key = Set(Some(encrypted_api_key));
        active_model.api_key_status = Set(ApiKeyStatus::NotValidated)
    }
    active_model.updated_at = Set(Utc::now());
    if let Some(default_model) = req.default_model {
        active_model.default_model = Set(default_model);
    }
    if let Some(default_image_gen_model) = req.default_image_gen_model {
        active_model.default_image_gen_model = Set(Some(default_image_gen_model));
    }
    if let Some(plugin_config) = req.plugin_config {
        if ai_engine.plugin_config.is_none() {
            return Err(AuthError::InvalidRequest {
                field: "plugin_config",
            });
        }
        let manifest = parse_manifest(&plugin_config).map_err(|_| AuthError::InvalidRequest {
            field: "plugin_config",
        })?;
        if manifest.id != ai_engine_key {
            return Err(AuthError::InvalidRequest {
                field: "plugin_config.manifest.id",
            });
        }
        active_model.plugin_config =
            Set(Some(serde_json::to_value(plugin_config).map_err(|_| {
                AuthError::InvalidRequest {
                    field: "plugin_config",
                }
            })?));
    }
    if let Some(whitelist_models) = req.whitelisted_models {
        active_model.whitelist_models = Set(whitelist_models);
    }
    if let Some(is_enabled) = req.is_enabled {
        active_model.is_enabled = Set(is_enabled);
    }
    let model = active_model.clone().try_into_model().map_err(|e| {
        eprintln!("db error model parse error {e}");
        AuthError::DbTimeout
    })?;
    let decrypted_api_key = match &model.api_key {
        Some(api_key) => Some(
            decrypt_key(&app_state.settings.auth.app_key, api_key).map_err(|e| {
                eprintln!("Decryption api key error {:?}", e);
                AuthError::DbTimeout
            })?,
        ),
        None => None,
    };
    let compiled_provider = if model.is_enabled {
        Some(
            build_provider(&app_state.settings.auth.app_key, &model).map_err(|_| {
                AuthError::InvalidRequest {
                    field: "plugin_config",
                }
            })?,
        )
    } else if let Some(config_value) = model.plugin_config.as_ref() {
        let config = parse_plugin_config(config_value).map_err(|_| AuthError::InvalidRequest {
            field: "plugin_config",
        })?;
        let manifest = parse_manifest(&config).map_err(|_| AuthError::InvalidRequest {
            field: "plugin_config",
        })?;
        let compile_key = decrypted_api_key.clone().or_else(|| {
            (!model.is_enabled)
                .then(|| manifest.credentials.first())
                .flatten()
                .map(|_| "validation-placeholder".to_string())
        });
        Some(
            compile_provider(config, &model.engine_key, compile_key).map_err(|_| {
                AuthError::InvalidRequest {
                    field: "plugin_config",
                }
            })?,
        )
    } else {
        None
    };
    active_model
        .update(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error update one {e}");
            AuthError::DbTimeout
        })?;
    app_state
        .settings
        .load_ai_engine_in_state(
            &ai_engine_key,
            decrypted_api_key,
            model.is_enabled,
            model.whitelist_models.clone(),
        )
        .await
        .map_err(|e| {
            eprintln!("Ai engine loading error in state {e}");
            AuthError::DbTimeout
        })?;
    if model.is_enabled {
        if let Some(provider) = compiled_provider {
            app_state
                .provider_registry
                .register(Arc::new(provider))
                .await;
        }
    } else {
        unregister_provider(&app_state, &model.engine_key).await;
    }
    let plugin_config = model
        .plugin_config
        .as_ref()
        .and_then(|value| serde_json::from_value(value.clone()).ok());
    let response = AIEngineDetail {
        icon: ai_models.get_icons(&model.engine_key).0,
        icon_dark: ai_models.get_icons(&model.engine_key).1,
        plugin_version: provider_plugin_version(&model),
        engine_key: model.engine_key,
        display_name: model.display_name,
        is_enabled: model.is_enabled,
        api_key_configured: model.api_key.is_some(),
        api_key_status: model.api_key_status,
        api_key_preview: app_state.get_decrypted_api_key_preview(&model.api_key),
        api_key_last_validated_at: model.api_key_validated_at,
        whitelisted_models: model.whitelist_models,
        default_model: Some(model.default_model),
        default_image_gen_model: model.default_image_gen_model,
        plugin_config,
        created_at: model.created_at,
        updated_at: model.updated_at,
    };
    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    delete,
    path = "/admin/ai-engines/{engine_key}/api-key",
    tag = "admin",
    params(
        ("engine_key" = String, Path, description = "Engine key example 'openai','anthropic'")
    ),
    responses(
       (status = 200, body = AIEngineDetail),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 404, content_type = "application/json", body = Error, description = "Ai Engine not found (code=5003)"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),
    )
)]
pub async fn delete_ai_engines_api_key_key(
    claims: Claims,
    Path(ai_engine_key): Path<String>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<AIEngineDetail>), AuthError> {
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
    let ai_models = load_models_response(&app_state).await?;
    let ai_engine = ai_engines::Entity::find()
        .filter(ai_engines::Column::EngineKey.eq(ai_engine_key))
        .order_by_desc(ai_engines::Column::CreatedAt)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error get all {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;
    let mut active_model = ai_engine.clone().into_active_model();
    active_model.api_key = Set(None);
    active_model.updated_at = Set(Utc::now());
    active_model.api_key_status = Set(ApiKeyStatus::NotConfigured);
    active_model.is_enabled = Set(false);
    app_state
        .settings
        .load_ai_engine_in_state(
            &ai_engine.engine_key,
            None,
            false,
            ai_engine.whitelist_models.clone(),
        )
        .await
        .map_err(|e| {
            eprintln!("Ai engine removal from state failed: {e}");
            AuthError::DbTimeout
        })?;
    unregister_provider(&app_state, &ai_engine.engine_key).await;
    active_model
        .clone()
        .update(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error update one {e}");
            AuthError::DbTimeout
        })?;
    let model = active_model.try_into_model().map_err(|e| {
        eprintln!("db error model parse error {e}");
        AuthError::DbTimeout
    })?;
    let plugin_config = model
        .plugin_config
        .as_ref()
        .and_then(|value| serde_json::from_value(value.clone()).ok());
    let response = AIEngineDetail {
        icon: ai_models.get_icons(&model.engine_key).0,
        icon_dark: ai_models.get_icons(&model.engine_key).1,
        plugin_version: provider_plugin_version(&model),
        engine_key: model.engine_key,
        display_name: model.display_name,
        is_enabled: model.is_enabled,
        api_key_configured: model.api_key.is_some(),
        api_key_status: model.api_key_status,
        api_key_preview: app_state.get_decrypted_api_key_preview(&model.api_key),
        api_key_last_validated_at: model.api_key_validated_at,
        whitelisted_models: model.whitelist_models,
        default_model: Some(model.default_model),
        default_image_gen_model: model.default_image_gen_model,
        plugin_config,
        created_at: model.created_at,
        updated_at: model.updated_at,
    };
    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    post,
    path = "/admin/ai-engines/{engine_key}/validate",
    tag = "admin",
    params(
        ("engine_key" = String, Path, description = "Engine key example 'openai','anthropic'")
    ),
    responses(
        (status = 200, body = AIEngineValidation),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
    )
)]
pub async fn validate_ai_engines_by_key(
    claims: Claims,
    Path(ai_engine_key): Path<String>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<AIEngineValidation>), AuthError> {
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
    let ai_engine = ai_engines::Entity::find()
        .filter(ai_engines::Column::EngineKey.eq(ai_engine_key.clone()))
        .order_by_desc(ai_engines::Column::CreatedAt)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error get all {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;
    let validation = match build_provider(&app_state.settings.auth.app_key, &ai_engine) {
        Ok(provider) => match provider.models() {
            Some(models) => match models.list_models().await {
                Ok(models) => (ApiKeyStatus::Valid, models.len() as i64),
                Err(ProviderError::QuotaExhausted)
                | Err(ProviderError::HttpStatus { status: 429, .. }) => (ApiKeyStatus::Valid, 0),
                Err(ProviderError::MissingCredential(_)) => (ApiKeyStatus::NotConfigured, 0),
                Err(ProviderError::HttpStatus {
                    status: 401 | 403, ..
                }) => (ApiKeyStatus::Invalid, 0),
                Err(_) => (ApiKeyStatus::NotValidated, 0),
            },
            None => (ApiKeyStatus::NotValidated, 0),
        },
        Err(ProviderLoadError::CredentialDecryption) => return Err(AuthError::DbTimeout),
        Err(_) => (ApiKeyStatus::NotValidated, 0),
    };
    let (api_key_status, models_available) = validation;
    let mut active_model = ai_engine.clone().into_active_model();
    active_model.api_key_status = Set(api_key_status.clone());
    active_model.updated_at = Set(Utc::now());
    active_model.api_key_validated_at = Set(Some(Utc::now()));
    active_model
        .clone()
        .update(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error update one {e}");
            AuthError::DbTimeout
        })?;
    let (valid, message) = match api_key_status {
        ApiKeyStatus::Valid => (true, "API key validated successfully".to_string()),
        ApiKeyStatus::Invalid => (false, format!("API key is incorrect for {ai_engine_key}.")),
        ApiKeyStatus::NotConfigured => (
            false,
            format!("API key is not configured for {ai_engine_key}."),
        ),
        ApiKeyStatus::NotValidated => (
            false,
            format!("API key could not be validated for {ai_engine_key} right now."),
        ),
    };
    let response = AIEngineValidation {
        valid,
        message,
        models_available,
    };
    Ok((StatusCode::OK, Json(response)))
}
