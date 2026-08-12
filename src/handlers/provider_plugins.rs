// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chrono::Utc;
use llm_plugin::{
    DeclarativeProvider, ProviderManifestV1, ProviderPlugin, ProviderRuntimeConfig,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, Set, TransactionTrait,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    auth::{
        claims::Claims,
        encryption::encrypt_key,
        error::{AuthError, Error},
        permissions::{PERMISSION_AI_PLATFORM_MANAGE, PERMISSION_AI_PLATFORM_VIEW},
    },
    dto::provider_plugins::{
        ProviderCredentialDefinitionResponse, ProviderCredentialSlotResponse,
        ProviderPluginConnectionTestResponse, ProviderPluginInstallRequest, ProviderPluginResponse,
        ProviderPluginValidationRequest, ProviderPluginValidationResponse,
    },
    models::{provider_credentials, provider_plugins},
    services::{
        authorization::{AuthorizationService, PermissionScopeMode},
        provider_plugins::{build_provider, unregister_provider},
    },
    state::SharedState,
};

const RESERVED_PROVIDER_IDS: &[&str] = &["openai", "anthropic", "mistral", "gemini"];

#[utoipa::path(
    get,
    path = "/admin/provider-plugins/schema",
    tag = "admin",
    responses(
        (status = 200, body = Object),
        (status = 401, body = Error),
        (status = 403, body = Error)
    )
)]
pub async fn get_provider_plugin_schema(
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
    path = "/admin/provider-plugins/validate",
    tag = "admin",
    request_body = ProviderPluginValidationRequest,
    responses(
        (status = 200, body = ProviderPluginValidationResponse),
        (status = 401, body = Error),
        (status = 403, body = Error)
    )
)]
pub async fn validate_provider_plugin(
    claims: Claims,
    State(state): State<SharedState>,
    Json(request): Json<ProviderPluginValidationRequest>,
) -> Result<Json<ProviderPluginValidationResponse>, AuthError> {
    ensure_manage(&claims, &state).await?;
    let manifest_bytes = serde_json::to_vec(&request.manifest)
        .map_err(|_| AuthError::InvalidRequest { field: "manifest" })?;
    let manifest = match ProviderManifestV1::from_json(&manifest_bytes) {
        Ok(manifest) => manifest,
        Err(error) => {
            return Ok(Json(invalid_validation(error.to_string())));
        }
    };
    let credentials = manifest
        .credentials
        .iter()
        .map(|credential| (credential.id.clone(), "validation-placeholder".to_string()))
        .collect();
    let runtime = ProviderRuntimeConfig {
        base_url_override: request.base_url_override.clone(),
        configuration: request.configuration,
        credentials,
        allow_insecure_http: request.allow_insecure_http,
        allow_private_network: request.allow_private_network,
        ..Default::default()
    };
    let provider = match DeclarativeProvider::new(manifest.clone(), runtime) {
        Ok(provider) => provider,
        Err(error) => return Ok(Json(invalid_validation(error.to_string()))),
    };
    Ok(Json(ProviderPluginValidationResponse {
        valid: true,
        provider_key: Some(manifest.id),
        version: Some(manifest.version),
        name: Some(manifest.name),
        digest: Some(digest(&manifest_bytes)),
        destination: Some(
            request
                .base_url_override
                .unwrap_or_else(|| manifest.base_url.clone()),
        ),
        credential_slots: manifest
            .credentials
            .into_iter()
            .map(|credential| ProviderCredentialDefinitionResponse {
                slot_id: credential.id,
                label: credential.label,
                credential_type: match credential.credential_type {
                    llm_plugin::CredentialType::Secret => "secret",
                    llm_plugin::CredentialType::Text => "text",
                }
                .to_string(),
                required: credential.required,
            })
            .collect(),
        capabilities: Some(
            serde_json::to_value(&provider.descriptor().capabilities)
                .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?,
        ),
        error: None,
    }))
}

#[utoipa::path(
    post,
    path = "/admin/provider-plugins",
    tag = "admin",
    request_body = ProviderPluginInstallRequest,
    responses(
        (status = 201, body = ProviderPluginResponse),
        (status = 400, body = Error),
        (status = 401, body = Error),
        (status = 403, body = Error)
    )
)]
pub async fn install_provider_plugin(
    claims: Claims,
    State(state): State<SharedState>,
    Json(request): Json<ProviderPluginInstallRequest>,
) -> Result<(StatusCode, Json<ProviderPluginResponse>), AuthError> {
    ensure_manage(&claims, &state).await?;
    if !request.configuration.is_object() {
        return Err(AuthError::InvalidRequest {
            field: "configuration",
        });
    }
    let manifest_bytes = serde_json::to_vec(&request.manifest)
        .map_err(|_| AuthError::InvalidRequest { field: "manifest" })?;
    let manifest = ProviderManifestV1::from_json(&manifest_bytes)
        .map_err(|_| AuthError::InvalidRequest { field: "manifest" })?;
    if RESERVED_PROVIDER_IDS.contains(&manifest.id.as_str()) {
        return Err(AuthError::InvalidRequest {
            field: "manifest.id",
        });
    }
    let declared_slots = manifest
        .credentials
        .iter()
        .map(|credential| credential.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if request
        .credentials
        .keys()
        .any(|slot| !declared_slots.contains(slot.as_str()))
    {
        return Err(AuthError::InvalidRequest {
            field: "credentials",
        });
    }
    let credentials = request
        .credentials
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(slot, value)| (slot.clone(), value.clone()))
        .collect::<BTreeMap<_, _>>();
    if provider_plugins::Entity::find()
        .filter(provider_plugins::Column::ProviderKey.eq(&manifest.id))
        .one(&state.database)
        .await
        .map_err(|_| AuthError::DbTimeout)?
        .is_some()
    {
        return Err(AuthError::InvalidRequest {
            field: "manifest.id",
        });
    }

    let runtime = ProviderRuntimeConfig {
        base_url_override: request.base_url_override.clone(),
        configuration: request.configuration.clone(),
        credentials: credentials.clone(),
        allow_insecure_http: request.allow_insecure_http,
        allow_private_network: request.allow_private_network,
        ..Default::default()
    };
    let compiled = if request.enabled {
        Some(
            DeclarativeProvider::new(manifest.clone(), runtime)
                .map_err(|_| AuthError::InvalidRequest { field: "manifest" })?,
        )
    } else {
        let placeholder_credentials = manifest
            .credentials
            .iter()
            .map(|credential| (credential.id.clone(), "validation-placeholder".to_string()))
            .collect();
        DeclarativeProvider::new(
            manifest.clone(),
            ProviderRuntimeConfig {
                credentials: placeholder_credentials,
                ..runtime
            },
        )
        .map_err(|_| AuthError::InvalidRequest { field: "manifest" })?;
        None
    };

    let now = Utc::now();
    let plugin_id = Uuid::new_v4();
    let transaction = state
        .database
        .begin()
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    let plugin = provider_plugins::ActiveModel {
        id: Set(plugin_id),
        provider_key: Set(manifest.id.clone()),
        version: Set(manifest.version.clone()),
        manifest: Set(request.manifest),
        digest: Set(digest(&manifest_bytes)),
        source: Set("admin_upload".to_string()),
        status: Set(if request.enabled {
            provider_plugins::ProviderPluginStatus::Enabled
        } else {
            provider_plugins::ProviderPluginStatus::Disabled
        }),
        validation_error: Set(None),
        configuration: Set(request.configuration),
        base_url_override: Set(request.base_url_override),
        allow_insecure_http: Set(request.allow_insecure_http),
        allow_private_network: Set(request.allow_private_network),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&transaction)
    .await
    .map_err(|_| AuthError::DbTimeout)?;
    for (slot_id, value) in credentials {
        let encrypted_value = encrypt_key(&state.settings.auth.app_key, value.as_bytes())
            .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
        provider_credentials::ActiveModel {
            id: Set(Uuid::new_v4()),
            plugin_id: Set(plugin_id),
            slot_id: Set(slot_id),
            encrypted_value: Set(encrypted_value),
            status: Set(provider_credentials::ProviderCredentialStatus::NotValidated),
            validated_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&transaction)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    }
    transaction
        .commit()
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    if let Some(provider) = compiled {
        state.provider_registry.register(Arc::new(provider)).await;
    }
    let response = plugin_response(&state, plugin).await?;
    Ok((StatusCode::CREATED, Json(response)))
}

#[utoipa::path(
    get,
    path = "/admin/provider-plugins",
    tag = "admin",
    responses(
        (status = 200, body = Vec<ProviderPluginResponse>),
        (status = 401, body = Error),
        (status = 403, body = Error)
    )
)]
pub async fn list_provider_plugins(
    claims: Claims,
    State(state): State<SharedState>,
) -> Result<Json<Vec<ProviderPluginResponse>>, AuthError> {
    ensure_view(&claims, &state).await?;
    let plugins = provider_plugins::Entity::find()
        .all(&state.database)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    let mut response = Vec::with_capacity(plugins.len());
    for plugin in plugins {
        response.push(plugin_response(&state, plugin).await?);
    }
    Ok(Json(response))
}

#[utoipa::path(
    get,
    path = "/admin/provider-plugins/{provider_key}",
    tag = "admin",
    params(("provider_key" = String, Path, description = "Provider plugin key")),
    responses(
        (status = 200, body = ProviderPluginResponse),
        (status = 401, body = Error),
        (status = 403, body = Error),
        (status = 404, body = Error)
    )
)]
pub async fn get_provider_plugin(
    claims: Claims,
    State(state): State<SharedState>,
    Path(provider_key): Path<String>,
) -> Result<Json<ProviderPluginResponse>, AuthError> {
    ensure_view(&claims, &state).await?;
    let plugin = find_plugin(&state, &provider_key).await?;
    Ok(Json(plugin_response(&state, plugin).await?))
}

#[utoipa::path(
    post,
    path = "/admin/provider-plugins/{provider_key}/enable",
    tag = "admin",
    params(("provider_key" = String, Path, description = "Provider plugin key")),
    responses(
        (status = 200, body = ProviderPluginResponse),
        (status = 400, body = Error),
        (status = 401, body = Error),
        (status = 403, body = Error),
        (status = 404, body = Error)
    )
)]
pub async fn enable_provider_plugin(
    claims: Claims,
    State(state): State<SharedState>,
    Path(provider_key): Path<String>,
) -> Result<Json<ProviderPluginResponse>, AuthError> {
    ensure_manage(&claims, &state).await?;
    let plugin = find_plugin(&state, &provider_key).await?;
    let provider = build_provider(&state.database, &state.settings.auth.app_key, &plugin)
        .await
        .map_err(|_| AuthError::InvalidRequest {
            field: "providerPlugin",
        })?;
    let mut active = plugin.into_active_model();
    active.status = Set(provider_plugins::ProviderPluginStatus::Enabled);
    active.validation_error = Set(None);
    active.updated_at = Set(Utc::now());
    let plugin = active
        .update(&state.database)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    state.provider_registry.register(Arc::new(provider)).await;
    Ok(Json(plugin_response(&state, plugin).await?))
}

#[utoipa::path(
    post,
    path = "/admin/provider-plugins/{provider_key}/disable",
    tag = "admin",
    params(("provider_key" = String, Path, description = "Provider plugin key")),
    responses(
        (status = 200, body = ProviderPluginResponse),
        (status = 401, body = Error),
        (status = 403, body = Error),
        (status = 404, body = Error)
    )
)]
pub async fn disable_provider_plugin(
    claims: Claims,
    State(state): State<SharedState>,
    Path(provider_key): Path<String>,
) -> Result<Json<ProviderPluginResponse>, AuthError> {
    ensure_manage(&claims, &state).await?;
    let plugin = find_plugin(&state, &provider_key).await?;
    let mut active = plugin.into_active_model();
    active.status = Set(provider_plugins::ProviderPluginStatus::Disabled);
    active.updated_at = Set(Utc::now());
    let plugin = active
        .update(&state.database)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    unregister_provider(&state, &provider_key).await;
    Ok(Json(plugin_response(&state, plugin).await?))
}

#[utoipa::path(
    delete,
    path = "/admin/provider-plugins/{provider_key}",
    tag = "admin",
    params(("provider_key" = String, Path, description = "Provider plugin key")),
    responses(
        (status = 204),
        (status = 401, body = Error),
        (status = 403, body = Error),
        (status = 404, body = Error)
    )
)]
pub async fn delete_provider_plugin(
    claims: Claims,
    State(state): State<SharedState>,
    Path(provider_key): Path<String>,
) -> Result<StatusCode, AuthError> {
    ensure_manage(&claims, &state).await?;
    let plugin = find_plugin(&state, &provider_key).await?;
    provider_plugins::Entity::delete_by_id(plugin.id)
        .exec(&state.database)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    unregister_provider(&state, &provider_key).await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/admin/provider-plugins/{provider_key}/test",
    tag = "admin",
    params(("provider_key" = String, Path, description = "Provider plugin key")),
    responses(
        (status = 200, body = ProviderPluginConnectionTestResponse),
        (status = 401, body = Error),
        (status = 403, body = Error),
        (status = 404, body = Error)
    )
)]
pub async fn test_provider_plugin_connection(
    claims: Claims,
    State(state): State<SharedState>,
    Path(provider_key): Path<String>,
) -> Result<Json<ProviderPluginConnectionTestResponse>, AuthError> {
    ensure_manage(&claims, &state).await?;
    let plugin = find_plugin(&state, &provider_key).await?;
    let provider =
        match build_provider(&state.database, &state.settings.auth.app_key, &plugin).await {
            Ok(provider) => provider,
            Err(error) => {
                let error_class = provider_load_error_class(&error);
                update_credential_validation(&state, plugin.id, false).await?;
                return Ok(Json(ProviderPluginConnectionTestResponse {
                    valid: false,
                    mode: "configuration".to_string(),
                    models_available: None,
                    error_class: Some(error_class.to_string()),
                }));
            }
        };

    let (valid, mode, models_available, error_class) = if let Some(models) = provider.models() {
        match models.list_models().await {
            Ok(models) => (true, "model_list", Some(models.len()), None),
            Err(error) => (
                false,
                "model_list",
                None,
                Some(crate::services::provider_chat::provider_error_class(&error)),
            ),
        }
    } else {
        (true, "configuration", None, None)
    };
    update_credential_validation(&state, plugin.id, valid).await?;
    Ok(Json(ProviderPluginConnectionTestResponse {
        valid,
        mode: mode.to_string(),
        models_available,
        error_class: error_class.map(str::to_string),
    }))
}

async fn update_credential_validation(
    state: &SharedState,
    plugin_id: Uuid,
    valid: bool,
) -> Result<(), AuthError> {
    let credentials = provider_credentials::Entity::find()
        .filter(provider_credentials::Column::PluginId.eq(plugin_id))
        .all(&state.database)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    let now = Utc::now();
    for credential in credentials {
        let mut active = credential.into_active_model();
        active.status = Set(if valid {
            provider_credentials::ProviderCredentialStatus::Valid
        } else {
            provider_credentials::ProviderCredentialStatus::Invalid
        });
        active.validated_at = Set(Some(now));
        active.updated_at = Set(now);
        active
            .update(&state.database)
            .await
            .map_err(|_| AuthError::DbTimeout)?;
    }
    Ok(())
}

fn provider_load_error_class(
    error: &crate::services::provider_plugins::ProviderPluginLoadError,
) -> &'static str {
    match error {
        crate::services::provider_plugins::ProviderPluginLoadError::Database => "database",
        crate::services::provider_plugins::ProviderPluginLoadError::CredentialDecryption => {
            "credential_decryption"
        }
        crate::services::provider_plugins::ProviderPluginLoadError::Provider(error) => {
            crate::services::provider_chat::provider_error_class(error)
        }
    }
}

async fn find_plugin(
    state: &SharedState,
    provider_key: &str,
) -> Result<provider_plugins::Model, AuthError> {
    provider_plugins::Entity::find()
        .filter(provider_plugins::Column::ProviderKey.eq(provider_key))
        .one(&state.database)
        .await
        .map_err(|_| AuthError::DbTimeout)?
        .ok_or(AuthError::ResourceNotFound)
}

async fn plugin_response(
    state: &SharedState,
    plugin: provider_plugins::Model,
) -> Result<ProviderPluginResponse, AuthError> {
    let manifest = ProviderManifestV1::from_json(
        &serde_json::to_vec(&plugin.manifest).map_err(|_| AuthError::DbTimeout)?,
    )
    .map_err(|_| AuthError::DbTimeout)?;
    let credentials = provider_credentials::Entity::find()
        .filter(provider_credentials::Column::PluginId.eq(plugin.id))
        .all(&state.database)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    let by_slot = credentials
        .into_iter()
        .map(|credential| (credential.slot_id.clone(), credential))
        .collect::<BTreeMap<_, _>>();
    let credential_slots = manifest
        .credentials
        .iter()
        .map(|definition| {
            let credential = by_slot.get(&definition.id);
            ProviderCredentialSlotResponse {
                slot_id: definition.id.clone(),
                configured: credential.is_some(),
                status: credential
                    .map(|credential| credential_status(&credential.status))
                    .unwrap_or("not_configured")
                    .to_string(),
                validated_at: credential.and_then(|credential| credential.validated_at),
            }
        })
        .collect();
    let capabilities = serde_json::to_value(manifest.descriptor().capabilities)
        .map_err(|_| AuthError::DbTimeout)?;
    let destination = plugin
        .base_url_override
        .clone()
        .unwrap_or_else(|| manifest.base_url.clone());
    Ok(ProviderPluginResponse {
        id: plugin.id,
        provider_key: plugin.provider_key,
        version: plugin.version,
        name: manifest.name,
        digest: plugin.digest,
        source: plugin.source,
        status: plugin_status(&plugin.status).to_string(),
        validation_error: plugin.validation_error,
        destination,
        capabilities,
        credential_slots,
        allow_insecure_http: plugin.allow_insecure_http,
        allow_private_network: plugin.allow_private_network,
        created_at: plugin.created_at,
        updated_at: plugin.updated_at,
    })
}

async fn ensure_view(claims: &Claims, state: &SharedState) -> Result<(), AuthError> {
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

async fn ensure_manage(claims: &Claims, state: &SharedState) -> Result<(), AuthError> {
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

fn invalid_validation(error: String) -> ProviderPluginValidationResponse {
    ProviderPluginValidationResponse {
        valid: false,
        provider_key: None,
        version: None,
        name: None,
        digest: None,
        destination: None,
        credential_slots: Vec::new(),
        capabilities: None,
        error: Some(error),
    }
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn plugin_status(status: &provider_plugins::ProviderPluginStatus) -> &'static str {
    match status {
        provider_plugins::ProviderPluginStatus::Enabled => "enabled",
        provider_plugins::ProviderPluginStatus::Disabled => "disabled",
        provider_plugins::ProviderPluginStatus::Invalid => "invalid",
    }
}

fn credential_status(status: &provider_credentials::ProviderCredentialStatus) -> &'static str {
    match status {
        provider_credentials::ProviderCredentialStatus::Valid => "valid",
        provider_credentials::ProviderCredentialStatus::Invalid => "invalid",
        provider_credentials::ProviderCredentialStatus::NotValidated => "not_validated",
    }
}
