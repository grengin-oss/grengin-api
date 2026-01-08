use axum::{Json, response::IntoResponse};
use crate::docs::{app_error_catlog::{AppErrorCatalogItem, build_app_error_catalog}, auth_error_catlog::{AuthErrorCatalogItem, build_auth_error_catalog}};

#[utoipa::path(
    get,
    path = "/errors/app",
    tag = "errors",
    responses(
        (status = 200, content_type = "application/json", body = Vec<AppErrorCatalogItem>)
    )
)]
pub async fn get_app_error_catalog() -> impl IntoResponse {
    Json(build_app_error_catalog())
} 


#[utoipa::path(
    get,
    path = "/errors/auth",
    tag = "errors",
    responses(
        (status = 200, content_type = "application/json", body = Vec<AuthErrorCatalogItem>)
    )
)]
pub async fn get_auth_error_catalog() -> impl IntoResponse {
    Json(build_auth_error_catalog())
}