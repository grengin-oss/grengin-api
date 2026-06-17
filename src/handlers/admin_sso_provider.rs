use crate::{
    auth::{
        claims::Claims,
        encryption::{decrypt_key, encrypt_key},
        error::{AuthError, Error},
        permissions::{PERMISSION_SSO_PROVIDERS_MANAGE, PERMISSION_SSO_PROVIDERS_VIEW},
        sso_provider::{is_editable, sso_providers_list},
    },
    dto::admin_sso_providers::{
        EditableField, GrenginProxySetupRequest, SsoProvider, SsoProviderEditable,
        SsoProviderUpdate, SsoProviderValidationRequest, SsoProviderValidationResponse,
    },
    models::sso_providers,
    services::{
        authorization::{AuthorizationService, PermissionScopeMode},
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
use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait, IntoActiveModel};
use uuid::Uuid;

const EMPTY_VALUE: &str = "<empty>";

struct SsoProviderSeed {
    provider: String,
    name: String,
    tenant_id: Option<String>,
    client_id: String,
    client_secret: String,
    issuer_url: String,
    redirect_url: String,
    allowed_domains: Vec<String>,
    has_credentials: bool,
    is_enabled: bool,
    use_grengin_proxy: bool,
    jit_provisioning: bool,
}

fn read_non_empty_env(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn env_seed_for_template(
    template: crate::dto::admin_sso_providers::SsoProviderTemplate,
) -> SsoProviderSeed {
    let proxy_auto_enabled = std::env::var("SSO_PROXY_AUTO_ENABLE")
        .or_else(|_| std::env::var("SSO_PROXY_ENABLED"))
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "y" | "on"
            )
        })
        .unwrap_or(false);
    let proxy_jit = std::env::var("GRENGIN_PROXY_JIT_PROVISIONING")
        .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "false" | "0" | "no" | "off"))
        .unwrap_or(false);
    let proxy_allowed_domains: Vec<String> = std::env::var("GRENGIN_PROXY_ALLOWED_DOMAINS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let app_redirect_url =
        std::env::var("REDIRECT_URL").unwrap_or("http://localhost:8080".to_string());
    match template.provider.as_str() {
        "google" => {
            if proxy_auto_enabled {
                return SsoProviderSeed {
                    provider: template.provider,
                    name: template.name,
                    tenant_id: None,
                    client_id: "managed-by-grengin-proxy".to_string(),
                    client_secret: "managed-by-grengin-proxy".to_string(),
                    issuer_url: "https://accounts.google.com".to_string(),
                    redirect_url: format!("{}/auth/google/callback", app_redirect_url),
                    allowed_domains: proxy_allowed_domains.clone(),
                    has_credentials: true,
                    is_enabled: true,
                    use_grengin_proxy: true,
                    jit_provisioning: proxy_jit,
                };
            }

            let client_id = read_non_empty_env(&["GOOGLE_CLIENT_ID", "GOOGLE_CLIENT"]);
            let client_secret = read_non_empty_env(&["GOOGLE_CLIENT_SECRET"]);
            let has_credentials = client_id.is_some() && client_secret.is_some();
            SsoProviderSeed {
                provider: template.provider,
                name: template.name,
                tenant_id: None,
                client_id: client_id.unwrap_or_else(|| EMPTY_VALUE.to_string()),
                client_secret: client_secret.unwrap_or_else(|| EMPTY_VALUE.to_string()),
                issuer_url: "https://accounts.google.com".to_string(),
                redirect_url: format!("{}/auth/google/callback", app_redirect_url),
                allowed_domains: Vec::new(),
                has_credentials,
                is_enabled: false,
                use_grengin_proxy: false,
                jit_provisioning: true,
            }
        }
        "azure" => {
            if proxy_auto_enabled {
                let tenant_id = read_non_empty_env(&["GRENGIN_PROXY_AZURE_TENANT_ID"])
                    .or_else(|| read_non_empty_env(&["AZURE_TENANT_ID"]))
                    .unwrap_or_else(|| "common".to_string());
                return SsoProviderSeed {
                    provider: template.provider,
                    name: template.name,
                    tenant_id: Some(tenant_id.clone()),
                    client_id: "managed-by-grengin-proxy".to_string(),
                    client_secret: "managed-by-grengin-proxy".to_string(),
                    issuer_url: format!("https://login.microsoftonline.com/{tenant_id}/v2.0"),
                    redirect_url: format!("{}/auth/azure/callback", app_redirect_url),
                    allowed_domains: proxy_allowed_domains.clone(),
                    has_credentials: true,
                    is_enabled: true,
                    use_grengin_proxy: true,
                    jit_provisioning: proxy_jit,
                };
            }

            let client_id = read_non_empty_env(&["AZURE_CLIENT_ID"]);
            let client_secret = read_non_empty_env(&["AZURE_CLIENT_SECRET"]);
            let tenant_id =
                read_non_empty_env(&["AZURE_TENANT_ID"]).unwrap_or_else(|| "common".to_string());
            let has_credentials = client_id.is_some() && client_secret.is_some();
            SsoProviderSeed {
                provider: template.provider,
                name: template.name,
                tenant_id: Some(tenant_id.clone()),
                client_id: client_id.unwrap_or_else(|| EMPTY_VALUE.to_string()),
                client_secret: client_secret.unwrap_or_else(|| EMPTY_VALUE.to_string()),
                issuer_url: format!("https://login.microsoftonline.com/{tenant_id}/v2.0"),
                redirect_url: format!("{}/auth/azure/callback", app_redirect_url),
                allowed_domains: Vec::new(),
                has_credentials,
                is_enabled: false,
                use_grengin_proxy: false,
                jit_provisioning: true,
            }
        }
        _ => SsoProviderSeed {
            provider: template.provider,
            name: template.name,
            tenant_id: template.tenant_id,
            client_id: EMPTY_VALUE.to_string(),
            client_secret: EMPTY_VALUE.to_string(),
            issuer_url: template.issuer_url,
            redirect_url: template.redirect_url,
            allowed_domains: Vec::new(),
            has_credentials: false,
            is_enabled: false,
            use_grengin_proxy: false,
            jit_provisioning: true,
        },
    }
}

fn encrypted_secret_is_configured(app_state: &SharedState, encrypted_secret: &str) -> bool {
    decrypt_key(&app_state.settings.auth.app_key, encrypted_secret)
        .map(|secret| !secret.trim().is_empty())
        .unwrap_or(false)
}

fn model_needs_env_backfill(app_state: &SharedState, model: &sso_providers::Model) -> bool {
    let client_id_missing = model.client_id.trim().is_empty() || model.client_id == EMPTY_VALUE;
    let untouched_placeholder = model
        .updated_at
        .signed_duration_since(model.created_at)
        .num_seconds()
        .abs()
        <= 1;
    client_id_missing
        && !model.is_enabled
        && untouched_placeholder
        && !encrypted_secret_is_configured(app_state, &model.client_secret)
}

fn encrypted_seed_secret(
    app_state: &SharedState,
    seed: &SsoProviderSeed,
) -> Result<String, AuthError> {
    if !seed.has_credentials {
        return Ok(EMPTY_VALUE.to_string());
    }
    encrypt_key(
        &app_state.settings.auth.app_key,
        seed.client_secret.as_bytes(),
    )
    .map_err(|e| {
        eprintln!("Sso key encryption error {:?}", e);
        AuthError::DbTimeout
    })
}

async fn load_seed_in_state(app_state: &SharedState, seed: &SsoProviderSeed) {
    if !seed.is_enabled {
        return;
    }
    let _ = app_state
        .settings
        .load_sso_provider_in_state(
            seed.provider.clone(),
            seed.client_secret.clone(),
            seed.client_id.clone(),
            seed.redirect_url.clone(),
            seed.tenant_id.clone(),
            seed.is_enabled,
            seed.allowed_domains.clone(),
            seed.use_grengin_proxy,
            seed.jit_provisioning,
        )
        .await;
    let _ = app_state.refresh_oidc_client(&seed.provider).await;
}

async fn ensure_sso_providers_from_env(
    app_state: &SharedState,
    mut models: Vec<sso_providers::Model>,
) -> Result<Vec<sso_providers::Model>, AuthError> {
    let mut changed = false;
    let mut to_insert: Vec<(sso_providers::ActiveModel, SsoProviderSeed)> = Vec::new();

    for template in sso_providers_list() {
        let seed = env_seed_for_template(template);
        if let Some(index) = models
            .iter()
            .position(|model| model.provider == seed.provider)
        {
            let needs_backfill =
                seed.has_credentials && model_needs_env_backfill(app_state, &models[index]);
            // For proxy providers, always sync policy fields from env so that
            // allowed_domains and jit_provisioning are never frozen at initial seed values.
            let policy_changed = seed.use_grengin_proxy
                && (models[index].allowed_domains != seed.allowed_domains
                    || models[index].jit_provisioning != seed.jit_provisioning);
            if needs_backfill || policy_changed {
                let mut active_model = models[index].clone().into_active_model();
                if needs_backfill {
                    active_model.name = Set(seed.name.clone());
                    active_model.tenant_id = Set(seed.tenant_id.clone());
                    active_model.client_id = Set(seed.client_id.clone());
                    active_model.client_secret = Set(encrypted_seed_secret(app_state, &seed)?);
                    active_model.issuer_url = Set(seed.issuer_url.clone());
                    active_model.redirect_url = Set(seed.redirect_url.clone());
                    active_model.is_enabled = Set(seed.is_enabled);
                    active_model.use_grengin_proxy = Set(seed.use_grengin_proxy);
                }
                active_model.allowed_domains = Set(seed.allowed_domains.clone());
                active_model.jit_provisioning = Set(seed.jit_provisioning);
                active_model.updated_at = Set(Utc::now());
                let updated_model =
                    active_model
                        .update(&app_state.database)
                        .await
                        .map_err(|e| {
                            eprintln!("DB update sso provider from env error {:?}", e);
                            AuthError::DbTimeout
                        })?;
                models[index] = updated_model;
                load_seed_in_state(app_state, &seed).await;
                changed = true;
            }
            continue;
        }

        to_insert.push((
            sso_providers::ActiveModel {
                id: Set(Uuid::new_v4()),
                provider: Set(seed.provider.clone()),
                name: Set(seed.name.clone()),
                tenant_id: Set(seed.tenant_id.clone()),
                client_id: Set(seed.client_id.clone()),
                client_secret: Set(encrypted_seed_secret(app_state, &seed)?),
                issuer_url: Set(seed.issuer_url.clone()),
                redirect_url: Set(seed.redirect_url.clone()),
                allowed_domains: Set(seed.allowed_domains.clone()),
                is_enabled: Set(seed.is_enabled),
                is_default: Set(false),
                use_grengin_proxy: Set(seed.use_grengin_proxy),
                jit_provisioning: Set(seed.jit_provisioning),
                created_at: Set(Utc::now()),
                updated_at: Set(Utc::now()),
            },
            seed,
        ));
    }

    if !to_insert.is_empty() {
        sso_providers::Entity::insert_many(to_insert.iter().map(|(model, _)| model.clone()))
            .exec(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("DB insert sso providers from env error {:?}", e);
                AuthError::DbTimeout
            })?;
        for (_, seed) in &to_insert {
            load_seed_in_state(app_state, seed).await;
        }
        changed = true;
    }

    if changed {
        models = sso_providers::Entity::find()
            .all(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("Db get all error: {:?}", e);
                AuthError::DbTimeout
            })?;
    }

    Ok(models)
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
        .map(|model| {
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
                grengin_proxy_available,
                created_at: model.created_at,
                updated_at: model.updated_at,
            }
        })
        .collect();
    Ok((StatusCode::OK, Json(response)))
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
    )?;
    if has_sensitive_changes(&app_state, &model, &draft) {
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
    if let Some(provider) = req.provider {
        active_model.provider = Set(provider.to_lowercase());
    }
    if let Some(name) = req.name {
        active_model.name = Set(name);
    }
    if let Some(allowed_domains) = req.allowed_domains {
        active_model.allowed_domains = Set(allowed_domains);
    }
    if let Some(client_id) = req.client_id {
        active_model.client_id = Set(client_id);
    }
    if let Some(is_enabled) = req.is_enabled {
        active_model.is_enabled = Set(is_enabled);
    }
    if let Some(issuer_url) = req.issuer_url {
        active_model.issuer_url = Set(issuer_url);
    }
    if req.redirect_url.is_some() || req.frontend_hosted_url.is_some() {
        active_model.redirect_url = Set(draft.redirect_url.clone());
    }
    if let Some(tenant_id) = req.tenant_id {
        active_model.tenant_id = Set(Some(tenant_id));
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
    active_model.updated_at = Set(Utc::now());
    let updated_model = active_model
        .update(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("Db update error {:?}", e);
            AuthError::DbTimeout
        })?;
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
        let _ = app_state.refresh_oidc_client(&updated_model.provider).await;
    }
    let grengin_proxy_available = grengin_proxy_available_for_provider(&updated_model.provider);
    let response = SsoProvider {
        id: updated_model.id,
        provider: updated_model.provider,
        name: updated_model.name,
        client_id: updated_model.client_id,
        client_secret: app_state
            .get_decrypted_api_key_preview(&Some(updated_model.client_secret))
            .unwrap_or("<empty>".to_string()),
        issuer_url: updated_model.issuer_url,
        redirect_url: updated_model.redirect_url,
        tenant_id: updated_model.tenant_id,
        allowed_domains: updated_model.allowed_domains,
        is_enabled: updated_model.is_enabled,
        use_grengin_proxy: updated_model.use_grengin_proxy,
        jit_provisioning: updated_model.jit_provisioning,
        grengin_proxy_available,
        created_at: updated_model.created_at,
        updated_at: updated_model.updated_at,
    };
    Ok((StatusCode::OK, Json(response)))
}

// ─── Grengin SSO Proxy ────────────────────────────────────────────────────────

/// Returns true when provider can use Grengin's managed SSO proxy flow.
fn grengin_proxy_available_for_provider(provider: &str) -> bool {
    matches!(provider, "google" | "azure")
}

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
    let proxy_redirect_url =
        format!("{}/auth/{}/callback", app_redirect_url.trim_end_matches('/'), model.provider);

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
    let _ = app_state.refresh_oidc_client(&saved.provider).await;

    Ok(StatusCode::OK)
}
