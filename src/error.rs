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
                "Oops! We're experiencing some technical issues. Please try again later."
            ),
            AppError::ResourceNotFound =>  (
                StatusCode::NOT_FOUND,
                "ResourceNotFound",
                "Resource not found"
            ),
            AppError::InvalidLlmProvider =>  (
                StatusCode::BAD_REQUEST,
                "InvalidLlmProvider",
                "Invalid llm provider"
            ),
            AppError::LlmProviderNotConfigured =>  (
                StatusCode::BAD_REQUEST,
                "LlmProviderNotConfigured",
                "Not configured llm provider"
            ),
            AppError::NoMessageInRequest =>  (
                StatusCode::BAD_REQUEST,
                "NoMessageInRequest",
                "No Message found in request"
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