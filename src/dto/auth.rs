use serde::{Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use chrono::{DateTime, Utc};
use crate::models::users::{UserRole, UserStatus};

#[derive(Serialize, ToSchema)]
pub struct AuthInitResponse {
    /// URL to redirect user for authentication
    #[schema(format = "uri")]
    pub auth_url: String,
    /// CSRF protection state token
    pub state: String,
}

#[derive(Serialize, IntoParams, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponse {
    pub token: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuthTokenResponse {
    /// JWT access token
    pub access_token: String,
    /// Always "Bearer"
    pub token_type: TokenType,
    /// Token expiration time in seconds
    pub expires_in: i32,
    /// Refresh token for obtaining new access tokens
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// User profile
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<User>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "PascalCase")]
pub enum TokenType {
    Bearer,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct User {
    /// User ID (required, UUID)
    pub id: Uuid,
    /// OIDC subject identifier (required)
    pub sub: String,
    /// User email (required)
    #[schema(format = "email")]
    pub email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Profile picture URL
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(format = "uri")]
    pub picture: Option<String>,
    /// Hosted domain (organization domain)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hd: Option<String>,
    pub role:UserRole,
    pub status:UserStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub department: Option<String>,
    /// Super admin has full platform control and cannot be deleted (default: false)
    #[serde(default)]
    pub is_super_admin: bool,
    /// Whether user has password authentication (vs SSO-only)
    pub has_password: bool,
    /// Whether MFA is enabled for this user
    pub mfa_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_login_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_changed_at: Option<DateTime<Utc>>,
    /// User creation timestamp (required)
    pub created_at: DateTime<Utc>,
    /// User last update timestamp (required)
    pub updated_at: DateTime<Utc>,
}