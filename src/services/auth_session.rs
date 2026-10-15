// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::{
        claims::{ACCESS_TOKEN_TTL_SECS, Claiming, Claims, RefreshClaims},
        error::AuthError,
    },
    dto::auth::{AuthToken, TokenType, User},
    models::users::{self, UserStatus},
    services::authorization::AuthorizationService,
    state::SharedState,
};
use axum::http::Extensions;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect};
use uuid::Uuid;

#[derive(Clone)]
pub struct SessionGuard {
    db: DatabaseConnection,
}

impl SessionGuard {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    async fn ensure_active_user(&self, user_id: Uuid) -> Result<(), AuthError> {
        let status = users::Entity::find_by_id(user_id)
            .select_only()
            .column(users::Column::Status)
            .into_tuple::<UserStatus>()
            .one(&self.db)
            .await
            .map_err(|e| {
                eprintln!("session user status lookup error: {e}");
                AuthError::DbTimeout
            })?;
        ensure_session_status(status.as_ref())
    }
}

#[derive(Clone, Copy)]
struct VerifiedSession(Uuid);

pub fn ensure_session_status(status: Option<&UserStatus>) -> Result<(), AuthError> {
    match status {
        Some(UserStatus::Active) => Ok(()),
        Some(UserStatus::Deactivated | UserStatus::Suspended) => Err(AuthError::AccountDeactivated),
        Some(UserStatus::Pending) => Err(AuthError::AccountPendingApproval),
        Some(UserStatus::Deleted) | None => Err(AuthError::InvalidToken),
    }
}

pub async fn ensure_request_session(
    extensions: &mut Extensions,
    user_id: Uuid,
) -> Result<(), AuthError> {
    if extensions
        .get::<VerifiedSession>()
        .is_some_and(|session| session.0 == user_id)
    {
        return Ok(());
    }
    let guard = extensions.get::<SessionGuard>().cloned().ok_or_else(|| {
        eprintln!("SessionGuard extension is missing; rejecting authenticated request");
        AuthError::ServiceTemporarilyUnavailable
    })?;
    guard.ensure_active_user(user_id).await?;
    extensions.insert(VerifiedSession(user_id));
    Ok(())
}

pub async fn refresh_access_token(
    app_state: &SharedState,
    refresh_token: &str,
) -> Result<AuthToken, AuthError> {
    let refresh_claims = RefreshClaims::from_token_string(refresh_token).map_err(|e| {
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
    ensure_session_status(Some(&user.status))?;
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
        department_id: user.department_id,
        is_super_admin,
        has_password: user.password.is_some(),
        mfa_enabled: user.mfa_enabled,
        last_login_at: Some(user.last_login_at),
        password_changed_at: None,
        created_at: user.created_at,
        updated_at: user.updated_at,
        effective_permissions: user.effective_permissions,
    };
    Ok(AuthToken {
        access_token: access_token_claims.get_token_string(),
        token_type: TokenType::Bearer,
        expires_in: ACCESS_TOKEN_TTL_SECS as i32,
        refresh_token: None,
        user: Some(user_response),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_user_keeps_their_session() {
        assert!(ensure_session_status(Some(&UserStatus::Active)).is_ok());
    }

    #[test]
    fn deactivated_user_loses_their_session() {
        assert!(matches!(
            ensure_session_status(Some(&UserStatus::Deactivated)),
            Err(AuthError::AccountDeactivated)
        ));
    }

    #[test]
    fn suspended_user_loses_their_session() {
        assert!(matches!(
            ensure_session_status(Some(&UserStatus::Suspended)),
            Err(AuthError::AccountDeactivated)
        ));
    }

    #[test]
    fn pending_user_has_no_session() {
        assert!(matches!(
            ensure_session_status(Some(&UserStatus::Pending)),
            Err(AuthError::AccountPendingApproval)
        ));
    }

    #[test]
    fn deleted_user_loses_their_session() {
        assert!(matches!(
            ensure_session_status(Some(&UserStatus::Deleted)),
            Err(AuthError::InvalidToken)
        ));
    }

    #[test]
    fn user_missing_from_the_database_has_no_session() {
        assert!(matches!(
            ensure_session_status(None),
            Err(AuthError::InvalidToken)
        ));
    }

    #[tokio::test]
    async fn request_without_session_guard_is_rejected() {
        let mut extensions = Extensions::new();

        assert!(matches!(
            ensure_request_session(&mut extensions, Uuid::new_v4()).await,
            Err(AuthError::ServiceTemporarilyUnavailable)
        ));
    }

    #[tokio::test]
    async fn session_verified_earlier_in_the_request_is_not_rechecked() {
        let user_id = Uuid::new_v4();
        let mut extensions = Extensions::new();
        extensions.insert(VerifiedSession(user_id));

        assert!(
            ensure_request_session(&mut extensions, user_id)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn session_verified_for_another_user_does_not_carry_over() {
        let mut extensions = Extensions::new();
        extensions.insert(VerifiedSession(Uuid::new_v4()));

        assert!(matches!(
            ensure_request_session(&mut extensions, Uuid::new_v4()).await,
            Err(AuthError::ServiceTemporarilyUnavailable)
        ));
    }
}
