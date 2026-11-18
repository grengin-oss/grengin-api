// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use chrono::Utc;
use llm_plugin::ProviderPlugin;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait,
    IntoActiveModel, QueryFilter, Statement, TransactionTrait,
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
    models::ai_engines::{self, ApiKeyStatus, PluginConfig},
    services::{
        ai_engine_defaults::{enabled_after_key, starter_models},
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

const BASE_URL_OVERRIDE_FIELD: AuthError = AuthError::InvalidRequest {
    field: "base_url_override",
};

// Catalog engines such as Azure OpenAI (per-resource host) and Bedrock (per-region host) need
// the customer's own endpoint; it gets the same destination rules as a custom plugin URL.
pub fn catalog_base_url_override(raw: &str) -> Result<Option<String>, AuthError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let url = url::Url::parse(raw).map_err(|_| BASE_URL_OVERRIDE_FIELD)?;
    if url.query().is_some() {
        return Err(BASE_URL_OVERRIDE_FIELD);
    }
    llm_plugin::security::validate_provider_url(&url, false, false)
        .map_err(|_| BASE_URL_OVERRIDE_FIELD)?;
    Ok(Some(url.to_string()))
}

// A custom engine keeps its endpoint inside plugin_config, so the column only belongs to catalog engines.
pub fn apply_base_url_override(
    active: &mut ai_engines::ActiveModel,
    engine: &ai_engines::Model,
    requested: Option<&str>,
    plugin_config_replaced: bool,
) -> Result<(), AuthError> {
    match requested {
        Some(_) if plugin_config_replaced || engine.plugin_config.is_some() => {
            Err(BASE_URL_OVERRIDE_FIELD)
        }
        Some(raw) => {
            active.base_url_override = Set(catalog_base_url_override(raw)?);
            Ok(())
        }
        None if plugin_config_replaced => {
            active.base_url_override = Set(None);
            Ok(())
        }
        None => Ok(()),
    }
}

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
        base_url_override: engine.base_url_override,
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
    let (config, catalog_install) = match (request.engine_key.as_deref(), request.plugin_config) {
        (Some(key), None) if !key.trim().is_empty() => {
            let package = state
                .discovery_catalog
                .ai_provider(key, None)
                .await
                .map_err(|error| match error {
                    crate::services::discovery_catalog::DiscoveryError::NotFound => {
                        AuthError::ResourceNotFound
                    }
                    _ => AuthError::ServiceTemporarilyUnavailable,
                })?;
            (
                PluginConfig {
                    manifest: package.plugin,
                    configuration: serde_json::json!({}),
                    base_url_override: request
                        .base_url_override
                        .as_deref()
                        .map(catalog_base_url_override)
                        .transpose()?
                        .flatten(),
                    allow_insecure_http: false,
                    allow_private_network: false,
                },
                true,
            )
        }
        (None, Some(_)) if request.base_url_override.is_some() => {
            return Err(BASE_URL_OVERRIDE_FIELD);
        }
        (None, Some(config)) => (config, false),
        _ => {
            return Err(AuthError::InvalidRequest {
                field: "engine_key/plugin_config",
            });
        }
    };
    let manifest = parse_manifest(&config).map_err(|_| AuthError::InvalidRequest {
        field: "plugin_config",
    })?;
    if !catalog_install && RESERVED_ENGINE_KEYS.contains(&manifest.id.as_str()) {
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

    let api_key = match request.api_key {
        Some(value) if value.trim().is_empty() => {
            return Err(AuthError::InvalidRequest { field: "api_key" });
        }
        Some(value) => Some(value.trim().to_string()),
        None => None,
    };
    let is_enabled = enabled_after_key(false, api_key.is_some(), request.is_enabled);
    if manifest.credentials.is_empty() && api_key.is_some() {
        return Err(AuthError::InvalidRequest { field: "api_key" });
    }
    if is_enabled
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
    let provider = compile_provider(config.clone(), &manifest.id, compile_key).map_err(|_| {
        AuthError::InvalidRequest {
            field: "plugin_config",
        }
    })?;

    let now = Utc::now();
    let whitelist_models = request
        .whitelisted_models
        .unwrap_or_else(|| starter_models(&manifest));
    let default_model = request
        .default_model
        .or_else(|| whitelist_models.first().cloned())
        .unwrap_or_else(|| "<empty>".to_string());
    let encrypted_api_key = api_key
        .as_ref()
        .map(|value| encrypt_key(&state.settings.auth.app_key, value.as_bytes()))
        .transpose()
        .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    let base_url_override = catalog_install
        .then(|| config.base_url_override.clone())
        .flatten();
    let plugin_config = if catalog_install {
        None
    } else {
        Some(
            serde_json::to_value(&config).map_err(|_| AuthError::InvalidRequest {
                field: "plugin_config",
            })?,
        )
    };
    let txn = state
        .database
        .begin()
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    // SeaORM cannot express PostgreSQL advisory locks; serialize same-key installations.
    txn.execute(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        [format!("grengin-engine:{}", manifest.id).into()],
    ))
    .await
    .map_err(|_| AuthError::DbTimeout)?;
    if ai_engines::Entity::find()
        .filter(ai_engines::Column::EngineKey.eq(&manifest.id))
        .one(&txn)
        .await
        .map_err(|_| AuthError::DbTimeout)?
        .is_some()
    {
        return Err(AuthError::DbConflict);
    }
    let engine = ai_engines::ActiveModel {
        id: Set(Uuid::new_v4()),
        display_name: Set(request
            .display_name
            .unwrap_or_else(|| manifest.name.clone())),
        is_enabled: Set(is_enabled),
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
        plugin_config: Set(plugin_config),
        base_url_override: Set(base_url_override),
        api_key_validated_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&txn)
    .await
    .map_err(|_| AuthError::DbTimeout)?;
    txn.commit().await.map_err(|_| AuthError::DbTimeout)?;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(plugin_config: Option<serde_json::Value>) -> ai_engines::Model {
        let now = Utc::now();
        ai_engines::Model {
            id: Uuid::new_v4(),
            display_name: "Azure OpenAI".to_string(),
            is_enabled: true,
            engine_key: "azure-openai".to_string(),
            api_key_status: ApiKeyStatus::NotValidated,
            api_key: None,
            whitelist_models: Vec::new(),
            default_model: "<empty>".to_string(),
            default_image_gen_model: None,
            plugin_config,
            base_url_override: None,
            api_key_validated_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn customer_azure_resource_and_bedrock_region_are_accepted() {
        assert_eq!(
            catalog_base_url_override(" https://contoso.services.ai.azure.com/openai/v1/ ")
                .unwrap()
                .as_deref(),
            Some("https://contoso.services.ai.azure.com/openai/v1/")
        );
        assert_eq!(
            catalog_base_url_override("https://bedrock-runtime.eu-west-1.amazonaws.com/openai/v1/")
                .unwrap()
                .as_deref(),
            Some("https://bedrock-runtime.eu-west-1.amazonaws.com/openai/v1/")
        );
        assert_eq!(catalog_base_url_override("  ").unwrap(), None);
    }

    #[test]
    fn unsafe_override_destinations_are_rejected() {
        for raw in [
            "http://contoso.services.ai.azure.com/openai/v1/",
            "https://127.0.0.1/v1/",
            "https://169.254.169.254/latest/",
            "https://metadata.google.internal/v1/",
            "https://user:pass@contoso.services.ai.azure.com/",
            "https://contoso.services.ai.azure.com/openai/v1/?api-key=x",
            "not a url",
        ] {
            assert!(catalog_base_url_override(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn override_is_stored_only_for_catalog_engines() {
        let catalog = engine(None);
        let mut active = catalog.clone().into_active_model();
        apply_base_url_override(
            &mut active,
            &catalog,
            Some("https://contoso.services.ai.azure.com/openai/v1/"),
            false,
        )
        .unwrap();
        assert_eq!(
            active.base_url_override,
            Set(Some(
                "https://contoso.services.ai.azure.com/openai/v1/".to_string()
            ))
        );

        let mut active = catalog.clone().into_active_model();
        apply_base_url_override(&mut active, &catalog, Some(""), false).unwrap();
        assert_eq!(active.base_url_override, Set(None));

        let custom = engine(Some(serde_json::json!({})));
        let mut active = custom.clone().into_active_model();
        assert!(
            apply_base_url_override(&mut active, &custom, Some("https://example.com/"), false)
                .is_err()
        );
        assert!(
            apply_base_url_override(&mut active, &catalog, Some("https://example.com/"), true)
                .is_err()
        );
    }

    #[test]
    fn replacing_the_plugin_config_drops_a_catalog_override() {
        let mut catalog = engine(None);
        catalog.base_url_override = Some("https://contoso.services.ai.azure.com/".to_string());
        let mut active = catalog.clone().into_active_model();
        apply_base_url_override(&mut active, &catalog, None, true).unwrap();
        assert_eq!(active.base_url_override, Set(None));
    }
}
