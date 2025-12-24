use axum::{Json, extract::{Path, State}};
use chrono::Utc;
use reqwest::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, TryIntoModel};
use uuid::Uuid;
use crate::{auth::{claims::Claims, encryption::{encrypt_key}, error::AuthError}, dto::admin_ai::{AiEngineResponse, AiEngineUpdateRequest, AiEngineValidationResponse}, handlers::{admin_org::get_org, models::list_models}, llm::provider::{AnthropicApis, OpenaiApis}, models::{ai_engines::{self, ApiKeyStatus}, users::UserRole}, state::SharedState};

#[utoipa::path(
    get,
    path = "/admin/ai-engines",
    tag = "admin",
    responses(
        (status = 200, body = Vec<AiEngineResponse>),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
    )
)]
pub async fn get_ai_engines(
    claims:Claims,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode,Json<Vec<AiEngineResponse>>),AuthError>{
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
    }
    let ai_engines = ai_engines::Entity::find()
      .order_by_desc(ai_engines::Column::CreatedAt)
      .all(&app_state.database)
      .await
      .map_err(|e|{
         eprintln!("db error get all {e}");
         AuthError::ServiceTemporarilyUnavailable
      })?;
    if ai_engines.is_empty() {
       let (_,Json(models)) = list_models()
         .await;
       let (_,Json(org)) = get_org(claims,State(app_state.clone()))
         .await
         .map_err(|e|{
           eprintln!("db error get one {:?}",e);
           AuthError::ServiceTemporarilyUnavailable
        })?;
       let ai_engines_active_models:Vec<ai_engines::ActiveModel> = models
        .providers
        .iter()
        .map(|provider|{
           ai_engines::ActiveModel {
             id:Set(Uuid::new_v4()),
             org_id:Set(org.id),
             display_name:Set(provider.name.clone()),
             is_enabled:Set(false),
             engine_key:Set(provider.key.clone()),
             api_key_status:Set(ApiKeyStatus::NotValidated),
             api_key:Set(None),
             whitelist_models:Set(provider
                .models
                .iter()
                .map(|model| model.name.clone())
                .collect::<Vec<String>>()),
             default_model:Set(String::from("<empty>")),
             api_key_validated_at:Set(None),
             created_at:Set(Utc::now()),
             updated_at:Set(Utc::now()), 
            }
          })
          .collect();
       ai_engines::Entity::insert_many(ai_engines_active_models)
         .exec(&app_state.database)
         .await
         .map_err(|e|{
            eprintln!("db insert many error {:?}",e);
            AuthError::ServiceTemporarilyUnavailable
         })?;
       let response = models
         .providers
         .into_iter()
         .map(|provider|{
            AiEngineResponse { 
              engine_key:provider.key,
              display_name:provider.name,
              is_enabled:false,
              api_key_configured:false,
              api_key_status:ApiKeyStatus::NotValidated,
              api_key_preview:Some("<empty>".to_string()),
              api_key_last_validated_at:None,
              whitelisted_models:provider
                .models
                .into_iter()
                .map(|model| model.name)
                .collect(),
              default_model:None,
              created_at:Utc::now(),
              updated_at:Utc::now(),
            }
          })
          .collect();
       return Ok((StatusCode::OK,Json(response)))
    }
    let response = ai_engines
      .into_iter()
      .map(|model|{
        AiEngineResponse{
            engine_key:model.engine_key,
            display_name:model.display_name,
            is_enabled:model.is_enabled,
            api_key_configured:model.api_key.is_some(),
            api_key_status:model.api_key_status,
            api_key_preview:app_state.get_decrypted_api_key_preview(&model.api_key),
            api_key_last_validated_at:model.api_key_validated_at,
            whitelisted_models:model.whitelist_models,
            default_model:Some(model.default_model),
            created_at:model.created_at,
            updated_at:model.updated_at
        }
     }).collect();
 Ok((StatusCode::OK,Json(response)))
}

#[utoipa::path(
    get,
    path = "/admin/ai-engines/{engine_key}",
    tag = "admin",
    params(
        ("ai_engine_key" = String, Path, description = "Engine key example 'openai','anthropic'")
    ),
    responses(
        (status = 200, body = AiEngineResponse),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
    )
)]
pub async fn get_ai_engines_by_key(
    claims:Claims,
    Path(ai_engine_key):Path<String>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode,Json<AiEngineResponse>),AuthError>{
   match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
   }
   let model = ai_engines::Entity::find()
      .filter(ai_engines::Column::EngineKey.eq(ai_engine_key))
      .order_by_desc(ai_engines::Column::CreatedAt)
      .one(&app_state.database)
      .await
      .map_err(|e|{
        eprintln!("db error get all {e}");
        AuthError::ServiceTemporarilyUnavailable
      })?
      .ok_or(AuthError::ResourceNotFound)?;
    let response = AiEngineResponse{
            engine_key:model.engine_key,
            display_name:model.display_name,
            is_enabled:model.is_enabled,
            api_key_configured:model.api_key.is_some(),
            api_key_status:model.api_key_status,
            api_key_preview:app_state.get_decrypted_api_key_preview(&model.api_key),
            api_key_last_validated_at:model.api_key_validated_at,
            whitelisted_models:model.whitelist_models,
            default_model:Some(model.default_model),
            created_at:model.created_at,
            updated_at:model.updated_at
        };
 Ok((StatusCode::OK,Json(response)))
}

#[utoipa::path(
    put,
    path = "/admin/ai-engines/{ai_engine_key}",
    tag = "admin",
    params(
        ("ai_engine_key" = String, Path, description = "Engine key example 'openai','anthropic'")
    ),
    responses(
        (status = 200, description = "Updated successfully"),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
    )
)]
pub async fn update_ai_engines_by_key(
    claims:Claims,
    Path(ai_engine_key):Path<String>,
    State(app_state): State<SharedState>,
    Json(req):Json<AiEngineUpdateRequest>
) -> Result<(StatusCode,Json<AiEngineResponse>),AuthError>{
   match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
   }
   let ai_engine = ai_engines::Entity::find()
      .filter(ai_engines::Column::EngineKey.eq(ai_engine_key))
      .order_by_desc(ai_engines::Column::CreatedAt)
      .one(&app_state.database)
      .await
      .map_err(|e|{
        eprintln!("db error get all {e}");
        AuthError::ServiceTemporarilyUnavailable
      })?
      .ok_or(AuthError::ResourceNotFound)?;
    let mut active_model = ai_engine
      .clone()
      .into_active_model();
    let encrypted_api_key = encrypt_key(&app_state.settings.auth.app_key,req.api_key.as_bytes())
      .map_err(|e|{
        eprintln!("Encryption error for api key: {:?}",e);
        AuthError::ServiceTemporarilyUnavailable
       })?;
    active_model.api_key = Set(Some(encrypted_api_key));
    active_model.updated_at = Set(Utc::now());
    active_model.default_model = Set(req.default_model);
    active_model.whitelist_models = Set(req.whitelisted_models);
    active_model
     .clone()
     .update(&app_state.database)
     .await
     .map_err(|e|{
        eprintln!("db error update one {e}");
        AuthError::ServiceTemporarilyUnavailable
      })?;
    let model = active_model
      .try_into_model()
      .map_err(|e|{
        eprintln!("db error model parse error {e}");
        AuthError::ServiceTemporarilyUnavailable
      })?;
      let response = AiEngineResponse{
            engine_key:model.engine_key,
            display_name:model.display_name,
            is_enabled:model.is_enabled,
            api_key_configured:model.api_key.is_some(),
            api_key_status:model.api_key_status,
            api_key_preview:app_state.get_decrypted_api_key_preview(&model.api_key),
            api_key_last_validated_at:model.api_key_validated_at,
            whitelisted_models:model.whitelist_models,
            default_model:Some(model.default_model),
            created_at:model.created_at,
            updated_at:model.updated_at
        };
 Ok((StatusCode::OK,Json(response)))
}

#[utoipa::path(
    delete,
    path = "/admin/ai-engines/{ai_engine_key}/api-key",
    tag = "admin",
    params(
        ("ai_engine_key" = String, Path, description = "Engine key example 'openai','anthropic'")
    ),
    responses(
        (status = 200, description = "Updated successfully"),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
    )
)]
pub async fn delete_ai_engines_api_key_key(
    claims:Claims,
    Path(ai_engine_key):Path<String>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode,Json<AiEngineResponse>),AuthError>{
   match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
   }
   let ai_engine = ai_engines::Entity::find()
      .filter(ai_engines::Column::EngineKey.eq(ai_engine_key))
      .order_by_desc(ai_engines::Column::CreatedAt)
      .one(&app_state.database)
      .await
      .map_err(|e|{
        eprintln!("db error get all {e}");
        AuthError::ServiceTemporarilyUnavailable
      })?
      .ok_or(AuthError::ResourceNotFound)?;
    let mut active_model = ai_engine
      .clone()
      .into_active_model();
    active_model.api_key = Set(None);
    active_model.updated_at = Set(Utc::now());
    active_model.api_key_status = Set(ApiKeyStatus::NotConfigured);
    active_model
     .clone()
     .update(&app_state.database)
     .await
     .map_err(|e|{
        eprintln!("db error update one {e}");
        AuthError::ServiceTemporarilyUnavailable
      })?;
    let model = active_model
      .try_into_model()
      .map_err(|e|{
        eprintln!("db error model parse error {e}");
        AuthError::ServiceTemporarilyUnavailable
      })?;
      let response = AiEngineResponse{
            engine_key:model.engine_key,
            display_name:model.display_name,
            is_enabled:model.is_enabled,
            api_key_configured:model.api_key.is_some(),
            api_key_status:model.api_key_status,
            api_key_preview:app_state.get_decrypted_api_key_preview(&model.api_key),
            api_key_last_validated_at:model.api_key_validated_at,
            whitelisted_models:model.whitelist_models,
            default_model:Some(model.default_model),
            created_at:model.created_at,
            updated_at:model.updated_at
        };
 Ok((StatusCode::OK,Json(response)))
}

#[utoipa::path(
    post,
    path = "/admin/ai-engines/{ai_engine_key}/validate",
    tag = "admin",
    params(
        ("ai_engine_key" = String, Path, description = "Engine key example 'openai','anthropic'")
    ),
    responses(
        (status = 200, body = AiEngineValidationResponse),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
    )
)]
pub async fn validate_ai_engines_by_key(
    claims:Claims,
    Path(ai_engine_key):Path<String>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode,Json<AiEngineValidationResponse>),AuthError>{
   match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
   }
   let api_key_status =  match ai_engine_key.as_ref() {
       "openai" => {
         let openai_settings = &app_state
           .settings
           .openai
           .as_ref()
           .ok_or(AuthError::ResourceNotFound)?;
         let models = app_state
           .req_client
           .openai_list_models(openai_settings)
           .await;
          if models.is_ok(){
            ApiKeyStatus::Valid
          }else{
            ApiKeyStatus::InValid
          }
       }
       "anthropic" => {
        let anthropic_settings = &app_state
          .settings
          .anthropic
          .as_ref()
          .ok_or(AuthError::ResourceNotFound)?;
        let models = app_state
           .req_client
           .anthropic_get_models(anthropic_settings)
           .await;
          if models.is_ok(){
            ApiKeyStatus::Valid
          }else{
            ApiKeyStatus::InValid
          }
        }
       _ => ApiKeyStatus::NotConfigured,
   };
   let ai_engine = ai_engines::Entity::find()
      .filter(ai_engines::Column::EngineKey.eq(ai_engine_key.clone()))
      .order_by_desc(ai_engines::Column::CreatedAt)
      .one(&app_state.database)
      .await
      .map_err(|e|{
        eprintln!("db error get all {e}");
        AuthError::ServiceTemporarilyUnavailable
      })?
      .ok_or(AuthError::ResourceNotFound)?;
    let mut active_model = ai_engine
      .clone()
      .into_active_model();
    active_model.api_key_status = Set(api_key_status.clone());
    active_model.updated_at = Set(Utc::now());
    active_model.api_key_validated_at = Set(Some(Utc::now()));
    active_model
      .clone()
      .update(&app_state.database)
      .await
      .map_err(|e|{
         eprintln!("db error update one {e}");
         AuthError::ServiceTemporarilyUnavailable
       })?;
   let (valid,message) = if api_key_status == ApiKeyStatus::Valid {
     (true,"API key validated successfully".to_string())
   }else{
     (false,format!("API key is incorrect for {ai_engine_key}."))
   };
   let response = AiEngineValidationResponse{ 
      valid,
      message,
      models_available:ai_engine.whitelist_models.len() as i64,
    };
 Ok((StatusCode::OK,Json(response)))
}