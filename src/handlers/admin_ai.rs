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
        admin_ai::{AIEngineDetail, AIEngineModels, AIEngineUpdate, AIEngineValidation, AiModel, AiModelCapabilities},
        models::ModelType,
    },
    services::ai_engine_helpers::{
        load_models_response, load_models_response_refreshed, normalize_bearer_token,
    },
    llm::{
        gemini::GEMINI_API_URL,
        mistral::MISTRAL_API_URL,
        provider::{AnthropicApis, OpenaiApis},
    },
    models::ai_engines::{self, ApiKeyStatus},
    services::authorization::{AuthorizationService, PermissionScopeMode},
    state::SharedState,
};
use axum::{
    Json,
    extract::{Path, State},
};
use chrono::Utc;
use reqwest::StatusCode;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, TryIntoModel,
};
use std::collections::HashSet;
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
        .map(|model| AIEngineDetail {
            icon: ai_models.get_icons(&model.engine_key).0,
            icon_dark: ai_models.get_icons(&model.engine_key).1,
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
            created_at: model.created_at,
            updated_at: model.updated_at,
        })
        .collect();
    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    get,
    path = "/admin/ai-engines/{ai_engine_key}",
    tag = "admin",
    params(
        ("ai_engine_key" = String, Path, description = "Engine key example 'openai','anthropic'")
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
    let response = AIEngineDetail {
        icon: ai_models.get_icons(&model.engine_key).0,
        icon_dark: ai_models.get_icons(&model.engine_key).1,
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
        created_at: model.created_at,
        updated_at: model.updated_at,
    };
    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    get,
    path = "/admin/ai-engines/{ai_engine_key}/models",
    tag = "admin",
    params(
        ("ai_engine_key" = String, Path, description = "Engine key example 'openai','anthropic'")
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
    let ai_models = load_models_response(&app_state).await?;
    for provider in ai_models.providers {
        if provider.key != ai_engine_key {
            continue;
        }
        for model in provider.models.into_iter().filter(|m| m.model_type != ModelType::TextEmbedder) {
            response.models.push(AiModel {
                model_id: model.key.clone(),
                display_name: model.name.clone(),
                is_whitelisted: ai_engine.whitelist_models.contains(&model.key),
                capabilities: AiModelCapabilities {
                    vision: model.supports_vision,
                    function_calling: model.supports_tools,
                    streaming: model.supports_streaming,
                },
            })
        }
    }
    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    put,
    path = "/admin/ai-engines/{ai_engine_key}",
    tag = "admin",
    params(
        ("ai_engine_key" = String, Path, description = "Engine key example 'openai','anthropic'")
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
    if let Some(whitelist_models) = req.whitelisted_models {
        active_model.whitelist_models = Set(whitelist_models);
    }
    if let Some(is_enabled) = req.is_enabled {
        active_model.is_enabled = Set(is_enabled);
    }
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
    let decrypted_api_key = match &model.api_key {
        Some(api_key) => Some(
            decrypt_key(&app_state.settings.auth.app_key, api_key).map_err(|e| {
                eprintln!("Decryption api key error {:?}", e);
                AuthError::DbTimeout
            })?,
        ),
        None => None,
    };
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
    let _ = validate_ai_engines_by_key(claims, Path(ai_engine_key), State(app_state.clone()));
    let response = AIEngineDetail {
        icon: ai_models.get_icons(&model.engine_key).0,
        icon_dark: ai_models.get_icons(&model.engine_key).1,
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
        created_at: model.created_at,
        updated_at: model.updated_at,
    };
    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    delete,
    path = "/admin/ai-engines/{ai_engine_key}/api-key",
    tag = "admin",
    params(
        ("ai_engine_key" = String, Path, description = "Engine key example 'openai','anthropic'")
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
    let _ = app_state.settings.load_ai_engine_in_state(
        ai_engine.engine_key,
        None,
        false,
        ai_engine.whitelist_models.clone(),
    );
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
    let response = AIEngineDetail {
        icon: ai_models.get_icons(&model.engine_key).0,
        icon_dark: ai_models.get_icons(&model.engine_key).1,
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
        created_at: model.created_at,
        updated_at: model.updated_at,
    };
    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    post,
    path = "/admin/ai-engines/{ai_engine_key}/validate",
    tag = "admin",
    params(
        ("ai_engine_key" = String, Path, description = "Engine key example 'openai','anthropic'")
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
    let api_key_status = match ai_engine_key.as_ref() {
        "openai" => {
            let openai_settings = &app_state
                .settings
                .openai
                .read()
                .await
                .clone()
                .ok_or(AuthError::ResourceNotFound)?;
            let models = app_state
                .req_client
                .openai_list_models(openai_settings)
                .await;
            if models.is_ok() {
                ApiKeyStatus::Valid
            } else {
                ApiKeyStatus::Invalid
            }
        }
        "anthropic" => {
            let anthropic_settings = &app_state
                .settings
                .anthropic
                .write()
                .await
                .clone()
                .ok_or(AuthError::ResourceNotFound)?;
            let models = app_state
                .req_client
                .anthropic_get_models(anthropic_settings)
                .await;
            if models.is_ok() {
                ApiKeyStatus::Valid
            } else {
                ApiKeyStatus::Invalid
            }
        }
        "mistral" => {
            // Validate against the key stored in DB (not in-memory state) to avoid cache drift.
            let ai_engine = ai_engines::Entity::find()
                .filter(ai_engines::Column::EngineKey.eq(ai_engine_key.clone()))
                .order_by_desc(ai_engines::Column::CreatedAt)
                .one(&app_state.database)
                .await
                .map_err(|e| {
                    eprintln!("db error get engine for validation {e}");
                    AuthError::DbTimeout
                })?
                .ok_or(AuthError::ResourceNotFound)?;

            match ai_engine.api_key.as_ref() {
                None => ApiKeyStatus::NotConfigured,
                Some(encrypted_api_key) => {
                    let api_key = decrypt_key(&app_state.settings.auth.app_key, encrypted_api_key)
                        .map_err(|e| {
                            eprintln!("mistral api key decrypt error: {e:?}");
                            AuthError::DbTimeout
                        })?;
                    let api_key = normalize_bearer_token(&api_key);
                    if api_key.is_empty() {
                        ApiKeyStatus::NotConfigured
                    } else {
                        let response = app_state
                            .req_client
                            .get(format!("{MISTRAL_API_URL}/v1/models"))
                            .bearer_auth(api_key)
                            .send()
                            .await;
                        match response {
                            Ok(resp) if resp.status().is_success() => ApiKeyStatus::Valid,
                            // A 429 still implies the key was accepted/authenticated.
                            Ok(resp) if resp.status() == StatusCode::TOO_MANY_REQUESTS => {
                                ApiKeyStatus::Valid
                            }
                            Ok(resp)
                                if resp.status() == StatusCode::UNAUTHORIZED
                                    || resp.status() == StatusCode::FORBIDDEN =>
                            {
                                ApiKeyStatus::Invalid
                            }
                            Ok(_) => ApiKeyStatus::NotValidated,
                            Err(_) => ApiKeyStatus::NotValidated,
                        }
                    }
                }
            }
        }
        "gemini" => {
            let gemini_settings = &app_state
                .settings
                .gemini
                .read()
                .await
                .clone()
                .ok_or(AuthError::ResourceNotFound)?;
            let response = app_state
                .req_client
                .get(format!("{GEMINI_API_URL}/v1beta/models"))
                .header("x-goog-api-key", gemini_settings.api_key.clone())
                .send()
                .await;
            match response {
                Ok(resp) if resp.status().is_success() => ApiKeyStatus::Valid,
                Ok(resp) if resp.status() == StatusCode::TOO_MANY_REQUESTS => ApiKeyStatus::Valid,
                Ok(resp)
                    if resp.status() == StatusCode::UNAUTHORIZED
                        || resp.status() == StatusCode::FORBIDDEN =>
                {
                    ApiKeyStatus::Invalid
                }
                Ok(_) => ApiKeyStatus::NotValidated,
                Err(_) => ApiKeyStatus::NotValidated,
            }
        }
        _ => ApiKeyStatus::NotConfigured,
    };
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
        models_available: ai_engine.whitelist_models.len() as i64,
    };
    Ok((StatusCode::OK, Json(response)))
}
