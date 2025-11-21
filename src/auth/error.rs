use axum::{response::{IntoResponse, Response},http::StatusCode};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub enum AuthError {
    ServiceTemporarilyUnavailable,
    InvalidCredentials,
    EmailDoesNotExist,
    MissingCredentials,
    InvalidToken,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            AuthError::ServiceTemporarilyUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Oops! We're experiencing some technical issues. Please try again later."
            ),
            AuthError::InvalidCredentials => (
                StatusCode::UNAUTHORIZED,
                "The email or password you entered is incorrect. Please try again."
            ),
            AuthError::EmailDoesNotExist => (
                StatusCode::NOT_FOUND,
                "The email address you entered isn't registered with us. Please try again or sign up for a new account."
            ),
            AuthError::MissingCredentials => (
                StatusCode::BAD_REQUEST,
                "Missing credentials. Please provide all required fields."
            ),
            AuthError::InvalidToken => (
                StatusCode::UNAUTHORIZED,
                "Invalid token. Please login again."
            ),
        };
        return (status,error_message).into_response()
    }
}