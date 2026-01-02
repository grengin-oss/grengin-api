use axum::{Json, extract::{Path, State}};
use chrono::Utc;
use reqwest::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter};
use uuid::Uuid;
use crate::{auth::{claims::Claims, encryption::{decrypt_key, encrypt_key}, error::AuthError, sso_provider::sso_providers_list}, dto::admin_sso_providers::{SsoProviderResponse, SsoProviderUpdateRequest}, handlers::admin_org::get_org, models::{sso_providers, users::UserRole}, state::SharedState};

#[utoipa::path(
    get,
    path = "/admin/sso-providers",
    tag = "admin",
    responses(
        (status = 200, body = Vec<SsoProviderResponse>),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
    )
)]
pub async fn get_sso_providers(
    claims: Claims,
     State(app_state): State<SharedState>,
) -> Result<(StatusCode,Json<Vec<SsoProviderResponse>>), AuthError> {
     match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
     }
     let mut select = sso_providers::Entity::find();
     let org_id = if let Some(org_id) = claims.org_id {
        select = select.filter(sso_providers::Column::OrgId.eq(org_id));
        org_id
     }else{
        let (_,Json(org)) = get_org(claims,State(app_state.clone()))
         .await
         .map_err(|e|{
           eprintln!("org fetch error: {:?}",e);
           AuthError::ServiceTemporarilyUnavailable
       })?;
       org.id
     };
     let mut models = select
       .all(&app_state.database)
       .await
       .map_err(|e|{
          eprintln!("Db get all error: {:?}",e);
          AuthError::ServiceTemporarilyUnavailable
       })?;
      if models.is_empty(){
         let mut insert_models = Vec::new();
         for sso_provider in sso_providers_list() {
            insert_models.push(sso_providers::Model{ 
                id:Uuid::new_v4(),
                org_id:org_id,
                provider:sso_provider.provider,
                name:sso_provider.name,
                tenant_id:None,
                client_id:"<empty>".to_string(),
                client_secret:"<empty>".to_string(),
                issuer_url:sso_provider.issuer_url,
                redirect_url:sso_provider.redirect_url,
                allowed_domains:Vec::new(),
                is_enabled: false,
                is_default: false,
                created_at: Utc::now(),
                updated_at: Utc::now() 
            });
         }
         models = insert_models.clone();
         let insert_active_models:Vec<_> = insert_models
            .into_iter()
            .map(|m| m.into_active_model())
            .collect();
         sso_providers::Entity::insert_many(insert_active_models.clone())
           .exec(&app_state.database)
           .await
           .map_err(|e|{
             eprintln!("DB insert many error {:?}",e);
             AuthError::ServiceTemporarilyUnavailable
           })?;
      }
      let response = models
        .into_iter()
        .map(|model|{
            let decrypted_client_secret = decrypt_key(&app_state.settings.auth.app_key,&model.client_secret)
              .ok();
           SsoProviderResponse{
             id:model.id,
             redirect_url:model.redirect_url, 
             provider:model.provider,
             name:model.name,
             client_id:model.client_id,
             client_secret:app_state.get_decrypted_api_key_preview(&decrypted_client_secret),
             issuer_url: model.issuer_url,
             allowed_domains:model.allowed_domains,
             is_enabled:model.is_enabled,
             created_at:model.created_at,
             updated_at:model.updated_at,
          }
        }).collect();
  Ok((StatusCode::OK,Json(response)))
}


#[utoipa::path(
    get,
    path = "/admin/sso-providers/{provider_id}",
    tag = "admin",
    responses(
        (status = 200, body = SsoProviderResponse),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
    )
)]
pub async fn get_sso_provider_by_id(
     claims: Claims,
     Path(provider_id):Path<Uuid>,
     State(app_state): State<SharedState>,
) -> Result<(StatusCode,Json<SsoProviderResponse>), AuthError> {
     match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
     }
     let mut select = sso_providers::Entity::find_by_id(provider_id);
     if let Some(org_id) = claims.org_id {
        select = select.filter(sso_providers::Column::OrgId.eq(org_id));
     }
     let model = select
       .one(&app_state.database)
       .await
       .map_err(|e|{
          eprintln!("Db get all error: {}",e);
          AuthError::ServiceTemporarilyUnavailable
       })?;
      let response = model
        .map(|model|{
           let decrypted_client_secret = decrypt_key(&app_state.settings.auth.app_key,&model.client_secret)
             .ok();
            SsoProviderResponse{
              id:model.id, 
              provider:model.provider,
              name:model.name,
              redirect_url:model.redirect_url,
              client_id:model.client_id,
              client_secret:app_state.get_decrypted_api_key_preview(&decrypted_client_secret),
              issuer_url: model.issuer_url,
              allowed_domains:model.allowed_domains,
              is_enabled:model.is_enabled,
              created_at:model.created_at,
              updated_at:model.updated_at,
          }
        }).ok_or(AuthError::ResourceNotFound)?;
  Ok((StatusCode::OK,Json(response)))
}

#[utoipa::path(
    delete,
    path = "/admin/sso-providers/{provider_id}",
    tag = "admin",
    responses(
        (status = 200, body = SsoProviderResponse),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
    )
)]
pub async fn delete_sso_provider_by_id(
     claims: Claims,
     Path(provider_id):Path<Uuid>,
     State(app_state): State<SharedState>,
) -> Result<(StatusCode,&'static str), AuthError> {
     match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
     }
     let mut select = sso_providers::Entity::find_by_id(provider_id);
     if let Some(org_id) = claims.org_id {
        select = select.filter(sso_providers::Column::OrgId.eq(org_id));
     }
     let model = select
       .one(&app_state.database)
       .await
       .map_err(|e|{
          eprintln!("Db get one error: {}",e);
          AuthError::ServiceTemporarilyUnavailable
       })?
      .ok_or(AuthError::ResourceNotFound)?;
      let mut active_model = model
        .into_active_model();
      active_model.client_id = Set("<empty>".to_string());
      active_model.client_secret = Set("<empty>".to_string());
      active_model.updated_at = Set(Utc::now());
      active_model.is_default = Set(false);
      active_model.is_enabled = Set(false);
      active_model.tenant_id = Set(None);
      active_model
        .update(&app_state.database)
        .await
        .map_err(|e|{
           eprintln!("Db get one error: {}",e);
           AuthError::ServiceTemporarilyUnavailable
       })?;
  Ok((StatusCode::OK,"Deleted successfully"))
}

#[utoipa::path(
    delete,
    path = "/admin/sso-providers/{provider_id}",
    tag = "admin",
    responses(
        (status = 200, body = SsoProviderResponse),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
    )
)]
pub async fn update_sso_provider_by_id(
     claims: Claims,
     Path(provider_id):Path<Uuid>,
     State(app_state): State<SharedState>,
     Json(req):Json<SsoProviderUpdateRequest>
) -> Result<(StatusCode,Json<SsoProviderResponse>), AuthError> {
     match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
     }
     let mut select = sso_providers::Entity::find_by_id(provider_id);
     if let Some(org_id) = claims.org_id {
        select = select.filter(sso_providers::Column::OrgId.eq(org_id));
     }
     let model = select
       .one(&app_state.database)
       .await
       .map_err(|e|{
          eprintln!("Db get one error: {}",e);
          AuthError::ServiceTemporarilyUnavailable
       })?
      .ok_or(AuthError::ResourceNotFound)?;
     let mut active_model = model
       .into_active_model();
     if let Some(provider) = req.provider {
        active_model.provider = Set(provider);
     }
     if let Some(name) = req.name {
        active_model.name = Set(name);
     }
     if let Some(allowed_domains) = req.allowed_domains {
        active_model.allowed_domains = Set(allowed_domains);
     }
     if let Some(client_id) = req.client_id {
        active_model.client_id = Set(client_id);
     }
     if let Some(is_enabled) = req.is_enabled{
        active_model.is_enabled = Set(is_enabled);
     }
     if let Some(issuer_url) = req.issuer_url  {
         active_model.issuer_url = Set(issuer_url);
     }
     if let Some(redirect_url) = req.redirect_url  {
        active_model.redirect_url = Set(redirect_url);
     }
     active_model.tenant_id = Set(req.tenant_id);
     if let Some(client_secret) = req.client_secret {
        active_model.client_secret = Set(encrypt_key(&app_state.settings.auth.app_key,client_secret.as_bytes())
         .map_err(|e|{
             eprintln!("Sso key encryption error {:?}",e);
             AuthError::ServiceTemporarilyUnavailable
         })?);
     }
     active_model.updated_at = Set(Utc::now());
     let updated_model = active_model
        .update(&app_state.database)
        .await
        .map_err(|e|{
            eprintln!("Db update error {:?}",e);
            AuthError::ServiceTemporarilyUnavailable
     })?;
     if let Ok(client_secret) = decrypt_key(&app_state.settings.auth.app_key,&updated_model.client_secret)  {
      let allowed_domains = updated_model
        .allowed_domains
        .iter()
        .map(|d| d.into())
        .collect();
      let _ = app_state
        .settings
        .load_sso_provider_in_state(&updated_model.provider,&updated_model.client_id,&client_secret,&updated_model.redirect_url, updated_model.tenant_id.as_ref(), updated_model.is_enabled,allowed_domains)
        .await;
     }
     let response = SsoProviderResponse { 
        id:updated_model.id,
        provider:updated_model.provider,
        name:updated_model.name,
        client_id:updated_model.client_id,
        client_secret:app_state.get_decrypted_api_key_preview(&Some(updated_model.client_secret)),
        issuer_url:updated_model.issuer_url,
        redirect_url:updated_model.redirect_url,
        allowed_domains:updated_model.allowed_domains,
        is_enabled:updated_model.is_enabled,
        created_at:updated_model.created_at,
        updated_at:updated_model.updated_at,
    };
  Ok((StatusCode::OK,Json(response)))
}
