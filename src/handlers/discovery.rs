// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
};
use serde::Serialize;

use crate::{
    auth::error::AuthError,
    dto::discovery::{
        AiProviderDiscoveryResponse, AuthProviderDiscoveryResponse, DiscoveryListResponse,
        DiscoveryQuery,
    },
    services::discovery_catalog::{DiscoveryError, sha256_hex},
    state::SharedState,
};

const PUBLIC_CACHE_CONTROL: &str = "public, max-age=300, must-revalidate";

fn map_discovery_error(error: DiscoveryError) -> AuthError {
    match error {
        DiscoveryError::InvalidVersion => AuthError::InvalidRequest { field: "version" },
        DiscoveryError::NotFound => AuthError::ResourceNotFound,
        DiscoveryError::Unavailable(message) | DiscoveryError::InvalidCatalog(message) => {
            eprintln!("provider discovery failed: {message}");
            AuthError::ServiceTemporarilyUnavailable
        }
    }
}

fn etag_matches(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(',').any(|candidate| {
                let candidate = candidate.trim();
                candidate == etag || candidate.strip_prefix("W/") == Some(etag)
            })
        })
}

fn discovery_response<T: Serialize>(headers: &HeaderMap, value: &T) -> Result<Response, AuthError> {
    let body = serde_json::to_vec(value).map_err(|error| {
        eprintln!("provider discovery response serialization failed: {error}");
        AuthError::ServiceTemporarilyUnavailable
    })?;
    let etag = format!("\"sha256-{}\"", sha256_hex(&body));
    let status = if etag_matches(headers, &etag) {
        StatusCode::NOT_MODIFIED
    } else {
        StatusCode::OK
    };
    let response_body = if status == StatusCode::NOT_MODIFIED {
        Body::empty()
    } else {
        Body::from(body)
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, PUBLIC_CACHE_CONTROL)
        .header(header::ETAG, etag)
        .body(response_body)
        .map_err(|error| {
            eprintln!("provider discovery response construction failed: {error}");
            AuthError::ServiceTemporarilyUnavailable
        })
}

#[utoipa::path(
    get,
    path = "/discovery/auth-providers",
    tag = "discovery",
    params(DiscoveryQuery),
    responses(
        (status = 200, body = DiscoveryListResponse, description = "Supported authentication provider templates"),
        (status = 304, description = "Catalog has not changed"),
        (status = 400, description = "Invalid version selector"),
        (status = 503, description = "Provider catalog unavailable"),
    )
)]
pub async fn list_auth_provider_templates(
    State(state): State<SharedState>,
    Query(query): Query<DiscoveryQuery>,
    headers: HeaderMap,
) -> Result<Response, AuthError> {
    let response = state
        .discovery_catalog
        .list_auth(query.version.as_deref())
        .await
        .map_err(map_discovery_error)?;
    discovery_response(&headers, &response)
}

#[utoipa::path(
    get,
    path = "/discovery/auth-providers/{provider}",
    tag = "discovery",
    params(
        ("provider" = String, Path, description = "Authentication provider ID"),
        DiscoveryQuery,
    ),
    responses(
        (status = 200, body = AuthProviderDiscoveryResponse, description = "Versioned authentication provider template"),
        (status = 304, description = "Template has not changed"),
        (status = 404, description = "No compatible template found"),
        (status = 503, description = "Provider catalog unavailable"),
    )
)]
pub async fn get_auth_provider_template(
    State(state): State<SharedState>,
    Path(provider): Path<String>,
    Query(query): Query<DiscoveryQuery>,
    headers: HeaderMap,
) -> Result<Response, AuthError> {
    let response = state
        .discovery_catalog
        .auth_provider(&provider, query.version.as_deref())
        .await
        .map_err(map_discovery_error)?;
    discovery_response(&headers, &response)
}

#[utoipa::path(
    get,
    path = "/discovery/ai-providers",
    tag = "discovery",
    params(DiscoveryQuery),
    responses(
        (status = 200, body = DiscoveryListResponse, description = "Supported AI provider plugins"),
        (status = 304, description = "Catalog has not changed"),
        (status = 400, description = "Invalid version selector"),
        (status = 503, description = "Provider catalog unavailable"),
    )
)]
pub async fn list_ai_provider_plugins(
    State(state): State<SharedState>,
    Query(query): Query<DiscoveryQuery>,
    headers: HeaderMap,
) -> Result<Response, AuthError> {
    let response = state
        .discovery_catalog
        .list_ai(query.version.as_deref())
        .await
        .map_err(map_discovery_error)?;
    discovery_response(&headers, &response)
}

#[utoipa::path(
    get,
    path = "/discovery/ai-providers/{provider}",
    tag = "discovery",
    params(
        ("provider" = String, Path, description = "AI provider ID"),
        DiscoveryQuery,
    ),
    responses(
        (status = 200, body = AiProviderDiscoveryResponse, description = "Versioned AI provider plugin"),
        (status = 304, description = "Plugin has not changed"),
        (status = 404, description = "No compatible plugin found"),
        (status = 503, description = "Provider catalog unavailable"),
    )
)]
pub async fn get_ai_provider_plugin(
    State(state): State<SharedState>,
    Path(provider): Path<String>,
    Query(query): Query<DiscoveryQuery>,
    headers: HeaderMap,
) -> Result<Response, AuthError> {
    let response = state
        .discovery_catalog
        .ai_provider(&provider, query.version.as_deref())
        .await
        .map_err(map_discovery_error)?;
    discovery_response(&headers, &response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn response_etag_supports_conditional_get() {
        let value = DiscoveryListResponse {
            distribution_version: env!("CARGO_PKG_VERSION").to_string(),
            catalog_type: "auth_providers".to_string(),
            catalog_version: "1.0.0".to_string(),
            providers: Vec::new(),
        };
        let first = discovery_response(&HeaderMap::new(), &value).expect("discovery response");
        let etag = first.headers()[header::ETAG].clone();
        assert_eq!(first.status(), StatusCode::OK);
        assert!(
            !to_bytes(first.into_body(), usize::MAX)
                .await
                .expect("response body")
                .is_empty()
        );

        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag);
        let second = discovery_response(&headers, &value).expect("conditional response");
        assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
        assert!(
            to_bytes(second.into_body(), usize::MAX)
                .await
                .expect("response body")
                .is_empty()
        );
    }
}
