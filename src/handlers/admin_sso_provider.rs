// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::{
        claims::Claims,
        encryption::{decrypt_key, encrypt_key},
        error::{AuthError, Error},
        permissions::{PERMISSION_SSO_PROVIDERS_MANAGE, PERMISSION_SSO_PROVIDERS_VIEW},
        provider_config::{
            OidcProviderConfiguration, normalize_provider_slug, validate_provider_url,
        },
        sso_provider::is_editable,
    },
    dto::admin_sso_providers::{
        EditableField, GrenginProxySetupRequest, SsoProvider, SsoProviderCreate,
        SsoProviderEditable, SsoProviderUpdate, SsoProviderValidationRequest,
        SsoProviderValidationResponse,
    },
    models::sso_providers,
    services::{
        authorization::{AuthorizationService, PermissionScopeMode},
        sso_seed::{
            EMPTY_VALUE, ensure_sso_providers_from_env, grengin_proxy_available_for_provider,
        },
        sso_validation::{
            build_draft_config, config_hash, has_sensitive_changes, issue_validation_token,
            validate_sso_draft, validate_validation_token,
        },
    },
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
};
use uuid::Uuid;

fn configuration_from_model(model: &sso_providers::Model) -> OidcProviderConfiguration {
    OidcProviderConfiguration::from_value_for_provider(
        model.configuration.as_ref(),
        &model.provider,
    )
    .unwrap_or_default()
}

fn provider_response(app_state: &SharedState, model: sso_providers::Model) -> SsoProvider {
    let configuration = configuration_from_model(&model);
    let grengin_proxy_available = grengin_proxy_available_for_provider(&model.provider);
    SsoProvider {
        id: model.id,
        redirect_url: model.redirect_url,
        provider: model.provider,
        name: model.name,
        client_id: model.client_id,
        client_secret: app_state
            .get_decrypted_api_key_preview(&Some(model.client_secret))
            .unwrap_or(EMPTY_VALUE.to_string()),
        issuer_url: model.issuer_url,
        tenant_id: model.tenant_id,
        allowed_domains: model.allowed_domains,
        is_enabled: model.is_enabled,
        use_grengin_proxy: model.use_grengin_proxy,
        jit_provisioning: model.jit_provisioning,
        configuration,
        grengin_proxy_available,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

#[utoipa::path(
    get,
    path = "/admin/sso-providers",
    tag = "admin",
    responses(
       (status = 200, body = Vec<SsoProvider>),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 404, content_type = "application/json", body = Error, description = "Email does not exist (code=6101)"),
       (status = 404, content_type = "application/json", body = Error, description = "Sso Provider not found (code=5003)"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),
    )
)]
pub async fn get_sso_providers(
    claims: Claims,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<Vec<SsoProvider>>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_SSO_PROVIDERS_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;
    let models = sso_providers::Entity::find()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("Db get all error: {:?}", e);
            AuthError::DbTimeout
        })?;
    let models = ensure_sso_providers_from_env(&app_state, models).await?;
    let response = models
        .into_iter()
        .map(|model| provider_response(&app_state, model))
        .collect();
    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    post,
    path = "/admin/sso-providers",
    tag = "admin",
    request_body = SsoProviderCreate,
    responses(
        (status = 201, body = SsoProvider),
        (status = 400, content_type = "application/json", body = Error, description = "Invalid provider configuration"),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 409, content_type = "application/json", body = Error, description = "Provider slug already exists"),
    )
)]
pub async fn create_sso_provider(
    claims: Claims,
    State(app_state): State<SharedState>,
    Json(req): Json<SsoProviderCreate>,
) -> Result<(StatusCode, Json<SsoProvider>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_SSO_PROVIDERS_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let provider =
        normalize_provider_slug(&req.provider).map_err(|_| AuthError::InvalidProvider {
            provider: Some(req.provider.clone()),
        })?;
    validate_provider_url(&req.issuer_url, true).map_err(|_| AuthError::InvalidProvider {
        provider: Some(provider.clone()),
    })?;
    validate_provider_url(&req.redirect_url, true).map_err(|_| AuthError::InvalidRedirectUri {
        redirect_uri: Some(req.redirect_url.clone()),
    })?;
    req.configuration
        .validate_for_provider(&provider)
        .map_err(|_| AuthError::InvalidProvider {
            provider: Some(provider.clone()),
        })?;

    let exists = sso_providers::Entity::find()
        .filter(sso_providers::Column::Provider.eq(&provider))
        .one(&app_state.database)
        .await
        .map_err(|_| AuthError::DbTimeout)?
        .is_some();
    if exists {
        return Err(AuthError::DbConflict);
    }

    let mut allowed_domains = req
        .allowed_domains
        .into_iter()
        .map(|domain| domain.trim().trim_start_matches('@').to_ascii_lowercase())
        .filter(|domain| !domain.is_empty())
        .collect::<Vec<_>>();
    allowed_domains.sort();
    allowed_domains.dedup();
    let now = Utc::now();
    let model = sso_providers::ActiveModel {
        id: Set(Uuid::new_v4()),
        provider: Set(provider.clone()),
        name: Set(req.name.trim().to_string()),
        tenant_id: Set(req.tenant_id),
        client_id: Set(req.client_id.trim().to_string()),
        client_secret: Set(encrypt_key(
            &app_state.settings.auth.app_key,
            req.client_secret.trim().as_bytes(),
        )
        .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?),
        issuer_url: Set(req.issuer_url),
        redirect_url: Set(req.redirect_url),
        allowed_domains: Set(allowed_domains),
        is_enabled: Set(false),
        is_default: Set(false),
        use_grengin_proxy: Set(false),
        jit_provisioning: Set(req.jit_provisioning),
        configuration: Set(Some(
            serde_json::to_value(req.configuration)
                .map_err(|_| AuthError::InvalidCallbackParameters)?,
        )),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&app_state.database)
    .await
    .map_err(|error| {
        eprintln!("DB insert SSO provider error: {error:?}");
        AuthError::DbConflict
    })?;

    app_state
        .refresh_oidc_client(&provider)
        .await
        .map_err(|error| {
            eprintln!("OIDC provider cache refresh failed: {error:?}");
            AuthError::ServiceTemporarilyUnavailable
        })?;
    Ok((
        StatusCode::CREATED,
        Json(provider_response(&app_state, model)),
    ))
}

#[utoipa::path(
    get,
    path = "/admin/sso-providers/{provider_id}",
    tag = "admin",
    responses(
       (status = 200, body = inline(SsoProviderEditable)),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 404, content_type = "application/json", body = Error, description = "Sso Provider not found (code=5003)"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),
    )
)]
pub async fn get_sso_provider_by_id(
    claims: Claims,
    Path(provider_id): Path<Uuid>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<SsoProviderEditable>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_SSO_PROVIDERS_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            Some(provider_id),
        )
        .await?;
    let model = sso_providers::Entity::find_by_id(provider_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("Db get all error: {}", e);
            AuthError::DbTimeout
        })?;
    let response = model
        .map(|model| {
            let editable = is_editable(&model.provider);
            let configuration = configuration_from_model(&model);
            SsoProviderEditable {
                id: model.id,
                provider: EditableField {
                    editable,
                    value: model.provider,
                },
                name: EditableField {
                    editable,
                    value: model.name,
                },
                tenant_id: model.tenant_id.map(|t_id| EditableField {
                    editable: true,
                    value: t_id,
                }),
                redirect_url: EditableField {
                    editable,
                    value: model.redirect_url,
                },
                client_id: EditableField {
                    editable: true,
                    value: model.client_id,
                },
                client_secret: app_state
                    .get_decrypted_api_key_preview(&Some(model.client_secret))
                    .map(|preview| EditableField {
                        editable: true,
                        value: preview,
                    }),
                issuer_url: EditableField {
                    editable,
                    value: model.issuer_url,
                },
                allowed_domains: model.allowed_domains,
                is_enabled: model.is_enabled,
                jit_provisioning: model.jit_provisioning,
                configuration,
                created_at: model.created_at,
                updated_at: model.updated_at,
            }
        })
        .ok_or(AuthError::ResourceNotFound)?;
    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    post,
    path = "/admin/sso-providers/{provider_id}/validate",
    tag = "admin",
    responses(
       (status = 200, body = SsoProviderValidationResponse),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 404, content_type = "application/json", body = Error, description = "Sso Provider not found (code=5003)"),
       (status = 503, content_type = "application/json", body = Error, description = "Validation service unavailable"),
    )
)]
pub async fn validate_sso_provider_by_id(
    claims: Claims,
    Path(provider_id): Path<Uuid>,
    State(app_state): State<SharedState>,
    Json(req): Json<SsoProviderValidationRequest>,
) -> Result<(StatusCode, Json<SsoProviderValidationResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_SSO_PROVIDERS_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            Some(provider_id),
        )
        .await?;
    let model = sso_providers::Entity::find_by_id(provider_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("Db get one error: {}", e);
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::DbNotFound)?;

    let draft = build_draft_config(
        &app_state,
        &model,
        req.provider.as_ref(),
        req.tenant_id.as_ref(),
        req.client_id.as_ref(),
        req.client_secret.as_ref(),
        req.issuer_url.as_ref(),
        req.redirect_url.as_ref(),
        req.frontend_hosted_url.as_ref(),
        req.configuration.as_ref(),
    )?;

    let (valid, message) = validate_sso_draft(&app_state, &draft).await?;
    let (validation_token, validation_token_expires_at) = if valid {
        let (token, expires_at) = issue_validation_token(provider_id, claims.user_id, &draft);
        (Some(token), Some(expires_at))
    } else {
        (None, None)
    };

    Ok((
        StatusCode::OK,
        Json(SsoProviderValidationResponse {
            valid,
            message,
            redirect_url: draft.redirect_url,
            validation_token,
            validation_token_expires_at,
        }),
    ))
}

#[utoipa::path(
    delete,
    path = "/admin/sso-providers/{provider_id}",
    tag = "admin",
    responses(
       (status = 200, body = SsoProvider),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 404, content_type = "application/json", body = Error, description = "Sso Provider not found (code=5003)"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),
    )
)]
pub async fn delete_sso_provider_by_id(
    claims: Claims,
    Path(provider_id): Path<Uuid>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, &'static str), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_SSO_PROVIDERS_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            Some(provider_id),
        )
        .await?;
    let model = sso_providers::Entity::find_by_id(provider_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("Db get one error: {}", e);
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;
    let provider = model.provider.clone();
    let mut active_model = model.into_active_model();
    active_model.client_id = Set("<empty>".to_string());
    active_model.client_secret = Set("<empty>".to_string());
    active_model.updated_at = Set(Utc::now());
    active_model.is_default = Set(false);
    active_model.is_enabled = Set(false);
    active_model.tenant_id = Set(None);
    active_model
        .update(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("Db get one error: {}", e);
            AuthError::DbTimeout
        })?;
    app_state.remove_oidc_provider(&provider).await;
    Ok((StatusCode::OK, "Deleted successfully"))
}

#[utoipa::path(
    put,
    path = "/admin/sso-providers/{provider_id}",
    tag = "admin",
    responses(
       (status = 200, body = SsoProvider),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 404, content_type = "application/json", body = Error, description = "Sso Provider not found (code=5003)"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),
    )
)]
pub async fn update_sso_provider_by_id(
    claims: Claims,
    Path(provider_id): Path<Uuid>,
    State(app_state): State<SharedState>,
    Json(req): Json<SsoProviderUpdate>,
) -> Result<(StatusCode, Json<SsoProvider>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_SSO_PROVIDERS_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            Some(provider_id),
        )
        .await?;
    let model = sso_providers::Entity::find_by_id(provider_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("Db get one error: {}", e);
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::DbNotFound)?;
    let original_provider = model.provider.clone();
    let draft = build_draft_config(
        &app_state,
        &model,
        req.provider.as_ref(),
        req.tenant_id.as_ref(),
        req.client_id.as_ref(),
        req.client_secret.as_ref(),
        req.issuer_url.as_ref(),
        req.redirect_url.as_ref(),
        req.frontend_hosted_url.as_ref(),
        req.configuration.as_ref(),
    )?;
    let enabling_provider = req.is_enabled == Some(true) && !model.is_enabled;
    if has_sensitive_changes(&app_state, &model, &draft) || enabling_provider {
        let validation_token = req
            .validation_token
            .as_deref()
            .ok_or(AuthError::InvalidToken)?;
        validate_validation_token(
            validation_token,
            provider_id,
            claims.user_id,
            &config_hash(&draft),
        )?;
    }

    let mut active_model = model.into_active_model();
    if req.provider.is_some() {
        active_model.provider = Set(draft.provider.clone());
    }
    if let Some(name) = req.name {
        active_model.name = Set(name);
    }
    if let Some(allowed_domains) = req.allowed_domains {
        let mut allowed_domains = allowed_domains
            .into_iter()
            .map(|domain| domain.trim().trim_start_matches('@').to_ascii_lowercase())
            .filter(|domain| !domain.is_empty())
            .collect::<Vec<_>>();
        allowed_domains.sort();
        allowed_domains.dedup();
        active_model.allowed_domains = Set(allowed_domains);
    }
    if let Some(client_id) = req.client_id {
        active_model.client_id = Set(client_id);
    }
    if let Some(is_enabled) = req.is_enabled {
        active_model.is_enabled = Set(is_enabled);
    }
    if req.issuer_url.is_some() {
        active_model.issuer_url = Set(draft.issuer_url.clone());
    }
    if req.redirect_url.is_some() || req.frontend_hosted_url.is_some() {
        active_model.redirect_url = Set(draft.redirect_url.clone());
    }
    if req.tenant_id.is_some() {
        active_model.tenant_id = Set(draft.tenant_id.clone());
    }
    if let Some(client_secret) = req.client_secret {
        active_model.client_secret = Set(encrypt_key(
            &app_state.settings.auth.app_key,
            client_secret.as_bytes(),
        )
        .map_err(|e| {
            eprintln!("Sso key encryption error {:?}", e);
            AuthError::DbTimeout
        })?);
    }
    if let Some(jit_provisioning) = req.jit_provisioning {
        active_model.jit_provisioning = Set(jit_provisioning);
    }
    if let Some(configuration) = req.configuration {
        configuration
            .validate_for_provider(&draft.provider)
            .map_err(|_| AuthError::InvalidProvider {
                provider: Some(draft.provider.clone()),
            })?;
        active_model.configuration = Set(Some(
            serde_json::to_value(configuration)
                .map_err(|_| AuthError::InvalidCallbackParameters)?,
        ));
    }
    active_model.updated_at = Set(Utc::now());
    let updated_model = active_model
        .update(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("Db update error {:?}", e);
            AuthError::DbTimeout
        })?;
    if original_provider != updated_model.provider {
        app_state.remove_oidc_provider(&original_provider).await;
    }
    if let Ok(client_secret) = decrypt_key(
        &app_state.settings.auth.app_key,
        &updated_model.client_secret,
    ) {
        let allowed_domains = updated_model
            .allowed_domains
            .iter()
            .map(|d| d.into())
            .collect();
        let _ = app_state
            .settings
            .load_sso_provider_in_state(
                &updated_model.provider,
                &client_secret,
                &updated_model.client_id,
                &updated_model.redirect_url,
                updated_model.tenant_id.as_ref(),
                updated_model.is_enabled,
                allowed_domains,
                updated_model.use_grengin_proxy,
                updated_model.jit_provisioning,
            )
            .await;
    }
    app_state
        .refresh_oidc_client(&updated_model.provider)
        .await
        .map_err(|error| {
            eprintln!("OIDC provider cache refresh failed: {error:?}");
            AuthError::ServiceTemporarilyUnavailable
        })?;
    let response = provider_response(&app_state, updated_model);
    Ok((StatusCode::OK, Json(response)))
}

// ─── Grengin SSO Proxy ────────────────────────────────────────────────────────

/// POST /admin/sso-providers/:provider_id/quick-setup
///
/// Activate the Grengin SSO proxy for a provider. The instance does not need
/// local OAuth client credentials. OAuth exchange happens on the Grengin worker
/// and the callback is relayed back to this instance with a signed assertion.
///
/// The admin only needs to supply:
///   - `allowed_domains` — email domains permitted to sign in (e.g. ["acme.com"])
///   - `tenant_id`       — Azure only: the directory tenant (defaults to "common")
pub async fn quick_setup_grengin_proxy(
    claims: Claims,
    Path(provider_id): Path<Uuid>,
    State(app_state): State<SharedState>,
    Json(body): Json<GrenginProxySetupRequest>,
) -> Result<StatusCode, AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_SSO_PROVIDERS_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let model = sso_providers::Entity::find_by_id(provider_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error fetching provider: {e:?}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    let proxy_client_id = "managed-by-grengin-proxy".to_string();
    let proxy_client_secret_plain = "managed-by-grengin-proxy".to_string();

    let proxy_secret_encrypted = encrypt_key(
        &app_state.settings.auth.app_key,
        proxy_client_secret_plain.as_bytes(),
    )
    .map_err(|e| {
        eprintln!("encryption error: {e:?}");
        AuthError::ServiceTemporarilyUnavailable
    })?;

    let app_redirect_url =
        std::env::var("REDIRECT_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let proxy_redirect_url = format!(
        "{}/auth/{}/callback",
        app_redirect_url.trim_end_matches('/'),
        model.provider
    );

    let tenant_id = match model.provider.as_str() {
        "azure" => Some(
            body.tenant_id
                .clone()
                .unwrap_or_else(|| "common".to_string()),
        ),
        _ => None,
    };

    let now = Utc::now();
    let mut active = model.into_active_model();
    active.client_id = Set(proxy_client_id);
    active.client_secret = Set(proxy_secret_encrypted);
    active.redirect_url = Set(proxy_redirect_url.clone());
    active.allowed_domains = Set(body.allowed_domains.clone());
    active.is_enabled = Set(true);
    active.use_grengin_proxy = Set(true);
    active.updated_at = Set(now);
    if let Some(ref tid) = tenant_id {
        active.tenant_id = Set(Some(tid.clone()));
    }

    let saved = active.update(&app_state.database).await.map_err(|e| {
        eprintln!("db update error: {e:?}");
        AuthError::DbTimeout
    })?;

    let _ = app_state
        .settings
        .load_sso_provider_in_state(
            saved.provider.clone(),
            proxy_client_secret_plain,
            saved.client_id.clone(),
            proxy_redirect_url,
            tenant_id,
            true,
            body.allowed_domains,
            true,
            saved.jit_provisioning,
        )
        .await;
    app_state
        .refresh_oidc_client(&saved.provider)
        .await
        .map_err(|error| {
            eprintln!("OIDC proxy provider cache refresh failed: {error:?}");
            AuthError::ServiceTemporarilyUnavailable
        })?;

    Ok(StatusCode::OK)
}
