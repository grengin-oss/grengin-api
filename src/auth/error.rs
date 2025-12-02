use axum::{response::{IntoResponse, Response},http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub enum AuthError {
    ServiceTemporarilyUnavailable,
    InvalidCredentials,
    EmailDoesNotExist,
    MissingCredentials,
    InvalidToken,
    InvalidProvider,
    InvalidCallbackParameters,
    InvalidRedirectUri
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum ErrorDetailVariant {
    Simple(String),
    Rich(ErrorDetail),
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ErrorResponse {
    pub detail: ErrorDetailVariant,
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