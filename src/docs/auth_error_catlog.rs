use std::collections::BTreeMap;
use utoipa::ToSchema;
use uuid::Uuid;
use crate::auth::error::{AuthError, AuthErrorCode, ErrorDetail};

#[derive(Debug, serde::Serialize, ToSchema)]
pub struct AuthErrorCatalogItem {
    /// numeric internal code (same as AuthErrorCode)
    #[schema(value_type = u32, example = 6100)]
    pub code: AuthErrorCode,

    /// recommended HTTP status for this error
    #[schema(value_type = u16, example = 401)]
    pub http_status: u16,

    /// translation keys
    pub description_key: String,
    pub solution_key: String,

    /// example params for interpolation (e.g. {app}, {provider}, {org_id}, {redirect_uri}, {domain})
    #[schema(value_type = Object)]
    pub params_example: BTreeMap<String, String>,

    /// optional vendor/external code example
    pub external_code_example: Option<String>,
}

impl AuthErrorCatalogItem {
    fn from_detail(status: axum::http::StatusCode, detail: ErrorDetail) -> Self {
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

pub fn build_auth_error_catalog() -> Vec<AuthErrorCatalogItem> {
    let mut items = Vec::new();

    // --- simple variants (no params) ---
    for e in [
        AuthError::ServiceTemporarilyUnavailable,
        AuthError::DbUnavailable,
        AuthError::DbTimeout,
        AuthError::DbConflict,
        AuthError::DbNotFound,
        AuthError::InvalidCredentials,
        AuthError::EmailDoesNotExist,
        AuthError::MissingCredentials,
        AuthError::InvalidToken,
        AuthError::InvalidCallbackParameters,
        AuthError::PermissionDenied,
        AuthError::EmailAlreadyExist,
        AuthError::ResourceNotFound,
        AuthError::InvalidUserStatus,
        AuthError::AccountDeactivated,
    ] {
        let (status, detail) = e.to_detail();
        items.push(AuthErrorCatalogItem::from_detail(status, detail));
    }

    // --- provider / oauth / redirect params ---
    for e in [
        AuthError::InvalidProvider {
            provider: Some("azure".to_string()),
        },
        AuthError::InvalidProvider {
            provider: Some("google".to_string()),
        },
        AuthError::InvalidRedirectUri {
            redirect_uri: Some("https://example.com/callback".to_string()),
        },
        AuthError::InvalidRedirectUri { redirect_uri: None },
    ] {
        let (status, detail) = e.to_detail();
        items.push(AuthErrorCatalogItem::from_detail(status, detail));
    }

    // --- org_id param (Uuid) ---
    for e in [
        AuthError::OrgDoesNotExist {
            org_id: Some(Uuid::nil()),
        },
        AuthError::OrgDoesNotExist { org_id: None },
    ] {
        let (status, detail) = e.to_detail();
        items.push(AuthErrorCatalogItem::from_detail(status, detail));
    }

    // --- SSO provider params ---
    for e in [
        AuthError::SsoProviderNotConfigured {
            provider: Some("azure".to_string()),
        },
        AuthError::SsoProviderNotConfigured {
            provider: Some("google".to_string()),
        },
        AuthError::SsoProviderDisabledByAdmin {
            provider: Some("azure".to_string()),
        },
        AuthError::SsoProviderDisabledByAdmin {
            provider: Some("google".to_string()),
        },
    ] {
        let (status, detail) = e.to_detail();
        items.push(AuthErrorCatalogItem::from_detail(status, detail));
    }

    // --- email domain param ---
    for e in [
        AuthError::EmailDomainNotAllowed {
            domain: Some("example.com".to_string()),
        },
        AuthError::EmailDomainNotAllowed { domain: None },
    ] {
        let (status, detail) = e.to_detail();
        items.push(AuthErrorCatalogItem::from_detail(status, detail));
    }

    // stable ordering + de-dup by code
    items.sort_by_key(|x| x.code as u32);
    items.dedup_by_key(|x| x.code as u32);
    items
}

