use axum::{Json, extract::State};
use reqwest::StatusCode;
use sea_orm::EntityTrait;
use uuid::Uuid;

use crate::{
    auth::{claims::Claims, error::{AuthError, AuthErrorResponse}},
    dto::me::{AdministeredDepartmentsResponse, EffectivePermissionsResponse},
    models::users,
    services::authorization::AuthorizationService,
    state::SharedState,
};

#[utoipa::path(
    get,
    path = "/me/permissions",
    tag = "me",
    responses(
        (status = 200, content_type = "application/json", body = EffectivePermissionsResponse),
        (status = 401, content_type = "application/json", body = AuthErrorResponse),
        (status = 503, content_type = "application/json", body = AuthErrorResponse)
    )
)]
pub async fn get_my_permissions(
    claims: Claims,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<EffectivePermissionsResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);

    let mut user = users::Entity::find_by_id(claims.user_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("user lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    if user.effective_permissions.is_none() {
        authz.recompute_effective_permissions(user.id).await?;
        user = users::Entity::find_by_id(claims.user_id)
            .one(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("user lookup error: {e}");
                AuthError::DbTimeout
            })?
            .ok_or(AuthError::ResourceNotFound)?;
    }

    let effective = user
        .effective_permissions
        .unwrap_or(serde_json::json!({
            "permissions": {},
            "mcp_access": {},
            "administered_departments": [],
        }));

    let permissions = effective
        .get("permissions")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let mcp_access = effective
        .get("mcp_access")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let administered_departments = effective
        .get("administered_departments")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    Ok((StatusCode::OK, Json(EffectivePermissionsResponse {
        permissions,
        mcp_access,
        administered_departments,
    })))
}

#[utoipa::path(
    get,
    path = "/me/administered-departments",
    tag = "me",
    responses(
        (status = 200, body = AdministeredDepartmentsResponse),
        (status = 401, content_type = "application/json", body = AuthErrorResponse),
        (status = 503, content_type = "application/json", body = AuthErrorResponse)
    )
)]
pub async fn get_my_administered_departments(
    claims: Claims,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<AdministeredDepartmentsResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);

    let mut user = users::Entity::find_by_id(claims.user_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("user lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    if user.effective_permissions.is_none() {
        authz.recompute_effective_permissions(user.id).await?;
        user = users::Entity::find_by_id(claims.user_id)
            .one(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("user lookup error: {e}");
                AuthError::DbTimeout
            })?
            .ok_or(AuthError::ResourceNotFound)?;
    }

    let mut departments = Vec::new();
    if let Some(value) = user.effective_permissions {
        if let Some(list) = value.get("administered_departments").and_then(|v| v.as_array()) {
            for item in list {
                if let Some(id_str) = item.as_str() {
                    if let Ok(id) = Uuid::parse_str(id_str) {
                        departments.push(id);
                    }
                }
            }
        }
    }

    Ok((
        StatusCode::OK,
        Json(AdministeredDepartmentsResponse {
            administered_departments: departments,
        }),
    ))
}
