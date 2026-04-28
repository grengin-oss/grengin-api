use crate::{
    auth::azure::build_azure_public_client,
    auth::error::AuthError,
    config::setting::OidcClient,
    dto::{
        auth::{AuthToken, TokenType, User},
        oauth::{AuthCallback, StartParams},
    },
    state::SharedState,
};
use crate::{
    auth::{
        claims::{Claiming as _, Claims, RefreshClaims},
        error::Error,
    },
    dto::oauth::AuthProvider,
    models::{
        oauth_sessions, roles, user_role_assignments,
        users::{self, UserStatus},
    },
    services::authorization::AuthorizationService,
};
use axum::http::StatusCode;
use axum::{
    Json,
    extract::{Path, Query, State},
    response::Redirect,
};
use chrono::Utc;
use openidconnect::TokenResponse as OidcTokenResponse;
use openidconnect::{
    AuthorizationCode, ClaimsVerificationError, CsrfToken, Nonce, OAuth2TokenResponse,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
    core::{CoreAuthenticationFlow, CoreUserInfoClaims},
};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, TryIntoModel,
};
use std::borrow::Cow;
use uuid::Uuid;

#[derive(Clone, Copy)]
enum CallbackExchangeMode {
    Auto,
    AzureMobilePublic,
}

fn is_azure_mobile_redirect_uri(provider: &AuthProvider, redirect_uri: &str) -> bool {
    provider.eq_ignore_ascii_case("azure")
        && redirect_uri
            .get(..9)
            .map(|scheme| scheme.eq_ignore_ascii_case("msauth://"))
            .unwrap_or(false)
}

async fn build_azure_public_client_for_redirect(
    app_state: &SharedState,
    redirect_uri: &str,
) -> Result<OidcClient, AuthError> {
    let azure = app_state.settings.azure.read().await.clone().ok_or(
        AuthError::SsoProviderNotConfigured {
            provider: Some("azure".to_string()),
        },
    )?;
    build_azure_public_client(
        &app_state.req_client,
        azure.client_id,
        redirect_uri.to_string(),
        azure.tenant_id,
    )
    .await
    .map_err(|e| {
        eprintln!("azure public oidc client build error: {e:?}");
        AuthError::ServiceTemporarilyUnavailable
    })
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
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (oidc_client, _, default_redirect_uri) = app_state
        .get_oidc_client_and_column_and_redirect_uri(&provider)
        .await
        .map_err(|_| AuthError::InvalidProvider {
            provider: Some(provider.clone()),
        })?;
    let redirect_uri = RedirectUrl::new(query.redirect_uri.clone().unwrap_or(
        default_redirect_uri.ok_or(AuthError::SsoProviderNotConfigured {
            provider: Some(provider.clone()),
        })?,
    ))
    .map_err(|_| AuthError::InvalidRedirectUri {
        redirect_uri: query.redirect_uri.clone(),
    })?;
    let (auth_url, csrf_state, nonce) = oidc_client
        .read()
        .await
        .as_ref()
        .ok_or(AuthError::SsoProviderNotConfigured {
            provider: Some(provider.clone()),
        })?
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .set_redirect_uri(Cow::Owned(redirect_uri.clone()))
        .add_scope(Scope::new("email".to_string()))
        .add_scope(Scope::new("profile".to_string()))
        .set_pkce_challenge(pkce_challenge)
        .url();

    let state_str = csrf_state.secret().to_string();
    let sess = oauth_sessions::ActiveModel {
        state: Set(state_str.into()),
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

async fn oidc_oauth_callback(
    provider: AuthProvider,
    cb: AuthCallback,
    app_state: SharedState,
    exchange_mode: CallbackExchangeMode,
) -> Result<(StatusCode, Json<AuthToken>), AuthError> {
    // Check for OAuth error responses
    if let Some(error) = cb.error {
        eprintln!("OAuth error: {} - {:?}", error, cb.error_description);
        return Err(AuthError::InvalidCallbackParameters);
    }
    if matches!(exchange_mode, CallbackExchangeMode::AzureMobilePublic)
        && !provider.eq_ignore_ascii_case("azure")
    {
        return Err(AuthError::InvalidProvider {
            provider: Some(provider.clone()),
        });
    }
    let code = cb.code.ok_or(AuthError::InvalidCallbackParameters)?;
    let (oidc_client_configured, column, default_redirect_uri) = app_state
        .get_oidc_client_and_column_and_redirect_uri(&provider)
        .await
        .map_err(|_| AuthError::InvalidProvider {
            provider: Some(provider.clone()),
        })?;
    let sess = oauth_sessions::Entity::find()
        .filter(oauth_sessions::Column::State.eq(Some(cb.state.to_owned())))
        .order_by_desc(oauth_sessions::Column::CreatedAt)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error while fetching session: {e:?}");
            AuthError::ServiceTemporarilyUnavailable
        })?
        .ok_or(AuthError::InvalidToken)?;
    let redirect_uri_value = sess
        .redirect_uri
        .clone()
        .unwrap_or(
            default_redirect_uri.ok_or(AuthError::SsoProviderNotConfigured {
                provider: Some(provider.clone()),
            })?,
        );
    let is_mobile_redirect = is_azure_mobile_redirect_uri(&provider, &redirect_uri_value);
    if matches!(exchange_mode, CallbackExchangeMode::AzureMobilePublic) && !is_mobile_redirect {
        return Err(AuthError::InvalidRedirectUri {
            redirect_uri: Some(redirect_uri_value),
        });
    }
    let redirect_uri = RedirectUrl::new(redirect_uri_value.clone()).map_err(|_| {
        AuthError::InvalidRedirectUri {
            redirect_uri: sess.redirect_uri.clone(),
        }
    })?;
    let active: oauth_sessions::ActiveModel = sess.clone().into();
    active.delete(&app_state.database).await.map_err(|e| {
        eprintln!("db error while deleting oauth_session: {e:?}");
        AuthError::ServiceTemporarilyUnavailable
    })?;
    let use_public_azure_client = is_mobile_redirect;
    let mut oidc_client =
        if use_public_azure_client {
            build_azure_public_client_for_redirect(&app_state, &redirect_uri_value).await?
        } else {
            oidc_client_configured.read().await.clone().ok_or(
                AuthError::SsoProviderNotConfigured {
                    provider: Some(provider.clone()),
                },
            )?
        };
    let token_resp = oidc_client
        .exchange_code(AuthorizationCode::new(code))
        .expect("Failed to get token response")
        .set_pkce_verifier(PkceCodeVerifier::new(sess.pkce_verifier.clone()))
        .set_redirect_uri(Cow::Owned(redirect_uri))
        .request_async(&app_state.req_client)
        .await
        .map_err(|e| {
            eprintln!("token exchange err: {e:?}");
            AuthError::ServiceTemporarilyUnavailable
        })?;
    let nonce = Nonce::new(sess.nonce.clone());
    let id_token = token_resp
        .id_token()
        .ok_or(AuthError::ServiceTemporarilyUnavailable)?;
    let claims = match {
        let verifier = oidc_client.id_token_verifier();
        id_token.claims(&verifier, &nonce)
    } {
        Ok(c) => c,
        Err(e) => {
            let should_refresh = matches!(
                e,
                ClaimsVerificationError::SignatureVerification(_) // includes NoMatchingKey, CryptoError, etc.
            );
            if !should_refresh {
                eprintln!("id_token claims verification failed (non-refreshable): {e:?}");
                return Err(AuthError::InvalidToken);
            }
            if use_public_azure_client {
                oidc_client =
                    build_azure_public_client_for_redirect(&app_state, &redirect_uri_value).await?;
            } else {
                app_state
                    .refresh_oidc_client(&provider)
                    .await
                    .map_err(|err| {
                        eprintln!("oidc client refresh error: {err:?}");
                        AuthError::ServiceTemporarilyUnavailable
                    })?;

                oidc_client = oidc_client_configured.read().await.clone().ok_or(
                    AuthError::SsoProviderNotConfigured {
                        provider: Some(provider.clone()),
                    },
                )?;
            }

            let verifier2 = oidc_client.id_token_verifier();
            id_token.claims(&verifier2, &nonce).map_err(|e2| {
                eprintln!("id_token claims verification failed after refresh: {e2:?}");
                AuthError::InvalidToken
            })?
        }
    };
    let sub = claims.subject().as_str().to_string();
    let mut email = claims.email().map(|e| e.as_str().to_string());
    let mut display_name: Option<String> = None;
    let picture = claims
        .picture()
        .and_then(|pic_claim| pic_claim.get(None)) // default locale
        .map(|url| url.as_str().to_owned());
    let hd = claims
        .website()
        .and_then(|website_claim| website_claim.get(None)) // default locale
        .map(|url| url.as_str().to_owned());
    let google_id = if provider == "google" {
        Some(sub.clone())
    } else {
        None
    };
    let azure_id = if provider == "azure" {
        Some(sub.clone())
    } else {
        None
    };
    if email.is_none() {
        let info: CoreUserInfoClaims = oidc_client
            .user_info(token_resp.access_token().to_owned(), None)
            .expect("userinfo req")
            .request_async(&app_state.req_client)
            .await
            .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
        email = info.email().map(|e| e.as_str().to_string());
        if display_name.is_none() {
            display_name = info.name().and_then(|n| n.get(None).map(|s| s.to_string()));
        }
    } else {
        display_name = claims
            .name()
            .and_then(|n| n.get(None).map(|s| s.to_string()));
    }
    if let Some(email) = email.as_ref() {
        let (is_allowed, domain) = app_state.is_email_domain_allowed(email, &provider).await;
        if !is_allowed {
            return Err(AuthError::EmailDomainNotAllowed { domain });
        }
    }
    let mut user = users::Entity::find()
        .filter(column.eq(Some(sub.clone())))
        .filter(users::Column::Status.ne(UserStatus::Deleted))
        .order_by_desc(users::Column::CreatedAt)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error while fetching user: {e:?}");
            AuthError::ServiceTemporarilyUnavailable
        })?;
    if let Some(u) = &user {
        match &u.status {
            UserStatus::Deactivated | UserStatus::Suspended => {
                return Err(AuthError::AccountDeactivated);
            }
            _ => (),
        }
        let mut active_user: users::ActiveModel = u.clone().into();
        active_user.last_login_at = Set(Utc::now());
        active_user.update(&app_state.database).await.map_err(|e| {
            eprintln!("db error while updating user {:?}", e);
            AuthError::ServiceTemporarilyUnavailable
        })?;
    }
    if user.is_none() {
        if let Some(ref em) = email {
            user = users::Entity::find()
                .filter(users::Column::Email.eq(em))
                .filter(users::Column::Status.ne(UserStatus::Deleted))
                .one(&app_state.database)
                .await
                .map_err(|e| {
                    eprintln!("{:?}", e);
                    AuthError::ServiceTemporarilyUnavailable
                })?;

            if let Some(u) = &user {
                match &u.status {
                    UserStatus::Deactivated | UserStatus::Suspended => {
                        return Err(AuthError::AccountDeactivated);
                    }
                    _ => (),
                }
                let mut active_user: users::ActiveModel = u.clone().into();
                active_user.google_id = Set(google_id.clone());
                active_user.azure_id = Set(azure_id.clone());
                active_user.updated_at = Set(Utc::now());
                active_user.last_login_at = Set(Utc::now());
                active_user.update(&app_state.database).await.map_err(|e| {
                    eprintln!("db error while updating user {:?}", e);
                    AuthError::ServiceTemporarilyUnavailable
                })?;
            }
        }
    }
    if user.is_none() {
        let is_first_user = users::Entity::find()
            .count(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("db error while counting users {:?}", e);
                AuthError::ServiceTemporarilyUnavailable
            })?
            == 0;
        let new_user = users::ActiveModel {
            id: Set(Uuid::new_v4()),
            email: Set(email
                .clone()
                .unwrap_or_else(|| format!("{sub}@users.noreply.oidc"))),
            name: Set(display_name.into()),
            google_id: Set(google_id),
            azure_id: Set(azure_id),
            email_verified: Set(true),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
            last_login_at: Set(Utc::now()),
            password_changed_at: Set(None),
            department_id: Set(None),
            is_independent: Set(false),
            effective_permissions: Set(None),
            status: Set(UserStatus::Active),
            mfa_enabled: Set(false),
            mfa_secret: Set(None),
            picture: Set(picture.clone()),
            password: Set(None),
            metadata: Set(None),
            hd: Set(hd),
        };
        new_user
            .clone()
            .insert(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("{:?}", e);
                AuthError::ServiceTemporarilyUnavailable
            })?;
        let inserted_user = new_user.try_into_model().map_err(|e| {
            eprintln!("db error while parsing user {:?}", e);
            AuthError::ServiceTemporarilyUnavailable
        })?;
        let role_name = if is_first_user { "Super Admin" } else { "User" };
        if let Some(role) = roles::Entity::find()
            .filter(roles::Column::Name.eq(role_name))
            .one(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("role lookup error: {e}");
                AuthError::ServiceTemporarilyUnavailable
            })?
        {
            let now = Utc::now();
            let assignment = user_role_assignments::ActiveModel {
                id: Set(Uuid::new_v4()),
                user_id: Set(inserted_user.id),
                role_id: Set(role.id),
                scope_department_id: Set(None),
                assigned_by: Set(inserted_user.id),
                created_at: Set(now),
                updated_at: Set(now),
            };
            let _ = assignment.insert(&app_state.database).await;
        }
        let authz = AuthorizationService::new(&app_state.database);
        let _ = authz
            .recompute_effective_permissions(inserted_user.id)
            .await;
        user = Some(inserted_user);
    };
    let user = user.ok_or(AuthError::EmailDoesNotExist)?;

    let access_token_claims =
        Claims::new_access_token(user.email.clone(), user.name.clone(), user.id);
    let refresh_token_claims = RefreshClaims::new_refresh_token(user.email.clone(), user.id);
    let authz = AuthorizationService::new(&app_state.database);
    let mut roles_map = authz.user_roles_map(&[user.id]).await?;
    let roles = roles_map.remove(&user.id).unwrap_or_default();
    let is_super_admin = roles.iter().any(|r| r == "Super Admin");
    let user_response = User {
        id: user.id,
        sub: sub.clone(),
        email: user.email,
        name: user.name,
        picture: picture,
        hd: user.hd,
        roles,
        status: user.status,
        department_id: user.department_id,
        is_super_admin,
        has_password: user.password.is_some(), // SSO-only users don't have password
        mfa_enabled: user.mfa_enabled,
        last_login_at: Some(user.last_login_at),
        password_changed_at: None,
        created_at: user.created_at,
        updated_at: user.updated_at,
        effective_permissions: user.effective_permissions,
    };

    let resp = AuthToken {
        access_token: access_token_claims.get_token_string(),
        token_type: TokenType::Bearer,
        expires_in: 3600, // 1 hour - match your JWT expiry
        refresh_token: Some(refresh_token_claims.get_token_string()), // TODO: Implement refresh token logic if needed
        user: Some(user_response),
    };
    Ok((StatusCode::OK, Json(resp)))
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
