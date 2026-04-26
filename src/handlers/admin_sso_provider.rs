use crate::{
    auth::{
        claims::Claims,
        encryption::{decrypt_key, encrypt_key},
        error::{AuthError, Error},
        permissions::{PERMISSION_SSO_PROVIDERS_MANAGE, PERMISSION_SSO_PROVIDERS_VIEW},
        sso_provider::{is_editable, sso_providers_list},
    },
    dto::admin_sso_providers::{
        EditableField, SsoProvider, SsoProviderEditable, SsoProviderUpdate,
    },
    models::sso_providers,
    services::authorization::{AuthorizationService, PermissionScopeMode},
    state::SharedState,
};
use axum::{
    extract::{Path, State},
    Json,
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
    is_enabled: bool,
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
    let app_redirect_url =
        std::env::var("REDIRECT_URL").unwrap_or("http://localhost:8080".to_string());
    match template.provider.as_str() {
        "google" => {
            let client_id = read_non_empty_env(&["GOOGLE_CLIENT_ID", "GOOGLE_CLIENT"]);
            let client_secret = read_non_empty_env(&["GOOGLE_CLIENT_SECRET"]);
            let is_enabled = client_id.is_some() && client_secret.is_some();
            SsoProviderSeed {
                provider: template.provider,
                name: template.name,
                tenant_id: None,
                client_id: client_id.unwrap_or_else(|| EMPTY_VALUE.to_string()),
                client_secret: client_secret.unwrap_or_else(|| EMPTY_VALUE.to_string()),
                issuer_url: "https://accounts.google.com".to_string(),
                redirect_url: format!("{}/auth/google/callback", app_redirect_url),
                allowed_domains: Vec::new(),
                is_enabled,
            }
        }
        "azure" => {
            let client_id = read_non_empty_env(&["AZURE_CLIENT_ID"]);
            let client_secret = read_non_empty_env(&["AZURE_CLIENT_SECRET"]);
            let tenant_id =
                read_non_empty_env(&["AZURE_TENANT_ID"]).unwrap_or_else(|| "common".to_string());
            let is_enabled = client_id.is_some() && client_secret.is_some();
            SsoProviderSeed {
                provider: template.provider,
                name: template.name,
                tenant_id: Some(tenant_id.clone()),
                client_id: client_id.unwrap_or_else(|| EMPTY_VALUE.to_string()),
                client_secret: client_secret.unwrap_or_else(|| EMPTY_VALUE.to_string()),
                issuer_url: format!("https://login.microsoftonline.com/{tenant_id}/v2.0"),
                redirect_url: format!("{}/auth/azure/callback", app_redirect_url),
                allowed_domains: Vec::new(),
                is_enabled,
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
            is_enabled: false,
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
    if !seed.is_enabled {
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
    let allowed_domains = seed.allowed_domains.iter().map(|d| d.as_str()).collect();
    let _ = app_state
        .settings
        .load_sso_provider_in_state(
            seed.provider.as_str(),
            seed.client_secret.as_str(),
            seed.client_id.as_str(),
            seed.redirect_url.as_str(),
            seed.tenant_id.as_deref(),
            seed.is_enabled,
            allowed_domains,
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
            if seed.is_enabled && model_needs_env_backfill(app_state, &models[index]) {
                let mut active_model = models[index].clone().into_active_model();
                active_model.name = Set(seed.name.clone());
                active_model.tenant_id = Set(seed.tenant_id.clone());
                active_model.client_id = Set(seed.client_id.clone());
                active_model.client_secret = Set(encrypted_seed_secret(app_state, &seed)?);
                active_model.issuer_url = Set(seed.issuer_url.clone());
                active_model.redirect_url = Set(seed.redirect_url.clone());
                active_model.allowed_domains = Set(seed.allowed_domains.clone());
                active_model.is_enabled = Set(true);
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
        .map(|model| SsoProvider {
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
            created_at: model.created_at,
            updated_at: model.updated_at,
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
                created_at: model.created_at,
                updated_at: model.updated_at,
            }
        })
        .ok_or(AuthError::ResourceNotFound)?;
    Ok((StatusCode::OK, Json(response)))
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
    let mut active_model = model.into_active_model();
    if let Some(provider) = req.provider {
        active_model.provider = Set(provider);
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
    if let Some(redirect_url) = req.redirect_url {
        active_model.redirect_url = Set(redirect_url);
    }
    active_model.tenant_id = Set(req.tenant_id);
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
            )
            .await;
        let _ = app_state.refresh_oidc_client(&updated_model.provider).await;
    }
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
        created_at: updated_model.created_at,
        updated_at: updated_model.updated_at,
    };
    Ok((StatusCode::OK, Json(response)))
}
