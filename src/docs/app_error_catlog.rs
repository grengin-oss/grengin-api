use axum::{http::StatusCode};
use std::collections::BTreeMap;
use utoipa::ToSchema;
use crate::error::{AppError, ErrorCode, ErrorDetail};

#[derive(Debug, serde::Serialize, ToSchema)]
pub struct AppErrorCatalogItem {
    /// numeric internal code (same as ErrorCode)
    #[schema(value_type = u32, example = 5001)]
    pub code: ErrorCode,

    /// recommended HTTP status for this error
    #[schema(value_type = u16, example = 503)]
    pub http_status: u16,

    /// translation keys
    pub description_key: String,
    pub solution_key: String,

    /// example params for interpolation (e.g. {app}, {provider}, {field})
    #[schema(value_type = Object)]
    pub params_example: BTreeMap<String, String>,

    /// optional vendor/external code example
    pub external_code_example: Option<String>,
}

impl AppErrorCatalogItem {
    fn from_detail(status: StatusCode, detail: ErrorDetail) -> Self {
        Self {
            code: detail.code,
            http_status: status.as_u16(),
            description_key: detail.description_key,
            solution_key: detail.solution_key,
            params_example: detail.params,
            external_code_example: detail.external_code,
        }
    }
}

// Build the catalog list (one representative example per error)
pub fn build_app_error_catalog() -> Vec<AppErrorCatalogItem> {
    let mut items = Vec::new();

    // simple variants
    for e in [
        AppError::ServiceTemporarilyUnavailable,
        AppError::ResourceNotFound,
        AppError::DbUnavailable,
        AppError::DbTimeout,
        AppError::DbConflict,
        AppError::DbNotFound,
    ] {
        let (status, detail) = e.to_detail();
        items.push(AppErrorCatalogItem::from_detail(status, detail));
    }

    // field-based (pick representative fields)
    for e in [
        AppError::ValidationMissingField { field: "messages" },
        AppError::ValidationEmptyField { field: "messages" },
    ] {
        let (status, detail) = e.to_detail();
        items.push(AppErrorCatalogItem::from_detail(status, detail));
    }

    // sso provider examples: azure + google
    for e in [
        AppError::SsoSigninBlockedConditionalAccess {
            provider: "azure".to_string(),
            external_code: Some("53003"),
        },
        AppError::SsoSigninBlockedConditionalAccess {
            provider: "google".to_string(),
            external_code: None,
        },
    ] {
        let (status, detail) = e.to_detail();
        items.push(AppErrorCatalogItem::from_detail(status, detail));
    }

    // llm provider examples: openai + anthropic
    for e in [
        AppError::InvalidLlmProvider {
            provider: "openai".to_string(),
        },
        AppError::LlmProviderNotConfigured {
            provider: "anthropic".to_string(),
        },
        AppError::LlmProviderDisabledByAdmin {
            provider: "openai".to_string(),
        },
    ] {
        let (status, detail) = e.to_detail();
        items.push(AppErrorCatalogItem::from_detail(status, detail));
    }

    // Optional: stable ordering
    items.sort_by_key(|x| x.code as u32);
    items.dedup_by_key(|x| x.code as u32);

    items
}

