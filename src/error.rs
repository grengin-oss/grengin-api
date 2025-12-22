use axum::{Json, response::{IntoResponse, Response}};
use reqwest::StatusCode;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub enum AppError {
    ServiceTemporarilyUnavailable,
    ResourceNotFound,
    InvalidLlmProvider,
    LlmProviderNotConfigured,
    NoMessageInRequest,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Serialize,  ToSchema)]
#[serde(untagged)]
pub enum ErrorDetailVariant {
    Simple(String),
    Rich(ErrorDetail),
}

#[derive(Debug, Serialize,  ToSchema)]
pub struct ErrorResponse {
    pub detail: ErrorDetailVariant,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            AppError::ServiceTemporarilyUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "ServiceTemporarilyUnavailable",
                "The service is temporarily unavailable. Please try again in a few minutes.",
            ),
            AppError::ResourceNotFound => (
                StatusCode::NOT_FOUND,
                "ResourceNotFound",
                "The requested resource was not found.",
            ),
            AppError::InvalidLlmProvider => (
                StatusCode::BAD_REQUEST,
                "InvalidLlmProvider",
                "The specified LLM provider is invalid. Please check the provider name and try again.",
            ),
            AppError::LlmProviderNotConfigured => (
                StatusCode::BAD_REQUEST,
                "LlmProviderNotConfigured",
                "The specified LLM provider is not configured. Please configure it before use.",
            ),
            AppError::NoMessageInRequest => (
                StatusCode::BAD_REQUEST,
                "NoMessageInRequest",
                "Missing required field: `message`.",
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