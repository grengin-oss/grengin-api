use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::ser::{Serialize, Serializer};
use serde::Serialize as SerdeSerialize;
use std::collections::BTreeMap;
use utoipa::ToSchema;
use uuid::Uuid;

pub const APP_NAME: &str = "grengin";

/// Numeric, stable internal codes (explicit values only).
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, ToSchema)]
pub enum AuthErrorCode {
    // 6000-6999: auth generic / infra
    ServiceTemporarilyUnavailable = 6000,

    // 5000-5999: DB (same as AppError)
    DbUnavailable = 5000,
    DbTimeout = 5001,
    DbConflict = 5002,
    DbNotFound = 5003,

    // 6100-6199: credentials/session
    InvalidCredentials = 6100,
    EmailDoesNotExist = 6101,
    MissingCredentials = 6102,
    InvalidToken = 6103,
    InvalidUserStatus = 6104,
    AccountDeactivated = 6105,
    EmailAlreadyExist = 6106,

    // 6200-6299: provider / oauth / redirect
    InvalidProvider = 6200,
    InvalidCallbackParameters = 6201,
    InvalidRedirectUri = 6202,

    // 6300-6399: org / permissions / access control
    PermissionDenied = 6300,
    OrgDoesNotExist = 6301,
    ResourceNotFound = 6302,
    EmailDomainNotAllowed = 6303,

    // 6400-6499: SSO config / admin controls
    SsoProviderNotConfigured = 6400,
    SsoProviderDisabledByAdmin = 6401,
}

impl Serialize for AuthErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u32(*self as u32)
    }
}

#[derive(Debug, SerdeSerialize, ToSchema)]
pub struct AuthErrorResponse {
    pub detail: AuthErrorDetailVariant,
}

#[derive(Debug, SerdeSerialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthErrorDetailVariant {
    Rich(ErrorDetail),
}

#[derive(Debug, SerdeSerialize, ToSchema)]
pub struct ErrorDetail {
    /// Serialized as integer (e.g. 6400)
    #[schema(value_type = u32, example = 6400)]
    pub code: AuthErrorCode,

    /// English fallback
    pub description: String,
    pub solution: String,

    /// Translation keys
    pub description_key: String,
    pub solution_key: String,

    /// Placeholder values for translation rendering (e.g. {provider}, {app}, {org_id})
    #[schema(value_type = Object)]
    pub params: BTreeMap<String, String>,

    /// Vendor / external code if relevant
    pub external_code: Option<String>,
}

#[derive(Debug, ToSchema)]
pub enum AuthError {
    ServiceTemporarilyUnavailable,

    DbUnavailable,
    DbTimeout,
    DbConflict,
    DbNotFound,

    InvalidCredentials,
    EmailDoesNotExist,
    MissingCredentials,
    InvalidToken,

    InvalidProvider { provider: Option<String> },
    InvalidCallbackParameters,
    InvalidRedirectUri { redirect_uri: Option<String> },

    PermissionDenied,
    EmailAlreadyExist,

    OrgDoesNotExist { org_id: Option<Uuid> },
    ResourceNotFound,

    InvalidUserStatus,
    AccountDeactivated,

    SsoProviderNotConfigured { provider: Option<String> },
    SsoProviderDisabledByAdmin { provider: Option<String> },

    EmailDomainNotAllowed { domain: Option<String> },
}

impl AuthError {
    fn base_params() -> BTreeMap<String, String> {
        let mut p = BTreeMap::new();
        p.insert("app".to_string(), APP_NAME.to_string());
        p
    }

    fn render(template: &str, params: &BTreeMap<String, String>) -> String {
        let mut out = template.to_string();
        for (k, v) in params {
            out = out.replace(&format!("{{{k}}}"), v);
        }
        out
    }

    fn to_detail(&self) -> (StatusCode, ErrorDetail) {
        match self {
            // ---------- infra ----------
            AuthError::ServiceTemporarilyUnavailable => {
                let params = Self::base_params();
                let description_key = "error.auth.service_unavailable.description".to_string();
                let solution_key = "error.auth.service_unavailable.solution".to_string();

                let description_tpl = "The authentication service for {app} is temporarily unavailable.";
                let solution_tpl = "Try again in a few minutes. If it persists, contact support.";

                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    ErrorDetail {
                        code: AuthErrorCode::ServiceTemporarilyUnavailable,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            // ---------- DB (same as AppError) ----------
            AuthError::DbUnavailable => {
                let params = Self::base_params();
                let description_key = "error.db.unavailable.description".to_string();
                let solution_key = "error.db.unavailable.solution".to_string();

                let description_tpl = "{app} couldn't reach the database right now.";
                let solution_tpl =
                    "Try again in a few minutes. If it persists, check database health, network, and connection pool limits.";

                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    ErrorDetail {
                        code: AuthErrorCode::DbUnavailable,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AuthError::DbTimeout => {
                let params = Self::base_params();
                let description_key = "error.db.timeout.description".to_string();
                let solution_key = "error.db.timeout.solution".to_string();

                let description_tpl = "A database operation timed out in {app}.";
                let solution_tpl =
                    "Retry the request. If it continues, investigate database load, slow queries, and connection pool timeouts.";

                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    ErrorDetail {
                        code: AuthErrorCode::DbTimeout,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AuthError::DbConflict => {
                let params = Self::base_params();
                let description_key = "error.db.conflict.description".to_string();
                let solution_key = "error.db.conflict.solution".to_string();

                let description_tpl = "The request conflicts with existing data in {app}.";
                let solution_tpl =
                    "Refresh data and retry. If you're creating a resource, ensure unique fields are not duplicated.";

                (
                    StatusCode::CONFLICT,
                    ErrorDetail {
                        code: AuthErrorCode::DbConflict,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AuthError::DbNotFound => {
                let params = Self::base_params();
                let description_key = "error.db.not_found.description".to_string();
                let solution_key = "error.db.not_found.solution".to_string();

                let description_tpl = "The requested data was not found in {app}.";
                let solution_tpl = "Verify the id and try again.";

                (
                    StatusCode::NOT_FOUND,
                    ErrorDetail {
                        code: AuthErrorCode::DbNotFound,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            // ---------- credentials/session ----------
            AuthError::InvalidCredentials => {
                let params = Self::base_params();
                let description_key = "error.auth.invalid_credentials.description".to_string();
                let solution_key = "error.auth.invalid_credentials.solution".to_string();

                let description_tpl = "Invalid email or password.";
                let solution_tpl =
                    "Verify your credentials and try again. If you forgot your password, reset it.";

                (
                    StatusCode::UNAUTHORIZED,
                    ErrorDetail {
                        code: AuthErrorCode::InvalidCredentials,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AuthError::EmailDoesNotExist => {
                let params = Self::base_params();
                let description_key = "error.auth.email_not_found.description".to_string();
                let solution_key = "error.auth.email_not_found.solution".to_string();

                let description_tpl = "No account found for the provided email address.";
                let solution_tpl = "Double-check the email address or sign up for a new account.";

                (
                    StatusCode::NOT_FOUND,
                    ErrorDetail {
                        code: AuthErrorCode::EmailDoesNotExist,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AuthError::MissingCredentials => {
                let params = Self::base_params();
                let description_key = "error.auth.missing_credentials.description".to_string();
                let solution_key = "error.auth.missing_credentials.solution".to_string();

                let description_tpl = "Missing required credentials.";
                let solution_tpl = "Provide the required authentication fields and try again.";

                (
                    StatusCode::BAD_REQUEST,
                    ErrorDetail {
                        code: AuthErrorCode::MissingCredentials,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AuthError::InvalidToken => {
                let params = Self::base_params();
                let description_key = "error.auth.invalid_token.description".to_string();
                let solution_key = "error.auth.invalid_token.solution".to_string();

                let description_tpl = "The access token is invalid or expired.";
                let solution_tpl = "Sign in again to get a new token.";

                (
                    StatusCode::UNAUTHORIZED,
                    ErrorDetail {
                        code: AuthErrorCode::InvalidToken,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AuthError::InvalidUserStatus => {
                let params = Self::base_params();
                let description_key = "error.auth.invalid_user_status.description".to_string();
                let solution_key = "error.auth.invalid_user_status.solution".to_string();

                let description_tpl = "The specified user status is invalid.";
                let solution_tpl = "Contact support or an admin to correct the user status.";

                (
                    StatusCode::BAD_REQUEST,
                    ErrorDetail {
                        code: AuthErrorCode::InvalidUserStatus,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AuthError::AccountDeactivated => {
                let params = Self::base_params();
                let description_key = "error.auth.account_deactivated.description".to_string();
                let solution_key = "error.auth.account_deactivated.solution".to_string();

                let description_tpl = "This account is deactivated by an administrator.";
                let solution_tpl = "Contact your admin to reactivate your account.";

                (
                    StatusCode::UNAUTHORIZED,
                    ErrorDetail {
                        code: AuthErrorCode::AccountDeactivated,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AuthError::EmailAlreadyExist => {
                let params = Self::base_params();
                let description_key = "error.auth.email_already_exists.description".to_string();
                let solution_key = "error.auth.email_already_exists.solution".to_string();

                let description_tpl = "An account with this email address already exists.";
                let solution_tpl = "Sign in instead, or use a different email address.";

                (
                    StatusCode::CONFLICT,
                    ErrorDetail {
                        code: AuthErrorCode::EmailAlreadyExist,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            // ---------- provider / oauth / redirect ----------
            AuthError::InvalidProvider { provider } => {
                let mut params = Self::base_params();
                params.insert(
                    "provider".to_string(),
                    provider.clone().unwrap_or_else(|| "unknown".to_string()),
                );

                let description_key = "error.auth.invalid_provider.description".to_string();
                let solution_key = "error.auth.invalid_provider.solution".to_string();

                let description_tpl =
                    "The specified authentication provider `{provider}` is invalid or unsupported.";
                let solution_tpl =
                    "Use a supported provider (e.g., azure or google) and try again.";

                (
                    StatusCode::BAD_REQUEST,
                    ErrorDetail {
                        code: AuthErrorCode::InvalidProvider,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AuthError::InvalidCallbackParameters => {
                let params = Self::base_params();
                let description_key = "error.auth.invalid_callback_params.description".to_string();
                let solution_key = "error.auth.invalid_callback_params.solution".to_string();

                let description_tpl = "Invalid callback parameters.";
                let solution_tpl = "Restart the authentication flow and try again.";

                (
                    StatusCode::BAD_REQUEST,
                    ErrorDetail {
                        code: AuthErrorCode::InvalidCallbackParameters,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AuthError::InvalidRedirectUri { redirect_uri } => {
                let mut params = Self::base_params();
                if let Some(uri) = redirect_uri {
                    params.insert("redirect_uri".to_string(), uri.clone());
                }

                let description_key = "error.auth.invalid_redirect_uri.description".to_string();
                let solution_key = "error.auth.invalid_redirect_uri.solution".to_string();

                let description_tpl = if redirect_uri.is_some() {
                    "The redirect URI `{redirect_uri}` is invalid or not allowed."
                } else {
                    "The redirect URI is invalid or not allowed."
                };
                let solution_tpl = "Use an allowed redirect URI configured by your admin.";

                (
                    StatusCode::BAD_REQUEST,
                    ErrorDetail {
                        code: AuthErrorCode::InvalidRedirectUri,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            // ---------- org / permissions / access control ----------
            AuthError::PermissionDenied => {
                let params = Self::base_params();
                let description_key = "error.auth.permission_denied.description".to_string();
                let solution_key = "error.auth.permission_denied.solution".to_string();

                let description_tpl = "Permission denied. Admin role is required.";
                let solution_tpl = "Sign in with an admin account or request access from an admin.";

                (
                    StatusCode::FORBIDDEN,
                    ErrorDetail {
                        code: AuthErrorCode::PermissionDenied,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AuthError::OrgDoesNotExist { org_id } => {
                let mut params = Self::base_params();
                if let Some(id) = org_id {
                    params.insert("org_id".to_string(), id.to_string());
                }

                let description_key = "error.auth.org_not_found.description".to_string();
                let solution_key = "error.auth.org_not_found.solution".to_string();

                let description_tpl = if org_id.is_some() {
                    "The specified organization `{org_id}` was not found."
                } else {
                    "The specified organization was not found."
                };
                let solution_tpl = "Verify the organization and try again.";

                (
                    StatusCode::NOT_FOUND,
                    ErrorDetail {
                        code: AuthErrorCode::OrgDoesNotExist,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AuthError::ResourceNotFound => {
                let params = Self::base_params();
                let description_key = "error.auth.resource_not_found.description".to_string();
                let solution_key = "error.auth.resource_not_found.solution".to_string();

                let description_tpl = "The requested resource was not found.";
                let solution_tpl = "Check the identifier and try again.";

                (
                    StatusCode::NOT_FOUND,
                    ErrorDetail {
                        code: AuthErrorCode::ResourceNotFound,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AuthError::EmailDomainNotAllowed { domain } => {
                let mut params = Self::base_params();
                params.insert(
                    "domain".to_string(),
                    domain.clone().unwrap_or_else(|| "unknown".to_string()),
                );

                let description_key = "error.auth.email_domain_not_allowed.description".to_string();
                let solution_key = "error.auth.email_domain_not_allowed.solution".to_string();

                let description_tpl =
                    "This email domain `{domain}` is not allowed by an administrator.";
                let solution_tpl = "Use an approved email domain or contact your admin.";

                (
                    StatusCode::UNAUTHORIZED,
                    ErrorDetail {
                        code: AuthErrorCode::EmailDomainNotAllowed,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            // ---------- SSO config/admin ----------
            AuthError::SsoProviderNotConfigured { provider } => {
                let mut params = Self::base_params();
                params.insert(
                    "provider".to_string(),
                    provider.clone().unwrap_or_else(|| "unknown".to_string()),
                );

                let description_key = "error.auth.sso.not_configured.description".to_string();
                let solution_key = "error.auth.sso.not_configured.solution".to_string();

                let description_tpl = "SSO provider `{provider}` is not configured correctly for {app}.";
                let solution_tpl =
                    "Ask an admin to configure `{provider}` SSO settings, then try again.";

                (
                    StatusCode::CONFLICT,
                    ErrorDetail {
                        code: AuthErrorCode::SsoProviderNotConfigured,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AuthError::SsoProviderDisabledByAdmin { provider } => {
                let mut params = Self::base_params();
                params.insert(
                    "provider".to_string(),
                    provider.clone().unwrap_or_else(|| "unknown".to_string()),
                );

                let description_key = "error.auth.sso.disabled_by_admin.description".to_string();
                let solution_key = "error.auth.sso.disabled_by_admin.solution".to_string();

                let description_tpl = "SSO provider `{provider}` is disabled by an administrator.";
                let solution_tpl =
                    "Ask an admin to enable `{provider}` or use a different sign-in method.";

                (
                    StatusCode::FORBIDDEN,
                    ErrorDetail {
                        code: AuthErrorCode::SsoProviderDisabledByAdmin,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }
        }
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, detail) = self.to_detail();
        let body = AuthErrorResponse {
            detail: AuthErrorDetailVariant::Rich(detail),
        };
        (status, Json(body)).into_response()
    }
}
