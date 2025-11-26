use axum::{Json, extract::{Path, Query, State}, response::Redirect};
use chrono::Utc;
use oauth2::TokenResponse;
use openidconnect::{core::{CoreAuthenticationFlow, CoreUserInfoClaims}};
use openidconnect::{AuthorizationCode, CsrfToken, Nonce, PkceCodeChallenge, PkceCodeVerifier, Scope};
use openidconnect::{TokenResponse as OidcTokenResponse};
use axum::http::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, TryIntoModel};
use uuid::Uuid;
use crate::{auth::{claims::Claims, jwt::KEYS}, dto::{oauth::OidcProvider}, models::{oauth_sessions, users::{self, UserStatus}}};
use crate::{auth::error::AuthError, dto::{auth::LoginResponse, oauth::{OAuthCallback, StartParams}}, state::SharedState};

#[utoipa::path(
    get,
    path = "/auth/{oidc_provider}/login",
    params(
        ("oidc_provider" = OidcProvider, Path),
        ("redirect_to" = Option<String>, Query, description = "Optional post-login redirect target")),
    responses(
        (status = 302, description = "Redirects the user to Google for login"),
        (status = 503, description = "Service temporarily unavailable")
    )
)]
pub async fn oidc_login_start(
    Path(oidc_provider):Path<OidcProvider>,
    Query(StartParams { redirect_to }): Query<StartParams>,
    State(app_state):State<SharedState>
) -> Result<Redirect, AuthError> {
    // Generate PKCE + CSRF + nonce
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (oidc_client,_) = app_state.get_oidc_client_and_column(&oidc_provider);
    let (auth_url, csrf_state, nonce) =  oidc_client
        .read()
        .await
        .authorize_url(
        CoreAuthenticationFlow::AuthorizationCode,
        CsrfToken::new_random,
        Nonce::new_random,
        )
        .add_scope(Scope::new("openid".to_string()))
        .add_scope(Scope::new("email".to_string()))
        .add_scope(Scope::new("profile".to_string()))
        .set_pkce_challenge(pkce_challenge)
        .url();

    let state_str = csrf_state.secret().to_string();
    let sess = oauth_sessions::ActiveModel {
        state:Set(state_str.into()),
        pkce_verifier: Set(pkce_verifier.secret().to_string()),
        nonce: Set(nonce.secret().to_string()),
        redirect_to:Set(redirect_to),
        created_on:Set(Utc::now()),
    };
    sess.insert(&app_state.database)
      .await
      .map_err(|e|{
        eprintln!("{:?}",e);
        AuthError::ServiceTemporarilyUnavailable})?;
    Ok(Redirect::to(auth_url.as_str()))
}

#[utoipa::path(
    get,
    path = "/auth/{oidc_provider}/callback",
    params(
        ("oidc_provider" = OidcProvider, Path),
        ("code" = String, Query, description = "Authorization code from Google"),
        ("state" = String, Query, description = "CSRF state")
    ),
    responses(
        (status = 200, body = LoginResponse, description = "Logged in via Google"),
        (status = 400, description = "Invalid or expired OAuth state"),
        (status = 503, description = "Service temporarily unavailable")
    )
)]
pub async fn oidc_oauth_callback(
    Path(oidc_provider):Path<OidcProvider>,
    Query(cb): Query<OAuthCallback>,
    State(app_state):State<SharedState>
) -> Result<(StatusCode, Json<LoginResponse>), AuthError> {
    let (oidc_client,column) = app_state.get_oidc_client_and_column(&oidc_provider);
    let http_client = reqwest::ClientBuilder::new()
       .redirect(reqwest::redirect::Policy::none())
       .build()
       .expect("failed to build openidconnect reqwest client");
    let sess = oauth_sessions::Entity::find()
        .filter(oauth_sessions::Column::State.eq(Some(cb.state.to_owned())))
        .order_by_desc(oauth_sessions::Column::CreatedOn)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error while fetching session: {e:?}");
            AuthError::ServiceTemporarilyUnavailable})?
        .ok_or(AuthError::InvalidToken)?;
    let active: oauth_sessions::ActiveModel = sess.clone().into();
    active.delete(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error while deleting oauth_session: {e:?}");
            AuthError::ServiceTemporarilyUnavailable })?;
    let token_resp = oidc_client
        .read()
        .await
        .exchange_code(AuthorizationCode::new(cb.code))
        .expect("Failed to get token response")
        .set_pkce_verifier(PkceCodeVerifier::new(sess.pkce_verifier.clone()))
        .request_async(&http_client)
        .await
        .map_err(|e| {
            eprintln!("token exchange err: {e:?}");
            AuthError::ServiceTemporarilyUnavailable
        })?;
    let nonce = Nonce::new(sess.nonce.clone());
    let id_token = token_resp
        .id_token()
        .ok_or(AuthError::ServiceTemporarilyUnavailable)?;
    let claims = match id_token.claims(&oidc_client.read().await.id_token_verifier(),&nonce) {
      Ok(c) => c,
      Err(_e) => {
        app_state.refresh_oidc_clinet(&oidc_provider)
          .await
          .map_err(|e| {
            eprintln!("oidc client refresh error: {e:?}");
            AuthError::ServiceTemporarilyUnavailable
        })?;
        id_token
            .claims(&oidc_client.read().await.id_token_verifier(),&nonce)
            .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?
     }
    };
    let sub = claims.subject().as_str().to_string();
    let mut email = claims.email().map(|e| e.as_str().to_string());
    let mut display_name: Option<String> = None;
    let avatar = claims
       .picture()
       .and_then(|pic_claim| pic_claim.get(None))       // default locale
       .map(|url| url.as_str().to_owned());
    if email.is_none() {
        let http_client = reqwest::ClientBuilder::new()
           .redirect(reqwest::redirect::Policy::none())
           .build().unwrap();
        let info: CoreUserInfoClaims = oidc_client
            .read()
            .await
            .user_info(token_resp.access_token().to_owned(), None)
            .expect("userinfo req")
            .request_async(&http_client)
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
        .order_by_desc(users::Column::CreatedOn)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error while fetching user: {e:?}");
            AuthError::ServiceTemporarilyUnavailable})?;
    if let Some(u) = &user {
      let mut active_user:users::ActiveModel = u.clone().into();
      active_user.last_login_on = Set(Utc::now());
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
                active_user.google_id = Set(Some(sub.clone()));
                active_user.updated_on = Set(Utc::now());
                active_user.last_login_on = Set(Utc::now());
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
            email: Set(email.clone().unwrap_or_else(|| format!("{sub}@users.noreply.oidc"))),
            name: Set(display_name.into()),
            google_id: Set(Some(sub.clone())),
            azure_id:Set(None),
            email_verified:Set(true),
            created_on:Set(Utc::now()),
            updated_on:Set(Utc::now()),
            last_login_on:Set(Utc::now()),
            status:Set(UserStatus::Active),
            two_factor_auth:Set(false),
            two_factor_secret:Set(None),
            avatar:Set(avatar),
            metadata:Set(None),
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
    let user = user
     .ok_or(AuthError::EmailDoesNotExist)?;
    let claims = Claims::new(user.email, user.name, user.id);
    let token = jsonwebtoken::encode(&jsonwebtoken::Header::default(), &claims, &KEYS.get().unwrap().encoding).unwrap();
    let resp = LoginResponse {
        token,
    };
  Ok((StatusCode::OK, Json(resp)))
}