// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::Utc;
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
        AIEngineUpdate, AIEngineValidation,
    },
    models::ai_engines::{self, ApiKeyStatus},
    services::{
        ai_engine_defaults::{enabled_after_key, starter_models},
        authorization::{AuthorizationService, PermissionScopeMode},
        provider_chat::provider_error_class,
        provider_runtime::{
            ProviderLoadError, build_provider, compile_engine, parse_manifest, parse_plugin_config,
            provider_plugin_version,
        },
    },
    state::SharedState,
};

pub const RESERVED_ENGINE_KEYS: &[&str] = &["openai", "anthropic", "mistral", "gemini", "grengin"];

const BASE_URL_OVERRIDE_FIELD: AuthError = AuthError::InvalidRequest {
    field: "base_url_override",
};

// Catalog engines such as Azure OpenAI (per-resource host) and Bedrock (per-region host) need
// the customer's own endpoint; it gets the same destination rules as a custom plugin URL.
fn catalog_base_url_override(engine_key: &str, raw: &str) -> Result<Option<String>, AuthError> {
    if engine_key == "grengin" {
        return Err(BASE_URL_OVERRIDE_FIELD);
    }
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

#[derive(Debug, PartialEq, Eq)]
struct Destination {
    base_url: Option<String>,
    allow_insecure_http: bool,
    allow_private_network: bool,
}

impl Destination {
    // None for a catalog engine's own catalog URL; the plugin's URL for a custom engine.
    fn of(engine: &ai_engines::Model) -> Option<Self> {
        let Some(value) = engine.plugin_config.as_ref() else {
            return Some(Self {
                base_url: engine.base_url_override.clone(),
                allow_insecure_http: false,
                allow_private_network: false,
            });
        };
        let config = parse_plugin_config(value).ok()?;
        let base_url = match config.base_url_override.clone() {
            Some(url) => url,
            None => parse_manifest(&config).ok()?.base_url,
        };
        Some(Self {
            base_url: Some(base_url),
            allow_insecure_http: config.allow_insecure_http,
            allow_private_network: config.allow_private_network,
        })
    }
}

// Requests carry the engine's key to wherever the engine points, so a stored key never follows a
// new destination; the key must be entered again with it.
pub fn ensure_key_follows_destination(
    previous: &ai_engines::Model,
    updated: &ai_engines::Model,
    key_supplied: bool,
) -> Result<(), AuthError> {
    if previous.api_key.is_none()
        || key_supplied
        || Destination::of(previous) == Destination::of(updated)
    {
        return Ok(());
    }
    Err(AuthError::InvalidRequest { field: "api_key" })
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
            active.base_url_override = Set(catalog_base_url_override(&engine.engine_key, raw)?);
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
        default_model: Some(engine.default_model).filter(|model| !model.is_empty()),
        default_image_gen_model: engine.default_image_gen_model,
        plugin_config,
        base_url_override: engine.base_url_override,
        pending_operation: None,
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
    if is_catalog_grengin(&engine) {
        let result = grengin_model_access().await;
        return Ok(AIEngineConnectionTest {
            valid: result
                .as_ref()
                .is_ok_and(|access| grengin_access_usable(access)),
            mode: "workspace_connection".to_string(),
            models_available: None,
            error_class: result
                .err()
                .map(|error| provider_error_class(&error).to_string()),
        });
    }
    let provider = match build_provider(state, &engine).await {
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

async fn grengin_model_access() -> Result<&'static str, llm_plugin::ProviderError> {
    super::grengin_grant::GrenginGrant::configured()?
        .status()
        .await
}

// The connection can be healthy while the workspace has model access switched off or no balance.
fn grengin_access_usable(access: &str) -> bool {
    matches!(access, "on" | "low")
}

pub async fn validate_grengin_connection() -> AIEngineValidation {
    let (valid, message) = match grengin_model_access().await {
        Ok(access) if grengin_access_usable(access) => (
            true,
            format!(
                "Workspace connection validated. Grengin model access is {access}; model availability was not tested."
            ),
        ),
        Ok(access) => (
            false,
            format!(
                "Workspace connection validated, but Grengin model access is {access}, so models cannot be used yet."
            ),
        ),
        Err(error) => (
            false,
            format!("Workspace connection could not be validated: {error}"),
        ),
    };
    AIEngineValidation {
        valid,
        message,
        models_available: 0,
    }
}

/// Applies a model-policy edit to the catalog Grengin engine. Grengin has no API key or plugin
/// configuration; adding a model or a new default checks it against the live service catalog.
/// Returns the requested enable/disable, which the caller runs through `grengin_consent` after
/// saving: an enable never opens routing in this write.
pub async fn apply_grengin_update(
    state: &SharedState,
    engine: &ai_engines::Model,
    req: &mut AIEngineUpdate,
    active: &mut ai_engines::ActiveModel,
) -> Result<Option<bool>, AuthError> {
    let toggle = grengin_intent(req)?;
    if req.whitelisted_models.is_none()
        && req.default_model.is_none()
        && req.default_image_gen_model.is_none()
    {
        return Ok(toggle);
    }
    let additions = req.whitelisted_models.as_ref().is_some_and(|models| {
        models
            .iter()
            .any(|id| !engine.whitelist_models.contains(id))
    });
    let new_default = req
        .default_model
        .as_deref()
        .is_some_and(|id| !id.is_empty() && id != engine.default_model)
        || req.default_image_gen_model.as_deref().is_some_and(|id| {
            !id.is_empty() && Some(id) != engine.default_image_gen_model.as_deref()
        });
    let models = if additions || new_default {
        build_provider(state, engine)
            .await
            .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?
            .models()
            .ok_or(AuthError::ServiceTemporarilyUnavailable)?
            .list_models()
            .await
            .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?
    } else {
        Vec::new()
    };
    let (models, text, image) = grengin_model_policy(
        &models,
        Some(engine),
        req.whitelisted_models.take(),
        req.default_model.take(),
        req.default_image_gen_model.take(),
    )?;
    active.whitelist_models = Set(models);
    active.default_model = Set(text);
    active.default_image_gen_model = Set(image);
    Ok(toggle)
}

/// Every explicit enable/disable goes through the lifecycle, even one that matches the stored
/// flag: a disable while an enable is still pending must supersede it, and the generic update
/// never writes is_enabled for this engine.
fn grengin_intent(req: &mut AIEngineUpdate) -> Result<Option<bool>, AuthError> {
    if req.api_key.is_some() || req.plugin_config.is_some() || req.base_url_override.is_some() {
        return Err(AuthError::InvalidRequest {
            field: "api_key/plugin_config",
        });
    }
    let toggle = req.is_enabled.take();
    super::grengin_consent::require_consent(toggle == Some(true), req.consent_version.as_deref())?;
    Ok(toggle)
}

pub fn is_catalog_grengin(engine: &ai_engines::Model) -> bool {
    engine.engine_key == "grengin" && engine.plugin_config.is_none() && engine.api_key.is_none()
}

/// Existing unavailable selections may be retained or removed. Removed defaults
/// are cleared, never substituted. Additions/default changes require the catalog.
pub fn grengin_model_policy(
    models: &[llm_plugin::ProviderModel],
    previous: Option<&ai_engines::Model>,
    selected: Option<Vec<String>>,
    text: Option<String>,
    image: Option<String>,
) -> Result<(Vec<String>, String, Option<String>), AuthError> {
    use llm_plugin::ProviderModelType::{ImageGenerator, TextGenerator};
    let mut models = models.iter().collect::<Vec<_>>();
    models.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
    let selected = selected.unwrap_or_else(|| {
        if let Some(previous) = previous {
            return previous.whitelist_models.clone();
        }
        let image = models.iter().find(|m| m.model_type == ImageGenerator);
        let mut selected = models
            .iter()
            .filter(|m| m.model_type == TextGenerator)
            .take(if image.is_some() { 2 } else { 3 })
            .map(|m| m.id.to_string())
            .collect::<Vec<_>>();
        if let Some(image) = image {
            selected.push(image.id.to_string());
        }
        selected
    });
    let mut seen = std::collections::HashSet::new();
    for id in &selected {
        if !seen.insert(id)
            || (!previous.is_some_and(|p| p.whitelist_models.contains(id))
                && !models.iter().any(|m| m.id.as_str() == id))
        {
            return Err(AuthError::InvalidRequest {
                field: "whitelisted_models",
            });
        }
    }
    let default = |requested: Option<String>, old: Option<&str>, kind, field| {
        let value = requested
            .or_else(|| match previous {
                Some(_) => old
                    .filter(|id| selected.iter().any(|s| s == id))
                    .map(str::to_owned),
                None => models
                    .iter()
                    .find(|m| m.model_type == kind && selected.iter().any(|s| s == m.id.as_str()))
                    .map(|m| m.id.to_string()),
            })
            .filter(|value| !value.is_empty());
        if let Some(id) = value.as_deref() {
            let current = models.iter().find(|m| m.id.as_str() == id);
            if !selected.iter().any(|s| s == id)
                || !current.map_or(old == Some(id), |m| m.model_type == kind)
            {
                return Err(AuthError::InvalidRequest { field });
            }
        }
        Ok(value)
    };
    let text = default(
        text,
        previous.map(|p| p.default_model.as_str()),
        TextGenerator,
        "default_model",
    )?
    .unwrap_or_default();
    let image = default(
        image,
        previous.and_then(|p| p.default_image_gen_model.as_deref()),
        ImageGenerator,
        "default_image_gen_model",
    )?;
    Ok((selected, text, image))
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
            let mut config = super::provider_runtime::catalog_config(package.plugin);
            config.base_url_override = request
                .base_url_override
                .as_deref()
                .map(|raw| catalog_base_url_override(key, raw))
                .transpose()?
                .flatten();
            (config, true)
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
    let mut is_enabled = enabled_after_key(false, api_key.is_some(), request.is_enabled);
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
    let provider = compile_engine(
        config.clone(),
        &manifest.id,
        compile_key,
        catalog_install.then_some(&state.discovery_catalog),
    )
    .map_err(|_| AuthError::InvalidRequest {
        field: "plugin_config",
    })?;

    let now = Utc::now();
    let mut grengin_enable = false;
    let (whitelist_models, default_model, default_image_gen_model) =
        if catalog_install && manifest.id == "grengin" {
            let identity_ready = super::grengin_grant::GrenginGrant::configured().is_ok();
            let models = if identity_ready
                && request
                    .whitelisted_models
                    .as_ref()
                    .is_none_or(|m| !m.is_empty())
            {
                match provider.models() {
                    Some(models) => models.list_models().await.unwrap_or_default(),
                    None => Vec::new(),
                }
            } else {
                Vec::new()
            };
            let policy = grengin_model_policy(
                &models,
                None,
                request.whitelisted_models,
                request.default_model,
                request.default_image_gen_model,
            )?;
            // Installed disabled; an explicit enable runs the consent flow after the insert.
            super::grengin_consent::require_consent(
                request.is_enabled == Some(true),
                request.consent_version.as_deref(),
            )?;
            grengin_enable = request.is_enabled == Some(true);
            is_enabled = false;
            policy
        } else {
            let models = request
                .whitelisted_models
                .unwrap_or_else(|| starter_models(&manifest));
            let default = request
                .default_model
                .or_else(|| models.first().cloned())
                .unwrap_or_else(|| "<empty>".to_string());
            (models, default, request.default_image_gen_model)
        };
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
        default_image_gen_model: Set(default_image_gen_model),
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
        state.provider_registry.register(provider).await;
    }
    let mut detail = engine_detail(state, engine);
    if grengin_enable {
        let (pending, enabled) = super::grengin_consent::set_enabled(state, true).await?;
        detail.pending_operation = pending;
        detail.is_enabled = enabled;
    }
    Ok(detail)
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
            catalog_base_url_override(
                "azure-openai",
                " https://contoso.services.ai.azure.com/openai/v1/ "
            )
            .unwrap()
            .as_deref(),
            Some("https://contoso.services.ai.azure.com/openai/v1/")
        );
        assert_eq!(
            catalog_base_url_override(
                "bedrock",
                "https://bedrock-runtime.eu-west-1.amazonaws.com/openai/v1/"
            )
            .unwrap()
            .as_deref(),
            Some("https://bedrock-runtime.eu-west-1.amazonaws.com/openai/v1/")
        );
        assert_eq!(catalog_base_url_override("bedrock", "  ").unwrap(), None);
    }

    #[test]
    fn grengin_only_uses_its_registered_service_origin() {
        assert!(catalog_base_url_override("grengin", "https://example.com/").is_err());
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
            assert!(
                catalog_base_url_override("azure-openai", raw).is_err(),
                "{raw}"
            );
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
    fn a_stored_key_never_follows_a_new_destination() {
        let mut catalog = engine(None);
        catalog.api_key = Some("encrypted".to_string());
        catalog.base_url_override = Some("https://contoso.services.ai.azure.com/".to_string());
        let mut moved = catalog.clone();
        moved.base_url_override = Some("https://attacker.example/".to_string());
        assert!(ensure_key_follows_destination(&catalog, &moved, false).is_err());
        assert!(ensure_key_follows_destination(&catalog, &moved, true).is_ok());
        let mut renamed = catalog.clone();
        renamed.display_name = "Azure".to_string();
        assert!(ensure_key_follows_destination(&catalog, &renamed, false).is_ok());
        let mut cleared = catalog.clone();
        cleared.base_url_override = None;
        assert!(ensure_key_follows_destination(&catalog, &cleared, false).is_err());
        let mut keyless = moved.clone();
        keyless.api_key = None;
        assert!(ensure_key_follows_destination(&keyless, &catalog, false).is_ok());

        let config = |base_url: &str, private: bool| {
            serde_json::json!({"manifest": {}, "configuration": {}, "baseUrlOverride": base_url,
                               "allowPrivateNetwork": private})
        };
        let mut custom = engine(Some(config("https://llm.example.com/v1/", false)));
        custom.api_key = Some("encrypted".to_string());
        let mut same = custom.clone();
        same.plugin_config = Some(config("https://llm.example.com/v1/", false));
        assert!(ensure_key_follows_destination(&custom, &same, false).is_ok());
        let mut private = custom.clone();
        private.plugin_config = Some(config("https://llm.example.com/v1/", true));
        assert!(ensure_key_follows_destination(&custom, &private, false).is_err());
        let mut elsewhere = custom.clone();
        elsewhere.plugin_config = Some(config("https://attacker.example/v1/", false));
        assert!(ensure_key_follows_destination(&custom, &elsewhere, false).is_err());
        assert!(ensure_key_follows_destination(&custom, &catalog, false).is_err());
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

#[cfg(test)]
mod grengin_policy_tests {
    use super::*;
    use llm_plugin::{ProviderModel, ProviderModelType};

    fn models() -> Vec<ProviderModel> {
        [
            ("z/text", ProviderModelType::TextGenerator),
            ("a/image", ProviderModelType::ImageGenerator),
            ("b/text", ProviderModelType::TextGenerator),
            ("a/text", ProviderModelType::TextGenerator),
            ("a/embed", ProviderModelType::TextEmbedder),
        ]
        .into_iter()
        .map(|(id, model_type)| ProviderModel {
            id: id.into(),
            name: id.into(),
            model_type,
            capabilities: Default::default(),
            metadata: serde_json::Value::Null,
        })
        .collect()
    }

    #[test]
    fn defaults_are_bounded_sorted_and_capability_specific() {
        let mut catalog = models();
        let policy = grengin_model_policy(&catalog, None, None, None, None).unwrap();
        assert_eq!(policy.0, vec!["a/text", "b/text", "a/image"]);
        assert_eq!(policy.1, "a/text");
        assert_eq!(policy.2.as_deref(), Some("a/image"));
        let empty = grengin_model_policy(&catalog, None, Some(vec![]), None, None).unwrap();
        assert_eq!(empty, (vec![], "".into(), None));
        assert!(grengin_model_policy(&catalog, None, None, Some("a/image".into()), None).is_err());
        assert!(grengin_model_policy(&catalog, None, None, None, Some("a/text".into())).is_err());
        catalog.retain(|model| model.model_type != ProviderModelType::ImageGenerator);
        assert_eq!(
            grengin_model_policy(&catalog, None, None, None, None)
                .unwrap()
                .0,
            vec!["a/text", "b/text", "z/text"]
        );
    }

    #[test]
    fn unavailable_selections_can_be_removed_but_not_added_or_promoted() {
        let old: ai_engines::Model = serde_json::from_value(serde_json::json!({
            "id":Uuid::new_v4(), "displayName":"Grengin", "engineKey":"grengin", "isEnabled":false,
            "apiKeyStatus":"not_configured", "apiKey":null, "apiKeyValidatedAt":null, "pluginConfig":null,
            "whitelistModels":["retired/text","retired/image"], "defaultModel":"retired/text",
            "defaultImageGenModel":"retired/image", "createdAt":Utc::now(), "updatedAt":Utc::now()
        })).unwrap();
        let retained = grengin_model_policy(&[], Some(&old), None, None, None).unwrap();
        assert_eq!(retained.0, old.whitelist_models);
        assert_eq!(retained.1, old.default_model);
        assert_eq!(
            grengin_model_policy(&[], Some(&old), Some(vec![]), None, None).unwrap(),
            (vec![], "".into(), None)
        );
        assert!(
            grengin_model_policy(&[], None, Some(old.whitelist_models.clone()), None, None)
                .is_err()
        );
        assert!(
            grengin_model_policy(&[], Some(&old), None, Some("retired/image".into()), None)
                .is_err()
        );
    }
}

#[cfg(test)]
mod grengin_intent_tests {
    use super::*;

    fn update(is_enabled: Option<bool>, consent: Option<&str>) -> AIEngineUpdate {
        serde_json::from_value(serde_json::json!({
            "is_enabled": is_enabled,
            "consent_version": consent,
        }))
        .unwrap()
    }

    #[test]
    fn every_explicit_intent_is_kept_and_never_reaches_the_generic_update() {
        // A disable is honoured even when the stored flag is already false (an enable may be
        // pending), and an enable even when it is already true.
        for (requested, consent) in [(false, None), (true, Some("grengin-models-v1"))] {
            let mut req = update(Some(requested), consent);
            assert_eq!(grengin_intent(&mut req).unwrap(), Some(requested));
            assert!(req.is_enabled.is_none());
        }
        let mut req = update(None, None);
        assert_eq!(grengin_intent(&mut req).unwrap(), None);
    }

    #[test]
    fn enabling_without_consent_and_credential_fields_are_refused() {
        assert!(grengin_intent(&mut update(Some(true), None)).is_err());
        let mut keyed: AIEngineUpdate =
            serde_json::from_value(serde_json::json!({"api_key": "sk-x"})).unwrap();
        assert!(grengin_intent(&mut keyed).is_err());
    }
}
