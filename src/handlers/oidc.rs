// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::{
        error::{AuthError, Error},
        provider_config::OidcProviderConfiguration,
        sso_proxy::build_proxy_authorize_url,
    },
    dto::{
        auth::AuthToken,
        oauth::{
            AuthCallback, AuthProvider, AuthProviderSummary, CallbackExchangeMode, StartParams,
        },
    },
    models::{oauth_sessions, sso_providers},
    services::{oidc_proxy::provider_uses_proxy, oidc_service::oidc_oauth_callback},
    state::SharedState,
    utils::uri::is_azure_mobile_redirect_uri,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::Redirect,
};
use chrono::Utc;
use openidconnect::{
    CsrfToken, Nonce, PkceCodeChallenge, RedirectUrl, Scope, core::CoreAuthenticationFlow,
};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter, QueryOrder,
};
use std::borrow::Cow;

#[utoipa::path(
    get,
    path = "/auth/providers",
    tag = "auth",
    responses(
        (status = 200, body = Vec<AuthProviderSummary>),
        (status = 503, content_type = "application/json", body = Error, description = "Authentication configuration unavailable"),
    )
)]
pub async fn list_auth_providers(
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<Vec<AuthProviderSummary>>), AuthError> {
    let models = sso_providers::Entity::find()
        .filter(sso_providers::Column::IsEnabled.eq(true))
        .order_by_asc(sso_providers::Column::Name)
        .all(&app_state.database)
        .await
        .map_err(|error| {
            eprintln!("enabled auth provider lookup failed: {error:?}");
            AuthError::ServiceTemporarilyUnavailable
        })?;
    let mut providers = Vec::new();
    for model in models {
        let Some(runtime) = app_state.oidc_provider(&model.provider).await else {
            continue;
        };
        if !runtime.is_enabled {
            continue;
        }
        let Ok(configuration) = OidcProviderConfiguration::from_value(model.configuration.as_ref())
        else {
            continue;
        };
        providers.push(AuthProviderSummary {
            login_path: format!("/auth/{}", model.provider),
            provider: model.provider,
            name: model.name,
            auto_redirect: configuration.auto_redirect,
        });
    }
    Ok((StatusCode::OK, Json(providers)))
}

#[utoipa::path(
    get,
    path = "/auth/{provider}",
    tag = "auth",
    operation_id = "initiateAuth",
    params(
        ("provider" = String, Path, description = "Auth provider identifier (e.g., google, azure, keycloak)"),
        ("redirect_uri" = Option<String>, Query, description = "Optional post-login redirect target", format = "uri")),
    responses(
        (status = 400, content_type = "application/json", body = Error, description = "Invalid auth provider (code=6200)"),
        (status = 400, content_type = "application/json", body = Error, description = "Invalid redirect URI (code=6202)"),
        (status = 401, content_type = "application/json", body = Error, description = "Account deactivated (code=6105)"),
        (status = 403, content_type = "application/json", body = Error, description = "SSO provider disabled by admin (code=6401)"),
        (status = 404, content_type = "application/json", body = Error, description = "Email does not exist (code=6101)"),
        (status = 404, content_type = "application/json", body = Error, description = "Organization not found (code=6301)"),
        (status = 404, content_type = "application/json", body = Error, description = "DB not found (code=5003)"),
        (status = 409, content_type = "application/json", body = Error, description = "SSO provider not configured (code=6400)"),
        (status = 503, content_type = "application/json", body = Error, description = "Auth service temporarily unavailable (code=6000)"),
        (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000)"),
    )
)]
pub async fn oidc_login_start(
    Path(provider): Path<AuthProvider>,
    Query(query): Query<StartParams>,
    State(app_state): State<SharedState>,
) -> Result<Redirect, AuthError> {
    let is_enabled = app_state
        .check_sso_provider_is_enabled(&provider)
        .await
        .ok_or(AuthError::InvalidProvider {
            provider: Some(provider.clone()),
        })?;
    if !is_enabled {
        return Err(AuthError::SsoProviderDisabledByAdmin {
            provider: Some(provider.clone()),
        });
    }
    let runtime = app_state
        .get_oidc_provider_runtime(&provider)
        .await
        .map_err(|_| AuthError::InvalidProvider {
            provider: Some(provider.clone()),
        })?;
    let default_redirect_uri = Some(runtime.redirect_url.clone());

    // Accept a caller-supplied redirect_uri only when it exactly matches the
    // configured value or is a recognised mobile scheme (msauth://).  Arbitrary
    // redirect_uris are an open-redirect / CSRF vector: an attacker could steer
    // the OAuth dance to their own endpoint, capture the assertion JWT, and then
    // replay it against the real instance.
    let redirect_uri_value = match &query.redirect_uri {
        Some(provided) => {
            let is_mobile = is_azure_mobile_redirect_uri(&provider, provided);
            let matches_configured = default_redirect_uri.as_deref() == Some(provided.as_str());
            if !is_mobile && !matches_configured {
                return Err(AuthError::InvalidRedirectUri {
                    redirect_uri: Some(provided.clone()),
                });
            }
            provided.clone()
        }
        None => default_redirect_uri.ok_or(AuthError::SsoProviderNotConfigured {
            provider: Some(provider.clone()),
        })?,
    };
    let redirect_uri = RedirectUrl::new(redirect_uri_value.clone()).map_err(|_| {
        AuthError::InvalidRedirectUri {
            redirect_uri: query.redirect_uri.clone(),
        }
    })?;

    let use_proxy = provider_uses_proxy(&app_state, &provider).await;
    if use_proxy {
        let csrf_state = CsrfToken::new_random();
        let nonce = Nonce::new_random();
        let proxy_authorize_url = build_proxy_authorize_url(
            &provider,
            redirect_uri.as_str(),
            csrf_state.secret(),
            nonce.secret(),
        )
        .ok_or(AuthError::ServiceTemporarilyUnavailable)?;
        let sess = oauth_sessions::ActiveModel {
            state: Set(csrf_state.secret().to_string()),
            pkce_verifier: Set("proxy-managed".to_string()),
            nonce: Set(nonce.secret().to_string()),
            redirect_uri: Set(Some(redirect_uri.to_string())),
            created_at: Set(Utc::now()),
        };
        sess.insert(&app_state.database).await.map_err(|e| {
            eprintln!("{:?}", e);
            AuthError::ServiceTemporarilyUnavailable
        })?;
        return Ok(Redirect::to(&proxy_authorize_url));
    }

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let oidc_client = runtime.client.ok_or(AuthError::SsoProviderNotConfigured {
        provider: Some(provider.clone()),
    })?;
    let mut authorization = oidc_client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .set_redirect_uri(Cow::Owned(redirect_uri.clone()))
        .set_pkce_challenge(pkce_challenge);
    for scope in runtime.configuration.scopes {
        if scope != "openid" {
            authorization = authorization.add_scope(Scope::new(scope));
        }
    }
    for (key, value) in runtime.configuration.authorization_params {
        authorization = authorization.add_extra_param(key, value);
    }
    let (auth_url, csrf_state, nonce) = authorization.url();
    let sess = oauth_sessions::ActiveModel {
        state: Set(csrf_state.secret().to_string()),
        pkce_verifier: Set(pkce_verifier.secret().to_string()),
        nonce: Set(nonce.secret().to_string()),
        redirect_uri: Set(Some(redirect_uri.to_string())),
        created_at: Set(Utc::now()),
    };
    sess.insert(&app_state.database).await.map_err(|e| {
        eprintln!("{:?}", e);
        AuthError::ServiceTemporarilyUnavailable
    })?;
    Ok(Redirect::to(auth_url.as_str()))
}

#[utoipa::path(
    get,
    path = "/auth/{provider}/callback",
    tag = "auth",
    operation_id = "authCallback",
    params(
        ("provider" = String, Path, description = "Auth provider identifier (e.g., google, azure, keycloak)"),
        ("code" = String, Query, description = "Authorization code from provider"),
        ("assertion" = Option<String>, Query, description = "Signed identity assertion from Grengin SSO proxy"),
        ("state" = String, Query, description = "CSRF state"),
        ("error" = Option<String>, Query, description = "Error code from provider"),
        ("error_description" = Option<String>, Query, description = "Error description from provider")
    ),
    responses(
        (status = 400, content_type = "application/json", body = Error, description = "Missing credentials (code=6102)"),
        (status = 400, content_type = "application/json", body = Error, description = "Invalid auth provider (code=6200)"),
        (status = 400, content_type = "application/json", body = Error, description = "Invalid callback parameters (code=6201)"),
        (status = 400, content_type = "application/json", body = Error, description = "Invalid redirect URI (code=6202)"),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid credentials (code=6100)"),
        (status = 401, content_type = "application/json", body = Error, description = "Account deactivated (code=6105)"),
        (status = 401, content_type = "application/json", body = Error, description = "Email domain not allowed (code=6303)"),
        (status = 403, content_type = "application/json", body = Error, description = "SSO provider disabled by admin (code=6401)"),
        (status = 404, content_type = "application/json", body = Error, description = "Email does not exist (code=6101)"),
        (status = 404, content_type = "application/json", body = Error, description = "DB not found (code=5003)"),
        (status = 409, content_type = "application/json", body = Error, description = "Email already exists (code=6106)"),
        (status = 409, content_type = "application/json", body = Error, description = "SSO provider not configured (code=6400)"),
        (status = 503, content_type = "application/json", body = Error, description = "Auth service temporarily unavailable (code=6000)"),
        (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000)"),
    )
)]
pub async fn oidc_oauth_callback_get(
    Path(provider): Path<AuthProvider>,
    Query(cb): Query<AuthCallback>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<AuthToken>), AuthError> {
    oidc_oauth_callback(provider, cb, app_state, CallbackExchangeMode::Auto).await
}

#[utoipa::path(
    post,
    path = "/auth/{provider}/callback",
    tag = "auth",
    operation_id = "authCallbackPost",
    params(
        ("provider" = String, Path, description = "Auth provider identifier (e.g., google, azure, keycloak)"),
    ),
    request_body(content = AuthCallback, description = "OAuth callback parameters"),
    responses(
        (status = 400, content_type = "application/json", body = Error, description = "Missing credentials (code=6102)"),
        (status = 400, content_type = "application/json", body = Error, description = "Invalid auth provider (code=6200)"),
        (status = 400, content_type = "application/json", body = Error, description = "Invalid callback parameters (code=6201)"),
        (status = 400, content_type = "application/json", body = Error, description = "Invalid redirect URI (code=6202)"),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid credentials (code=6100)"),
        (status = 401, content_type = "application/json", body = Error, description = "Account deactivated (code=6105)"),
        (status = 401, content_type = "application/json", body = Error, description = "Email domain not allowed (code=6303)"),
        (status = 403, content_type = "application/json", body = Error, description = "SSO provider disabled by admin (code=6401)"),
        (status = 404, content_type = "application/json", body = Error, description = "Email does not exist (code=6101)"),
        (status = 404, content_type = "application/json", body = Error, description = "DB not found (code=5003)"),
        (status = 409, content_type = "application/json", body = Error, description = "Email already exists (code=6106)"),
        (status = 409, content_type = "application/json", body = Error, description = "SSO provider not configured (code=6400)"),
        (status = 503, content_type = "application/json", body = Error, description = "Auth service temporarily unavailable (code=6000)"),
        (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000)"),
    )
)]
pub async fn oidc_oauth_callback_post(
    Path(provider): Path<AuthProvider>,
    State(app_state): State<SharedState>,
    Json(cb): Json<AuthCallback>,
) -> Result<(StatusCode, Json<AuthToken>), AuthError> {
    oidc_oauth_callback(provider, cb, app_state, CallbackExchangeMode::Auto).await
}

#[utoipa::path(
    get,
    path = "/auth/azure/mobile/callback",
    tag = "auth",
    operation_id = "azureMobileAuthCallback",
    params(
        ("code" = String, Query, description = "Authorization code from Azure"),
        ("state" = String, Query, description = "CSRF state"),
        ("error" = Option<String>, Query, description = "Error code from Azure"),
        ("error_description" = Option<String>, Query, description = "Error description from Azure")
    ),
    responses(
        (status = 200, body = AuthToken),
        (status = 400, content_type = "application/json", body = Error, description = "Invalid callback parameters or redirect URI"),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid credentials/account state/domain"),
        (status = 409, content_type = "application/json", body = Error, description = "SSO provider not configured"),
        (status = 503, content_type = "application/json", body = Error, description = "Auth service or DB unavailable"),
    )
)]
pub async fn azure_mobile_oauth_callback_get(
    Query(cb): Query<AuthCallback>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<AuthToken>), AuthError> {
    oidc_oauth_callback(
        "azure".to_string(),
        cb,
        app_state,
        CallbackExchangeMode::AzureMobilePublic,
    )
    .await
}

#[utoipa::path(
    post,
    path = "/auth/azure/mobile/callback",
    tag = "auth",
    operation_id = "azureMobileAuthCallbackPost",
    request_body(content = AuthCallback, description = "Azure mobile OAuth callback parameters"),
    responses(
        (status = 200, body = AuthToken),
        (status = 400, content_type = "application/json", body = Error, description = "Invalid callback parameters or redirect URI"),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid credentials/account state/domain"),
        (status = 409, content_type = "application/json", body = Error, description = "SSO provider not configured"),
        (status = 503, content_type = "application/json", body = Error, description = "Auth service or DB unavailable"),
    )
)]
pub async fn azure_mobile_oauth_callback_post(
    State(app_state): State<SharedState>,
    Json(cb): Json<AuthCallback>,
) -> Result<(StatusCode, Json<AuthToken>), AuthError> {
    oidc_oauth_callback(
        "azure".to_string(),
        cb,
        app_state,
        CallbackExchangeMode::AzureMobilePublic,
    )
    .await
}
