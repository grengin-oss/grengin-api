use axum::{Json, extract::State};
use chrono::Utc;
use reqwest::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait, IntoActiveModel, TryIntoModel};
use uuid::Uuid;
use crate::{auth::{claims::Claims, error::AuthError}, dto::admin_org::{OrgRequest, OrgResponse, OrgSettings}, models::{organizations, users::UserRole}, state::SharedState};

#[utoipa::path(
    get,
    path = "/admin/organization",
    tag = "admin",
    responses(
        (status = 204, description = "User deleted"),
        (status = 404, description = "Resource not found"),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
    )
)]
pub async fn get_org(
    claims: Claims,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode,Json<OrgResponse>), AuthError> {
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
    }
    let org_model = organizations::Entity::find()
      .one(&app_state.database)
      .await
      .map_err(|e| {
         eprintln!("insert error: {e}");
         AuthError::ServiceTemporarilyUnavailable
        })?;

    let org = if let Some(model) = org_model{
        model
    }else {
        let default_org = organizations::Model{ 
            id: Uuid::new_v4(),
            name:"grengin".into(),
            domain: "grengin.com".into(),
            allowed_domains: vec![],
            sso_providers:vec!["azure".into(),"google".into()],
            logo_url:None,
            default_engine:"openai".into(),
            default_model:"gpt-5.1".into(),
            data_retention_days:90,
            require_mfa:false,
            created_on:Utc::now(),
            updated_on:Utc::now(), 
        };
        default_org
          .clone()
          .into_active_model()
          .insert(&app_state.database)
          .await
          .map_err(|e| {
            eprintln!("insert error: {e}");
            AuthError::ServiceTemporarilyUnavailable
        })?;
        default_org
    };
    
    let org_response = OrgResponse { 
        id: org.id,
        name: org.name,
        domain: org.domain,
        allowed_domains:Vec::new(),
        logo_url:org.logo_url,
        settings:OrgSettings { 
            sso_providers:org.sso_providers,
            default_engine:org.default_engine,
            default_model:org.default_model,
            data_retention_days:org.data_retention_days,
            require_mfa: org.require_mfa 
        },
        created_at:org.created_on,
        updated_at:org.updated_on, 
    };                     
    Ok((StatusCode::OK,Json(org_response)))
}

#[utoipa::path(
    put,
    path = "/admin/organization",
    tag = "admin",
    request_body = OrgRequest,
    responses(
        (status = 204, description = "User deleted"),
        (status = 404, description = "Resource not found"),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
    )
)]
pub async fn update_org(
    claims: Claims,
    State(app_state): State<SharedState>,
    Json(req): Json<OrgRequest>
) -> Result<(StatusCode,Json<OrgResponse>), AuthError> {
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
    }
    let org_model= organizations::Entity::find()
      .one(&app_state.database)
      .await
          .map_err(|e| {
        eprintln!("insert error: {e}");
        AuthError::ServiceTemporarilyUnavailable
       })?
      .ok_or(AuthError::OrgDoesNotExist)?;
    let mut active_model = org_model
       .into_active_model();
    active_model.allowed_domains = Set(req.allowed_domains);
    active_model.updated_on = Set(Utc::now());
    active_model.data_retention_days = Set(req.settings.data_retention_days);
    active_model.default_engine = Set(req.settings.default_engine);
    active_model.default_model = Set(req.settings.default_model);
    active_model.sso_providers = Set(req.settings.sso_providers);
    active_model.require_mfa = Set(req.settings.require_mfa);
    active_model.logo_url = Set(req.logo_url);
    active_model.name = Set(req.name);
    active_model
      .clone()
      .update(&app_state.database)
      .await
      .map_err(|e| {
          eprintln!("update error: {e}");
          AuthError::ServiceTemporarilyUnavailable
        })?;
    let org = active_model
      .try_into_model()
      .map_err(|e| {
          eprintln!("model parse error: {e}");
          AuthError::ServiceTemporarilyUnavailable
       })?;
    let org_response = OrgResponse { 
        id: org.id,
        name: org.name,
        domain: org.domain,
        allowed_domains:Vec::new(),
        logo_url:org.logo_url,
        settings:OrgSettings { 
            sso_providers:org.sso_providers,
            default_engine:org.default_engine,
            default_model:org.default_model,
            data_retention_days:org.data_retention_days,
            require_mfa: org.require_mfa 
        },
        created_at:org.created_on,
        updated_at:org.updated_on, 
    };                     
 Ok((StatusCode::OK,Json(org_response)))
}
