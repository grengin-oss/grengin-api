use crate::{
    auth::{
        claims::{Claiming, Claims, RefreshClaims},
        error::{AuthError, Error},
    },
    dto::auth::{AuthToken, RefreshToken, TokenType, User},
    models::users::{self, UserStatus},
    services::authorization::AuthorizationService,
    state::SharedState,
};
use axum::{extract::State, Json};
use reqwest::StatusCode;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

#[utoipa::path(
    post,
    path = "/auth/refresh",
    tag = "admin",
    request_body = RefreshToken,
    responses(
       (status = 400, content_type = "application/json", body = Error, description = "Missing credentials (code=6102)"),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired refresh token (code=6103)"),
       (status = 404, content_type = "application/json", body = Error, description = "Email does not exist (code=6101)"),
       (status = 404, content_type = "application/json", body = Error, description = "User not found (code=5003)"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000)"),
    )
)]
pub async fn handle_refresh_token(
    State(app_state): State<SharedState>,
    Json(req): Json<RefreshToken>,
) -> Result<(StatusCode, Json<AuthToken>), AuthError> {
    let refresh_claims = RefreshClaims::from_token_string(&req.refresh_token).map_err(|e| {
        eprintln!("Refresh token decoding error: {e}");
        AuthError::InvalidToken
    })?;
    let user = users::Entity::find_by_id(refresh_claims.user_id)
        .filter(users::Column::Status.ne(UserStatus::Deleted))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("Db get one error: {:?}", e);
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::EmailDoesNotExist)?;
    match user.status {
        UserStatus::Deactivated | UserStatus::Suspended => {
            return Err(AuthError::AccountDeactivated);
        }
        _ => (),
    }
    let access_token_claims =
        Claims::new_access_token(user.email.clone(), user.name.clone(), user.id);
    let authz = AuthorizationService::new(&app_state.database);
    let mut roles_map = authz.user_roles_map(&[user.id]).await?;
    let roles = roles_map.remove(&user.id).unwrap_or_default();
    let is_super_admin = roles.iter().any(|r| r == "Super Admin");
    let user_response = User {
        id: user.id,
        sub: user
            .azure_id
            .unwrap_or(user.google_id.unwrap_or(user.email.clone())),
        email: user.email,
        name: user.name,
        picture: user.picture,
        hd: user.hd,
        roles,
        status: user.status,
        department_id: user.department_id, // TODO
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
        refresh_token: None,
        user: Some(user_response),
    };
    Ok((StatusCode::OK, Json(resp)))
}
