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
    OrgDoesNotExist
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            AuthError::ServiceTemporarilyUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "ServiceTemporarilyUnavailable",
                "Oops! We're experiencing some technical issues. Please try again later."
            ),
            AuthError::InvalidCredentials => (
                StatusCode::UNAUTHORIZED,
                "InvalidCredentials",
                "The email or password you entered is incorrect. Please try again."
            ),
            AuthError::EmailDoesNotExist => (
                StatusCode::NOT_FOUND,
                "EmailDoesNotExist",
                "The email address you entered isn't registered with us. Please try again or sign up for a new account."
            ),
            AuthError::MissingCredentials => (
                StatusCode::BAD_REQUEST,
                "MissingCredentials",
                "Missing credentials. Please provide all required fields."
            ),
            AuthError::InvalidToken => (
                StatusCode::UNAUTHORIZED,
                "InvalidToken",
                "Invalid token. Please login again."
            ),
            AuthError::InvalidProvider => (
                StatusCode::BAD_REQUEST,
                "InvalidProvider",
                "Invalid or unsupported authentication provider."
            ),
            AuthError::InvalidCallbackParameters => (
                StatusCode::BAD_REQUEST,
                "InvalidCallbackParameters",
                "Invalid callback parameters. The authentication flow may have been interrupted."
            ),
            AuthError::InvalidRedirectUri => (
                StatusCode::BAD_REQUEST,
                "InvalidRedirectUri",
                "Invalid or not supported redirect uri"

            ),
            AuthError::PermissionDenied => (
                StatusCode::BAD_REQUEST,
                "PermissionDenied",
                "Admin role required"

            ),
            AuthError::OrgDoesNotExist =>  (
                StatusCode::NOT_FOUND,
                "OrgDoesNotExist",
                "Organization does not exist"
            ),
            AuthError::EmailAlreadyExist => (
                StatusCode::CONFLICT,
                "EmailAlreadyExist",
                "Email already registered"
            ),
        };

        let error_response = ErrorResponse {
            detail: ErrorDetailVariant::Rich(ErrorDetail {
                code: code.to_string(),
                message: message.to_string(),
            }),
        };

        return (status, Json(error_response)).into_response()
    }
}