use axum::{Json, extract::{Path, State}};
use chrono::Utc;
use reqwest::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, TryIntoModel};
use crate::{auth::{claims::Claims, error::AuthError}, dto::admin_ai::{AiEngineResponse, AiEngineUpdateRequest}, models::{ai_engines, users::UserRole}, state::SharedState};

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
   let response = ai_engines
     .into_iter()
     .map(|model|{
        AiEngineResponse{
            engine_key:model.engine_key,
            display_name:model.display_name,
            is_enabled:model.is_enabled,
            api_key_configured:model.api_key.is_some(),
            api_key_status:model.api_key_status,
            api_key_preview: model.api_key.as_ref().map(|key| {
             let key = key.trim();
             if key.is_empty() {
              "<empty>".to_string()
             } else {
              let keep = 4;
              let chars: Vec<char> = key.chars().collect();
              let len = chars.len();
             if len <= keep * 2 {
              key.to_string()
              } else {
              let start: String = chars.iter().take(keep).collect();
              let end: String = chars.iter().skip(len - keep).collect();
              format!("{start}...{end}")
             }
            }
           }),
            api_key_last_validated_at:model.api_key_validated_at,
            whitelisted_models:model.whitelist_models,
            default_model:model.default_model,
        }
     }).collect();
 Ok((StatusCode::OK,Json(response)))
}

#[utoipa::path(
    get,
    path = "/admin/ai-engines/{engine_key}",
    tag = "admin",
    params(
        ("engine_key" = String, Path, description = "Engine key example 'openai','anathropic'")
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
            api_key_preview: model.api_key.as_ref().map(|key| {
             let key = key.trim();
             if key.is_empty() {
              "<empty>".to_string()
             } else {
              let keep = 4;
              let chars: Vec<char> = key.chars().collect();
              let len = chars.len();
             if len <= keep * 2 {
              key.to_string()
              } else {
              let start: String = chars.iter().take(keep).collect();
              let end: String = chars.iter().skip(len - keep).collect();
              format!("{start}...{end}")
             }
            }
           }),
            api_key_last_validated_at:model.api_key_validated_at,
            whitelisted_models:model.whitelist_models,
            default_model:model.default_model,
        };
 Ok((StatusCode::OK,Json(response)))
}

#[utoipa::path(
    put,
    path = "/admin/ai-engines/{ai_engine_key}",
    tag = "admin",
    params(
        ("engine_key" = String, Path, description = "Engine key example 'openai','anathropic'")
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
    active_model.api_key = Set(Some(req.api_key));
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
            api_key_preview: model.api_key.as_ref().map(|key| {
             let key = key.trim();
             if key.is_empty() {
              "<empty>".to_string()
             } else {
              let keep = 4;
              let chars: Vec<char> = key.chars().collect();
              let len = chars.len();
             if len <= keep * 2 {
              key.to_string()
              } else {
              let start: String = chars.iter().take(keep).collect();
              let end: String = chars.iter().skip(len - keep).collect();
              format!("{start}...{end}")
             }
            }
           }),
            api_key_last_validated_at:model.api_key_validated_at,
            whitelisted_models:model.whitelist_models,
            default_model:model.default_model,
        };
 Ok((StatusCode::OK,Json(response)))
}