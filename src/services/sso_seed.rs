// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::{
        encryption::{decrypt_key, encrypt_key},
        error::AuthError,
        sso_provider::sso_providers_list,
    },
    dto::admin_sso_providers::SsoProviderTemplate,
    models::sso_providers,
    state::SharedState,
};
use chrono::Utc;
use sea_orm::ActiveValue::Set;
use sea_orm::{ActiveModelTrait, EntityTrait, IntoActiveModel};
use uuid::Uuid;

pub const EMPTY_VALUE: &str = "<empty>";

pub struct SsoProviderSeed {
    pub provider: String,
    pub name: String,
    pub tenant_id: Option<String>,
    pub client_id: String,
    pub client_secret: String,
    pub issuer_url: String,
    pub redirect_url: String,
    pub allowed_domains: Vec<String>,
    pub has_credentials: bool,
    pub is_enabled: bool,
    pub use_grengin_proxy: bool,
    pub jit_provisioning: bool,
}

pub fn read_non_empty_env(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

pub fn env_seed_for_template(template: SsoProviderTemplate) -> SsoProviderSeed {
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
        .map(|v| {
            !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "false" | "0" | "no" | "off"
            )
        })
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

pub fn encrypted_secret_is_configured(app_state: &SharedState, encrypted_secret: &str) -> bool {
    decrypt_key(&app_state.settings.auth.app_key, encrypted_secret)
        .map(|secret| !secret.trim().is_empty())
        .unwrap_or(false)
}

pub fn model_needs_env_backfill(app_state: &SharedState, model: &sso_providers::Model) -> bool {
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

pub fn encrypted_seed_secret(
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

pub async fn load_seed_in_state(app_state: &SharedState, seed: &SsoProviderSeed) {
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

pub async fn ensure_sso_providers_from_env(
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

pub fn grengin_proxy_available_for_provider(provider: &str) -> bool {
    matches!(provider, "google" | "azure")
}
