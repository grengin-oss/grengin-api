use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{ser::Serializer, Serialize};
use std::collections::BTreeMap;
use utoipa::ToSchema;

pub const APP_NAME: &str = "grengin";

/// Numeric, stable internal codes.
/// IMPORTANT: set every value explicitly so inserting variants later doesn’t change codes.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, ToSchema)]
pub enum ErrorCode {
    // 1000-1999: generic/platform
    ServiceTemporarilyUnavailable = 1000,
    ResourceNotFound = 1001,

    // 5000-5999: DB
    DbUnavailable = 5000,
    DbTimeout = 5001,
    DbConflict = 5002,
    DbNotFound = 5003,

    // 2000-2999: validation
    ValidationMissingField = 2001,
    ValidationEmptyField = 2002,

    // 3000-3999: SSO
    SsoSigninBlockedConditionalAccess = 3001,

    // 4000-4999: LLM
    InvalidLlmProvider = 4001,
    LlmProviderNotConfigured = 4002,
    LlmProviderDisabledByAdmin = 4003,
}

impl Serialize for ErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u32(*self as u32)
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorResponse {
    pub detail: ErrorDetailVariant,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ErrorDetailVariant {
    Rich(ErrorDetail),
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorDetail {
    /// Serialized as integer (e.g. 3001)
    #[schema(value_type = u32, example = 3001)]
    pub code: ErrorCode,

    /// English fallback
    pub description: String,
    pub solution: String,

    /// Translation keys
    pub description_key: String,
    pub solution_key: String,

    /// Placeholder values for translation rendering (e.g. {provider}, {app})
    #[schema(value_type = Object)]
    pub params: BTreeMap<String, String>,

    /// Vendor / external code if relevant (e.g. Microsoft Entra: "53003")
    pub external_code: Option<String>,
}

/// AppError only (your handlers return Result<T, AppError>)
#[derive(Debug, ToSchema)]
pub enum AppError {
    ServiceTemporarilyUnavailable,
    ResourceNotFound,

    // DB errors
    DbUnavailable,
    DbTimeout,
    DbConflict,
    DbNotFound,

    /// Generic missing field. Use for things like "message" etc.
    ValidationMissingField { field: &'static str },
    ValidationEmptyField { field: &'static str },

    /// Microsoft-style conditional access block.
    /// - `external_code`: set to Some("53003") if you want to mirror Microsoft codes
    SsoSigninBlockedConditionalAccess {
        provider: String,
        external_code: Option<&'static str>,
    },

    InvalidLlmProvider { provider: String },
    LlmProviderNotConfigured { provider: String },
    LlmProviderDisabledByAdmin { provider: String },
}

impl AppError {
    pub fn missing_field(field: &'static str) -> Self {
        Self::ValidationMissingField { field }
    }

    pub fn empty_field(field: &'static str) -> Self {
        Self::ValidationEmptyField { field }
    }

    pub fn sso_conditional_access(provider: String, external_code: Option<&'static str>) -> Self {
        Self::SsoSigninBlockedConditionalAccess {
            provider,
            external_code,
        }
    }

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
            // -------- generic/platform --------
            AppError::ServiceTemporarilyUnavailable => {
                let params = Self::base_params();
                let description_key = "error.service_unavailable.description".to_string();
                let solution_key = "error.service_unavailable.solution".to_string();

                let description_tpl = "The {app} service is temporarily unavailable.";
                let solution_tpl =
                    "Try again in a few minutes. If it persists, contact support.";

                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    ErrorDetail {
                        code: ErrorCode::ServiceTemporarilyUnavailable,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AppError::ResourceNotFound => {
                let params = Self::base_params();
                let description_key = "error.not_found.description".to_string();
                let solution_key = "error.not_found.solution".to_string();

                let description_tpl = "The requested resource was not found.";
                let solution_tpl = "Check the URL and try again.";

                (
                    StatusCode::NOT_FOUND,
                    ErrorDetail {
                        code: ErrorCode::ResourceNotFound,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            // -------- DB --------
            AppError::DbUnavailable => {
                let params = Self::base_params();
                let description_key = "error.db.unavailable.description".to_string();
                let solution_key = "error.db.unavailable.solution".to_string();

                let description_tpl = "{app} couldn't reach the database right now.";
                let solution_tpl =
                    "Try again in a few minutes. If it persists, check database health, network, and connection pool limits.";

                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    ErrorDetail {
                        code: ErrorCode::DbUnavailable,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AppError::DbTimeout => {
                let params = Self::base_params();
                let description_key = "error.db.timeout.description".to_string();
                let solution_key = "error.db.timeout.solution".to_string();

                let description_tpl = "A database operation timed out in {app}.";
                let solution_tpl =
                    "Retry the request. If it continues, investigate database load, slow queries, and connection pool timeouts.";

                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    ErrorDetail {
                        code: ErrorCode::DbTimeout,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AppError::DbConflict => {
                let params = Self::base_params();
                let description_key = "error.db.conflict.description".to_string();
                let solution_key = "error.db.conflict.solution".to_string();

                let description_tpl =
                    "The request conflicts with existing data in {app}.";
                let solution_tpl =
                    "Refresh data and retry. If you're creating a resource, ensure unique fields are not duplicated.";

                (
                    StatusCode::CONFLICT,
                    ErrorDetail {
                        code: ErrorCode::DbConflict,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AppError::DbNotFound => {
                let params = Self::base_params();
                let description_key = "error.db.not_found.description".to_string();
                let solution_key = "error.db.not_found.solution".to_string();

                let description_tpl = "The requested data was not found in {app}.";
                let solution_tpl = "Verify the identifier and try again.";

                (
                    StatusCode::NOT_FOUND,
                    ErrorDetail {
                        code: ErrorCode::DbNotFound,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            // -------- validation --------
            AppError::ValidationMissingField { field } => {
                let mut params = Self::base_params();
                params.insert("field".to_string(), (*field).to_string());

                let description_key = "error.validation.missing_field.description".to_string();
                let solution_key = "error.validation.missing_field.solution".to_string();

                let description_tpl = "Missing required field: `{field}`.";
                let solution_tpl = "Include `{field}` in the request body and try again.";

                (
                    StatusCode::BAD_REQUEST,
                    ErrorDetail {
                        code: ErrorCode::ValidationMissingField,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AppError::ValidationEmptyField { field } => {
                let mut params = Self::base_params();
                params.insert("field".to_string(), (*field).to_string());

                let description_key = "error.validation.empty_field.description".to_string();
                let solution_key = "error.validation.empty_field.solution".to_string();

                let description_tpl = "Empty array field required items: `{field}`.";
                let solution_tpl =
                    "Include items in array `{field}` in the request body and try again.";

                (
                    StatusCode::BAD_REQUEST,
                    ErrorDetail {
                        code: ErrorCode::ValidationEmptyField,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            // -------- SSO --------
            AppError::SsoSigninBlockedConditionalAccess {
                provider,
                external_code,
            } => {
                let mut params = Self::base_params();
                params.insert("provider".to_string(), provider.clone());

                let description_key = "error.sso.conditional_access.description".to_string();
                let solution_key = "error.sso.conditional_access.solution".to_string();

                let description_tpl = "Your sign-in attempt to {provider} was blocked by a Conditional Access policy set by your organization's IT admin.";
                let solution_tpl = "If you're a user: contact your IT admin or try from an allowed device/network/location. If you're an admin: review Conditional Access policies and sign-in logs.";

                (
                    StatusCode::FORBIDDEN,
                    ErrorDetail {
                        code: ErrorCode::SsoSigninBlockedConditionalAccess,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: external_code.map(|s| s.to_string()),
                    },
                )
            }

            // -------- LLM --------
            AppError::InvalidLlmProvider { provider } => {
                let mut params = Self::base_params();
                params.insert("provider".to_string(), provider.clone());

                let description_key = "error.llm.invalid_provider.description".to_string();
                let solution_key = "error.llm.invalid_provider.solution".to_string();

                let description_tpl = "Invalid LLM provider: `{provider}`.";
                let solution_tpl = "Use a supported LLM provider for {app}.";

                (
                    StatusCode::BAD_REQUEST,
                    ErrorDetail {
                        code: ErrorCode::InvalidLlmProvider,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AppError::LlmProviderNotConfigured { provider } => {
                let mut params = Self::base_params();
                params.insert("provider".to_string(), provider.clone());

                let description_key = "error.llm.not_configured.description".to_string();
                let solution_key = "error.llm.not_configured.solution".to_string();

                let description_tpl = "The LLM provider `{provider}` is not configured for {app}.";
                let solution_tpl =
                    "Ask an admin to configure `{provider}` credentials, then try again.";

                (
                    StatusCode::BAD_REQUEST,
                    ErrorDetail {
                        code: ErrorCode::LlmProviderNotConfigured,
                        description: Self::render(description_tpl, &params),
                        solution: Self::render(solution_tpl, &params),
                        description_key,
                        solution_key,
                        params,
                        external_code: None,
                    },
                )
            }

            AppError::LlmProviderDisabledByAdmin { provider } => {
                let mut params = Self::base_params();
                params.insert("provider".to_string(), provider.clone());

                let description_key = "error.llm.disabled_by_admin.description".to_string();
                let solution_key = "error.llm.disabled_by_admin.solution".to_string();

                let description_tpl = "The LLM provider `{provider}` is disabled by an administrator.";
                let solution_tpl =
                    "Ask an admin to enable `{provider}` or select a different provider.";

                (
                    StatusCode::FORBIDDEN,
                    ErrorDetail {
                        code: ErrorCode::LlmProviderDisabledByAdmin,
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

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, detail) = self.to_detail();
        let body = ErrorResponse {
            detail: ErrorDetailVariant::Rich(detail),
        };
        (status, Json(body)).into_response()
    }
}
