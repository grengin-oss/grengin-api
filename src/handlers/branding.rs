use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
        permissions::PERMISSION_AI_PLATFORM_MANAGE,
    },
    dto::branding::{Branding, BrandingUpdate},
    models::branding,
    services::authorization::{AuthorizationService, PermissionScopeMode},
    state::SharedState,
};
use axum::{Json, extract::State};
use chrono::Utc;
use reqwest::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait, IntoActiveModel};
use uuid::Uuid;

fn create_default_branding() -> branding::Model {
    branding::Model {
        id: Uuid::new_v4(),
        name: "Grengin".into(),
        logo_url: None,
        color_primary: "#4079c5".into(),
        color_accent: "#2d906b".into(),
        font_family: "Coustard".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

fn model_to_response(model: &branding::Model) -> Branding {
    Branding {
        name: model.name.clone(),
        logo_url: model.logo_url.clone(),
        color_primary: model.color_primary.clone(),
        color_accent: model.color_accent.clone(),
        font_family: model.font_family.clone(),
    }
}

async fn get_or_create_branding(app_state: &SharedState) -> Result<branding::Model, AuthError> {
    let branding_model = branding::Entity::find()
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("branding query error: {e}");
            AuthError::DbTimeout
        })?;

    if let Some(model) = branding_model {
        Ok(model)
    } else {
        let default_branding = create_default_branding();
        default_branding
            .clone()
            .into_active_model()
            .insert(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("branding insert error: {e}");
                AuthError::DbTimeout
            })?;
        Ok(default_branding)
    }
}

/// Get branding configuration (public, no auth required)
#[utoipa::path(
    get,
    path = "/branding",
    tag = "branding",
    responses(
       (status = 200, body = Branding),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000)"),
    )
)]
pub async fn get_branding(
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<Branding>), AuthError> {
    let branding = get_or_create_branding(&app_state).await?;
    Ok((StatusCode::OK, Json(model_to_response(&branding))))
}

/// Get branding configuration (admin)
#[utoipa::path(
    get,
    path = "/admin/branding",
    tag = "admin",
    responses(
       (status = 200, body = Branding),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 403, content_type = "application/json", body = Error, description = "Permission denied"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000)"),
    )
)]
pub async fn get_admin_branding(
    claims: Claims,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<Branding>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AI_PLATFORM_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;
    let branding = get_or_create_branding(&app_state).await?;
    Ok((StatusCode::OK, Json(model_to_response(&branding))))
}

/// Update branding configuration (admin)
#[utoipa::path(
    put,
    path = "/admin/branding",
    tag = "admin",
    request_body = BrandingUpdate,
    responses(
       (status = 200, body = Branding),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 403, content_type = "application/json", body = Error, description = "Permission denied"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000)"),
    )
)]
pub async fn update_branding(
    claims: Claims,
    State(app_state): State<SharedState>,
    Json(req): Json<BrandingUpdate>,
) -> Result<(StatusCode, Json<Branding>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AI_PLATFORM_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let branding_model = get_or_create_branding(&app_state).await?;
    let mut active_model = branding_model.into_active_model();

    if let Some(name) = req.name {
        active_model.name = Set(name);
    }
    if let Some(logo_url) = req.logo_url {
        active_model.logo_url = Set(Some(logo_url));
    }
    if let Some(color_primary) = req.color_primary {
        active_model.color_primary = Set(color_primary);
    }
    if let Some(color_accent) = req.color_accent {
        active_model.color_accent = Set(color_accent);
    }
    if let Some(font_family) = req.font_family {
        active_model.font_family = Set(font_family);
    }
    active_model.updated_at = Set(Utc::now());

    let updated = active_model
        .update(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("branding update error: {e}");
            AuthError::DbTimeout
        })?;

    Ok((StatusCode::OK, Json(model_to_response(&updated))))
}
