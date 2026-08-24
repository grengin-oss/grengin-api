// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use chrono::Utc;
use llm_plugin::ProviderPlugin;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter,
};
use uuid::Uuid;

use crate::{
    auth::{
        claims::Claims,
        encryption::encrypt_key,
        error::AuthError,
        permissions::{PERMISSION_AI_PLATFORM_MANAGE, PERMISSION_AI_PLATFORM_VIEW},
    },
    dto::admin_ai::{
        AIEngineConnectionTest, AIEngineCreate, AIEngineDetail, AIEnginePluginValidationResponse,
    },
    models::ai_engines::{self, ApiKeyStatus},
    services::{
        authorization::{AuthorizationService, PermissionScopeMode},
        provider_chat::provider_error_class,
        provider_runtime::{
            ProviderLoadError, build_provider, compile_provider, parse_manifest,
            provider_plugin_version,
        },
    },
    state::SharedState,
};

pub const RESERVED_ENGINE_KEYS: &[&str] = &["openai", "anthropic", "mistral", "gemini"];

pub async fn find_engine(
    state: &SharedState,
    engine_key: &str,
) -> Result<ai_engines::Model, AuthError> {
    ai_engines::Entity::find()
        .filter(ai_engines::Column::EngineKey.eq(engine_key))
        .one(&state.database)
        .await
        .map_err(|_| AuthError::DbTimeout)?
        .ok_or(AuthError::ResourceNotFound)
}

async fn update_validation(
    state: &SharedState,
    engine: ai_engines::Model,
    valid: bool,
) -> Result<(), AuthError> {
    let mut active = engine.into_active_model();
    active.api_key_status = Set(if valid {
        ApiKeyStatus::Valid
    } else {
        ApiKeyStatus::Invalid
    });
    active.api_key_validated_at = Set(Some(Utc::now()));
    active.updated_at = Set(Utc::now());
    active
        .update(&state.database)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    Ok(())
}

fn engine_detail(state: &SharedState, engine: ai_engines::Model) -> AIEngineDetail {
    let plugin_config = engine
        .plugin_config
        .as_ref()
        .and_then(|value| serde_json::from_value(value.clone()).ok());
    AIEngineDetail {
        icon: None,
        icon_dark: None,
        plugin_version: provider_plugin_version(&engine),
        engine_key: engine.engine_key,
        display_name: engine.display_name,
        is_enabled: engine.is_enabled,
        api_key_configured: engine.api_key.is_some(),
        api_key_status: engine.api_key_status,
        api_key_preview: state.get_decrypted_api_key_preview(&engine.api_key),
        api_key_last_validated_at: engine.api_key_validated_at,
        whitelisted_models: engine.whitelist_models,
        default_model: Some(engine.default_model),
        default_image_gen_model: engine.default_image_gen_model,
        plugin_config,
        created_at: engine.created_at,
        updated_at: engine.updated_at,
    }
}

pub fn invalid_validation(error: String) -> AIEnginePluginValidationResponse {
    AIEnginePluginValidationResponse {
        valid: false,
        engine_key: None,
        version: None,
        name: None,
        destination: None,
        capabilities: None,
        credential_required: false,
        models: Vec::new(),
        error: Some(error),
    }
}

fn load_error_class(error: &ProviderLoadError) -> &'static str {
    match error {
        ProviderLoadError::Database => "database",
        ProviderLoadError::CredentialDecryption => "credential_decryption",
        ProviderLoadError::ManifestUnavailable => "manifest_unavailable",
        ProviderLoadError::Provider(error) => provider_error_class(error),
    }
}

pub async fn ensure_view(claims: &Claims, state: &SharedState) -> Result<(), AuthError> {
    AuthorizationService::new(&state.database)
        .ensure_permission(
            claims.user_id,
            PERMISSION_AI_PLATFORM_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await
}

pub async fn ensure_manage(claims: &Claims, state: &SharedState) -> Result<(), AuthError> {
    AuthorizationService::new(&state.database)
        .ensure_permission(
            claims.user_id,
            PERMISSION_AI_PLATFORM_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await
}

pub async fn test_engine_connection(
    state: &SharedState,
    engine: ai_engines::Model,
) -> Result<AIEngineConnectionTest, AuthError> {
    let provider =
        match build_provider(&state.settings.auth.app_key, &state.req_client, &engine).await {
            Ok(provider) => provider,
            Err(error) => {
                update_validation(state, engine, false).await?;
                return Ok(AIEngineConnectionTest {
                    valid: false,
                    mode: "configuration".to_string(),
                    models_available: None,
                    error_class: Some(load_error_class(&error).to_string()),
                });
            }
        };
    // A provider with no model-listing operation cannot be probed, so a compiled
    // manifest is the strongest signal available for it.
    let (valid, mode, models_available, error_class) = match provider.models() {
        Some(models) => match models.list_models().await {
            Ok(models) => (true, "model_list", Some(models.len()), None),
            Err(error) => (
                false,
                "model_list",
                None,
                Some(provider_error_class(&error)),
            ),
        },
        None => (true, "configuration", None, None),
    };
    update_validation(state, engine, valid).await?;
    Ok(AIEngineConnectionTest {
        valid,
        mode: mode.to_string(),
        models_available,
        error_class: error_class.map(str::to_string),
    })
}

pub async fn create_engine(
    state: &SharedState,
    request: AIEngineCreate,
) -> Result<AIEngineDetail, AuthError> {
    let manifest =
        parse_manifest(&request.plugin_config).map_err(|_| AuthError::InvalidRequest {
            field: "plugin_config",
        })?;
    if RESERVED_ENGINE_KEYS.contains(&manifest.id.as_str()) {
        return Err(AuthError::InvalidRequest {
            field: "plugin_config.manifest.id",
        });
    }
    if ai_engines::Entity::find()
        .filter(ai_engines::Column::EngineKey.eq(&manifest.id))
        .one(&state.database)
        .await
        .map_err(|_| AuthError::DbTimeout)?
        .is_some()
    {
        return Err(AuthError::DbConflict);
    }

    let api_key = request
        .api_key
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if manifest.credentials.is_empty() && api_key.is_some() {
        return Err(AuthError::InvalidRequest { field: "api_key" });
    }
    if request.is_enabled
        && manifest
            .credentials
            .first()
            .is_some_and(|credential| credential.required)
        && api_key.is_none()
    {
        return Err(AuthError::InvalidRequest { field: "api_key" });
    }
    let compile_key = api_key.clone().or_else(|| {
        manifest
            .credentials
            .first()
            .map(|_| "validation-placeholder".to_string())
    });
    let provider = compile_provider(request.plugin_config.clone(), &manifest.id, compile_key)
        .map_err(|_| AuthError::InvalidRequest {
            field: "plugin_config",
        })?;

    let now = Utc::now();
    let whitelist_models = if request.whitelisted_models.is_empty() {
        manifest
            .models
            .iter()
            .map(|model| model.id.clone())
            .collect()
    } else {
        request.whitelisted_models
    };
    let default_model = request
        .default_model
        .or_else(|| whitelist_models.first().cloned())
        .unwrap_or_else(|| "<empty>".to_string());
    let encrypted_api_key = api_key
        .as_ref()
        .map(|value| encrypt_key(&state.settings.auth.app_key, value.as_bytes()))
        .transpose()
        .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    let plugin_config =
        serde_json::to_value(&request.plugin_config).map_err(|_| AuthError::InvalidRequest {
            field: "plugin_config",
        })?;
    let engine = ai_engines::ActiveModel {
        id: Set(Uuid::new_v4()),
        display_name: Set(request
            .display_name
            .unwrap_or_else(|| manifest.name.clone())),
        is_enabled: Set(request.is_enabled),
        engine_key: Set(manifest.id.clone()),
        api_key_status: Set(if encrypted_api_key.is_some() {
            ApiKeyStatus::NotValidated
        } else {
            ApiKeyStatus::NotConfigured
        }),
        api_key: Set(encrypted_api_key),
        whitelist_models: Set(whitelist_models.clone()),
        default_model: Set(default_model),
        default_image_gen_model: Set(request.default_image_gen_model),
        plugin_config: Set(Some(plugin_config)),
        api_key_validated_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&state.database)
    .await
    .map_err(|_| AuthError::DbTimeout)?;

    state
        .settings
        .load_ai_engine_in_state(
            &engine.engine_key,
            api_key,
            engine.is_enabled,
            whitelist_models,
        )
        .await
        .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    if engine.is_enabled {
        state.provider_registry.register(Arc::new(provider)).await;
    }
    Ok(engine_detail(state, engine))
}
