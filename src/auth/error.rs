use axum::{response::{IntoResponse, Response},http::StatusCode, Json};
use serde::{ Serialize};
use utoipa::ToSchema;
use crate::error::{ErrorDetail, ErrorDetailVariant, ErrorResponse};

#[derive(Debug, Serialize, ToSchema)]
pub enum AuthError {
    ServiceTemporarilyUnavailable,
    InvalidCredentials,
    EmailDoesNotExist,
    MissingCredentials,
    InvalidToken,
    InvalidProvider,
    InvalidCallbackParameters,
    InvalidRedirectUri,
    PermissionDenied,
    EmailAlreadyExist,
    OrgDoesNotExist,
    ResourceNotFound,
    InvalidUserStatus,
    AccountDeactivated,
    SsoProviderNotConfigured,
    SsoProviderDisabledByAdmin,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            AuthError::ServiceTemporarilyUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "ServiceTemporarilyUnavailable",
                "The authentication service is temporarily unavailable. Please try again in a few minutes.",
            ),
            AuthError::InvalidCredentials => (
                StatusCode::UNAUTHORIZED,
                "InvalidCredentials",
                "Invalid email or password.",
            ),
            AuthError::EmailDoesNotExist => (
                StatusCode::NOT_FOUND,
                "EmailDoesNotExist",
                "No account found for the provided email address.",
            ),
            AuthError::MissingCredentials => (
                StatusCode::BAD_REQUEST,
                "MissingCredentials",
                "Missing required credentials.",
            ),
            AuthError::InvalidToken => (
                StatusCode::UNAUTHORIZED,
                "InvalidToken",
                "The access token is invalid or expired. Please sign in again.",
            ),
            AuthError::InvalidProvider => (
                StatusCode::BAD_REQUEST,
                "InvalidProvider",
                "The specified authentication provider is invalid or unsupported.",
            ),
            AuthError::InvalidCallbackParameters => (
                StatusCode::BAD_REQUEST,
                "InvalidCallbackParameters",
                "Invalid callback parameters. Please restart the authentication flow.",
            ),
            AuthError::InvalidRedirectUri => (
                StatusCode::BAD_REQUEST,
                "InvalidRedirectUri",
                "The redirect URI is invalid or not allowed.",
            ),
            AuthError::PermissionDenied => (
                StatusCode::BAD_REQUEST,
                "PermissionDenied",
                "Permission denied. Admin role is required.",
            ),
            AuthError::OrgDoesNotExist => (
                StatusCode::NOT_FOUND,
                "OrgDoesNotExist",
                "The specified organization was not found.",
            ),
            AuthError::EmailAlreadyExist => (
                StatusCode::CONFLICT,
                "EmailAlreadyExist",
                "An account with this email address already exists.",
            ),
            AuthError::ResourceNotFound => (
                StatusCode::NOT_FOUND,
                "ResourceNotFound",
                "The requested resource was not found.",
            ),
            AuthError::InvalidUserStatus => (
                StatusCode::BAD_REQUEST,
                "InvalidUserStatus",
                "The specified user status is invalid.",
            ),
            AuthError::AccountDeactivated => (
                StatusCode::UNAUTHORIZED,
                "AccountDeactivated",
                "This account is deactivated by admin.",
            ),
            AuthError::SsoProviderNotConfigured => (
              StatusCode::CONFLICT,
                "SsoProviderNotConfigured",
                "Sso provider not configured correctly",
            ),
            AuthError::SsoProviderDisabledByAdmin => (
              StatusCode::FORBIDDEN,
                "SsoProviderDisabledByAdmin",
                "The Sso provider has been disabled by Admin",
            ),
        };

        let error_response = ErrorResponse {
            detail: ErrorDetailVariant::Rich(ErrorDetail {
                code: code.to_string(),
                message: message.to_string(),
            }),
        };

        (status, Json(error_response)).into_response()
    }
}
