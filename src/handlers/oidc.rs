use std::borrow::Cow;
use axum::{Json, extract::{Path, Query, State}, response::Redirect};
use chrono::Utc;
use openidconnect::{AuthorizationCode, CsrfToken, Nonce, OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, core::{CoreAuthenticationFlow, CoreUserInfoClaims}};
use openidconnect::{TokenResponse as OidcTokenResponse};
use axum::http::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, TryIntoModel};
use uuid::Uuid;
use crate::{auth::{claims::Claims, jwt::KEYS}, dto::oauth::AuthProvider, error::ErrorResponse, models::{oauth_sessions, users::{self, UserRole, UserStatus}}};
use crate::{auth::error::{AuthError}, dto::{auth::{AuthTokenResponse, TokenType, User}, oauth::{OAuthCallback, StartParams}}, state::SharedState};

#[utoipa::path(
    get,
    path = "/auth/{provider}",
    tag = "auth",
    operation_id = "initiateAuth",
    params(
        ("provider" = String, Path, description = "Auth provider identifier (e.g., google, azure, keycloak)"),
        ("redirect_uri" = Option<String>, Query, description = "Optional post-login redirect target", format = "uri")),
    responses(
        (status = 303, description = "Redirects the user to provider's login page"),
        (status = 400, body = ErrorResponse, description = "Invalid provider or configuration"),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
    )
)]
pub async fn oidc_login_start(
    Path(provider): Path<AuthProvider>,
    Query(query): Query<StartParams>,
    State(app_state): State<SharedState>
) -> Result<Redirect, AuthError> {
    // Generate PKCE + CSRF + nonce
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (oidc_client, _,default_redirect_uri) = app_state
        .get_oidc_client_and_column_and_redirect_uri(&provider)
        .map_err(|_| AuthError::InvalidProvider)?;
    let redirect_uri = RedirectUrl::new(query.redirect_uri.clone().unwrap_or(default_redirect_uri.to_string()))
        .map_err(|_| AuthError::InvalidRedirectUri)?;
    let (auth_url, csrf_state, nonce) = oidc_client
        .read()
        .await
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .set_redirect_uri(Cow::Owned(redirect_uri))
        .add_scope(Scope::new("email".to_string()))
        .add_scope(Scope::new("profile".to_string()))
        .set_pkce_challenge(pkce_challenge)
        .url();

    let state_str = csrf_state.secret().to_string();
    let sess = oauth_sessions::ActiveModel {
        state: Set(state_str.into()),
        pkce_verifier: Set(pkce_verifier.secret().to_string()),
        nonce: Set(nonce.secret().to_string()),
        redirect_uri: Set(Some(query.redirect_uri.unwrap_or(default_redirect_uri.to_string()))),
        created_at: Set(Utc::now()),
    };
    sess.insert(&app_state.database)
        .await
        .map_err(|e| {
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
        (status = 200, body = AuthTokenResponse, description = "Successfully authenticated"),
        (status = 302, description = "Redirect to application with tokens"),
        (status = 400, body = ErrorResponse, description = "Invalid callback parameters"),
        (status = 401, body = ErrorResponse, description = "Unauthorized"),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
    )
)]
pub async fn oidc_oauth_callback_get(
    Path(provider): Path<AuthProvider>,
    Query(cb): Query<OAuthCallback>,
    State(app_state): State<SharedState>
) -> Result<(StatusCode, Json<AuthTokenResponse>), AuthError> {
    oidc_oauth_callback(provider, cb, app_state).await
}

async fn oidc_oauth_callback(
    provider: AuthProvider,
    cb: OAuthCallback,
    app_state: SharedState,
) -> Result<(StatusCode, Json<AuthTokenResponse>), AuthError> {
    // Check for OAuth error responses
    if let Some(error) = cb.error {
        eprintln!("OAuth error: {} - {:?}", error, cb.error_description);
        return Err(AuthError::InvalidCallbackParameters);
    }
    let code = cb
     .code
     .ok_or(AuthError::InvalidCallbackParameters)?;
    let (oidc_client, column,default_redirect_uri) = app_state
        .get_oidc_client_and_column_and_redirect_uri(&provider)
        .map_err(|_| AuthError::InvalidProvider)?;
    let sess = oauth_sessions::Entity::find()
        .filter(oauth_sessions::Column::State.eq(Some(cb.state.to_owned())))
        .order_by_desc(oauth_sessions::Column::CreatedAt)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error while fetching session: {e:?}");
            AuthError::ServiceTemporarilyUnavailable})?
        .ok_or(AuthError::InvalidToken)?;
    let redirect_uri = RedirectUrl::new(sess.redirect_uri.clone().unwrap_or(default_redirect_uri.to_string()))
        .map_err(|_| AuthError::InvalidRedirectUri)?;
    let active: oauth_sessions::ActiveModel = sess.clone().into();
    active.delete(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error while deleting oauth_session: {e:?}");
            AuthError::ServiceTemporarilyUnavailable })?;
    let token_resp = oidc_client
        .read()
        .await
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
    let claims = match id_token.claims(&oidc_client.read().await.id_token_verifier(), &nonce) {
        Ok(c) => c,
        Err(_e) => {
            app_state
                .refresh_oidc_clinet(&provider)
                .await
                .map_err(|e| {
                    eprintln!("oidc client refresh error: {e:?}");
                    AuthError::ServiceTemporarilyUnavailable
                })?;
            id_token
                .claims(&oidc_client.read().await.id_token_verifier(), &nonce)
                .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?
        }
    };
    let sub = claims.subject()
       .as_str()
       .to_string();
    let mut email = claims.email()
       .map(|e| e.as_str().to_string());
    let mut display_name: Option<String> = None;
    let picture = claims.picture()
       .and_then(|pic_claim| pic_claim.get(None))       // default locale
       .map(|url| url.as_str().to_owned());
    let hd = claims.website()
       .and_then(|website_claim| website_claim.get(None))       // default locale
       .map(|url| url.as_str().to_owned());
    let google_id = if provider == "google" {Some(sub.clone())}else{None};
    let azure_id = if provider == "azure" {Some(sub.clone())}else{None};
    if email.is_none() {
        let info: CoreUserInfoClaims = oidc_client
            .read()
            .await
            .user_info(token_resp.access_token().to_owned(), None)
            .expect("userinfo req")
            .request_async(&app_state.req_client)
            .await
            .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
        email = info.email().map(|e| e.as_str().to_string());
        if display_name.is_none(){
          display_name = info.name().and_then(|n| n.get(None).map(|s| s.to_string()));
        }
    } else {
        display_name = claims
            .name()
            .and_then(|n| n.get(None).map(|s| s.to_string()));
    }
    let mut user = users::Entity::find()
        .filter(column.eq(Some(sub.clone())))
        .order_by_desc(users::Column::CreatedAt)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error while fetching user: {e:?}");
            AuthError::ServiceTemporarilyUnavailable})?;
    if let Some(u) = &user {
      match &u.status {
         UserStatus::InActive |
         UserStatus::Suspended |
         UserStatus::Deleted => return Err(AuthError::EmailDoesNotExist),
         _ => ()
      }
      let mut active_user:users::ActiveModel = u.clone().into();
      active_user.last_login_at = Set(Utc::now());
      active_user.update(&app_state.database)
         .await
         .map_err(|e|{
            eprintln!("db error while updating user {:?}",e);
            AuthError::ServiceTemporarilyUnavailable})?;
    }
    if user.is_none() {
        if let Some(ref em) = email {
            user = users::Entity::find()
                .filter(users::Column::Email.eq(em))
                .one(&app_state.database)
                .await
                .map_err(|e|{
                      eprintln!("{:?}",e);
                      AuthError::ServiceTemporarilyUnavailable})?;
            if let Some(u) = &user {
                let mut active_user:users::ActiveModel = u.clone().into();
                active_user.google_id = Set(google_id.clone());
                active_user.azure_id = Set(azure_id.clone());
                active_user.updated_at = Set(Utc::now());
                active_user.last_login_at = Set(Utc::now());
                active_user.update(&app_state.database)
                  .await
                  .map_err(|e|{
                      eprintln!("db error while updating user {:?}",e);
                      AuthError::ServiceTemporarilyUnavailable})?;
            }
        }
    }
    if user.is_none() {
        let new_user = users::ActiveModel{
            id: Set(Uuid::new_v4()),
            org_id:Set(None),
            email: Set(email.clone().unwrap_or_else(|| format!("{sub}@users.noreply.oidc"))),
            name: Set(display_name.into()),
            google_id: Set(google_id),
            azure_id:Set(azure_id),
            email_verified:Set(true),
            created_at:Set(Utc::now()),
            updated_at:Set(Utc::now()),
            last_login_at:Set(Utc::now()),
            password_changed_at:Set(None),
            department:Set(None),
            status:Set(UserStatus::Active),
            mfa_enabled:Set(false),
            mfa_secret:Set(None),
            picture:Set(picture.clone()),
            password:Set(None),
            role:Set(UserRole::SuperAdmin),
            metadata:Set(None),
            hd:Set(hd),
        };
        new_user.clone()
           .insert(&app_state.database)
           .await
           .map_err(|e|{
             eprintln!("{:?}",e);
             AuthError::ServiceTemporarilyUnavailable})?;
        user = Some(new_user.try_into_model()
           .map_err(|e|{
             eprintln!("db error while parsing user {:?}",e);
             AuthError::ServiceTemporarilyUnavailable})?);
    };
    let user = user.ok_or(AuthError::EmailDoesNotExist)?;

    let jwt_claims = Claims::new_access_token(user.email.clone(), user.name.clone(), user.id,user.org_id,user.role);
    let access_token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &jwt_claims,
        &KEYS.get().unwrap().encoding
    ).unwrap();

    let user_response = User {
        id: user.id,
        sub: sub.clone(),
        email: user.email,
        name: user.name,
        picture: picture,
        hd: user.hd,
        role: user.role, // TODO: Map from database if role field exists
        status: user.status,
        department: user.department,
        is_super_admin: user.role == UserRole::SuperAdmin, // Default to false, update based on database field if available
        has_password: user.password.is_some(), // SSO-only users don't have password
        mfa_enabled: user.mfa_enabled,
        last_login_at: Some(user.last_login_at),
        password_changed_at: None,
        created_at: user.created_at,
        updated_at: user.updated_at,
    };

    let resp = AuthTokenResponse {
        access_token,
        token_type: TokenType::Bearer,
        expires_in: 3600, // 1 hour - match your JWT expiry
        refresh_token: None, // TODO: Implement refresh token logic if needed
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
    request_body(content = OAuthCallback, description = "OAuth callback parameters"),
    responses(
        (status = 200, body = AuthTokenResponse, description = "Successfully authenticated"),
        (status = 400, body = ErrorResponse, description = "Invalid callback parameters"),
        (status = 401, body = ErrorResponse, description = "Unauthorized"),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
    )
)]
pub async fn oidc_oauth_callback_post(
    Path(provider): Path<AuthProvider>,
    State(app_state): State<SharedState>,
    Json(cb): Json<OAuthCallback>,
) -> Result<(StatusCode, Json<AuthTokenResponse>), AuthError> {
    oidc_oauth_callback(provider, cb, app_state).await
}