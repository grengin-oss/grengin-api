use axum::{Json, extract::State};
use chrono::Utc;
use reqwest::StatusCode;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter,
};
use uuid::Uuid;

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
    },
    dto::prompts::{
        PromptFeedbackRequest, PromptSource, SystemPromptResponse, UserPromptPreferenceRequest,
    },
    models::{prompt_feedback, role_prompts, user_prompt_preferences},
    services::system_prompts,
    state::SharedState,
};

#[utoipa::path(
    get,
    path = "/me/system-prompt",
    tag = "me",
    responses(
        (status = 200, body = SystemPromptResponse),
        (status = 401, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn get_my_system_prompt(
    claims: Claims,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<SystemPromptResponse>), AuthError> {
    let resolved = system_prompts::resolve_system_prompt(&app_state.database, claims.user_id)
        .await
        .map_err(|e| {
            eprintln!("system prompt resolve error: {e}");
            AuthError::DbTimeout
        })?;

    let payload = if let Some(resolved) = resolved {
        SystemPromptResponse {
            prompt_text: Some(resolved.prompt_text),
            prompt_id: resolved.prompt_id,
            source: resolved.source,
            variables: resolved.variables,
        }
    } else {
        SystemPromptResponse {
            prompt_text: None,
            prompt_id: None,
            source: PromptSource::None,
            variables: None,
        }
    };

    Ok((StatusCode::OK, Json(payload)))
}

#[utoipa::path(
    put,
    path = "/me/system-prompt",
    tag = "me",
    request_body = UserPromptPreferenceRequest,
    responses(
        (status = 200, body = SystemPromptResponse),
        (status = 401, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
        (status = 409, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn set_my_system_prompt(
    claims: Claims,
    State(app_state): State<SharedState>,
    Json(req): Json<UserPromptPreferenceRequest>,
) -> Result<(StatusCode, Json<SystemPromptResponse>), AuthError> {
    let is_active = req.is_active.unwrap_or(true);

    if is_active
        && req.prompt_id.is_none()
        && req
            .custom_prompt_text
            .as_ref()
            .map(|v| v.trim().is_empty())
            .unwrap_or(true)
    {
        return Err(AuthError::DbConflict);
    }

    if let Some(prompt_id) = req.prompt_id {
        let exists = role_prompts::Entity::find_by_id(prompt_id)
            .one(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("prompt lookup error: {e}");
                AuthError::DbTimeout
            })?
            .is_some();
        if !exists {
            return Err(AuthError::ResourceNotFound);
        }
    }

    let existing = user_prompt_preferences::Entity::find()
        .filter(user_prompt_preferences::Column::UserId.eq(claims.user_id))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("user prompt preference lookup error: {e}");
            AuthError::DbTimeout
        })?;

    let now = Utc::now();
    let model = if let Some(existing) = existing {
        let mut active = existing.into_active_model();
        active.prompt_id = Set(req.prompt_id);
        active.custom_prompt_text = Set(req.custom_prompt_text);
        active.is_active = Set(is_active);
        active.updated_at = Set(now);
        active.update(&app_state.database).await.map_err(|e| {
            eprintln!("user prompt preference update error: {e}");
            AuthError::DbTimeout
        })?
    } else {
        user_prompt_preferences::ActiveModel {
            id: Set(Uuid::new_v4()),
            user_id: Set(claims.user_id),
            prompt_id: Set(req.prompt_id),
            custom_prompt_text: Set(req.custom_prompt_text),
            is_active: Set(is_active),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("user prompt preference insert error: {e}");
            AuthError::DbTimeout
        })?
    };

    let resolved = system_prompts::resolve_system_prompt(&app_state.database, claims.user_id)
        .await
        .map_err(|e| {
            eprintln!("system prompt resolve error: {e}");
            AuthError::DbTimeout
        })?;

    let payload = if let Some(resolved) = resolved {
        SystemPromptResponse {
            prompt_text: Some(resolved.prompt_text),
            prompt_id: resolved.prompt_id,
            source: resolved.source,
            variables: resolved.variables,
        }
    } else {
        SystemPromptResponse {
            prompt_text: model.custom_prompt_text,
            prompt_id: model.prompt_id,
            source: PromptSource::UserCustom,
            variables: None,
        }
    };

    Ok((StatusCode::OK, Json(payload)))
}

#[utoipa::path(
    delete,
    path = "/me/system-prompt",
    tag = "me",
    responses(
        (status = 204),
        (status = 401, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn reset_my_system_prompt(
    claims: Claims,
    State(app_state): State<SharedState>,
) -> Result<StatusCode, AuthError> {
    if let Some(existing) = user_prompt_preferences::Entity::find()
        .filter(user_prompt_preferences::Column::UserId.eq(claims.user_id))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("user prompt preference lookup error: {e}");
            AuthError::DbTimeout
        })?
    {
        let mut active = existing.into_active_model();
        active.is_active = Set(false);
        active.updated_at = Set(Utc::now());
        active.update(&app_state.database).await.map_err(|e| {
            eprintln!("user prompt preference update error: {e}");
            AuthError::DbTimeout
        })?;
    }

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/me/system-prompt/feedback",
    tag = "me",
    request_body = PromptFeedbackRequest,
    responses(
        (status = 201),
        (status = 401, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
        (status = 409, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn submit_prompt_feedback(
    claims: Claims,
    State(app_state): State<SharedState>,
    Json(req): Json<PromptFeedbackRequest>,
) -> Result<StatusCode, AuthError> {
    if req.rating < 1 || req.rating > 5 {
        return Err(AuthError::DbConflict);
    }

    if let Some(prompt_id) = req.prompt_id {
        let exists = role_prompts::Entity::find_by_id(prompt_id)
            .one(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("prompt lookup error: {e}");
                AuthError::DbTimeout
            })?
            .is_some();
        if !exists {
            return Err(AuthError::ResourceNotFound);
        }
    }

    let model = prompt_feedback::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(claims.user_id),
        prompt_id: Set(req.prompt_id),
        rating: Set(req.rating),
        comment: Set(req.comment),
        created_at: Set(Utc::now()),
    };

    model.insert(&app_state.database).await.map_err(|e| {
        eprintln!("prompt feedback insert error: {e}");
        AuthError::DbTimeout
    })?;

    Ok(StatusCode::CREATED)
}
