use axum::{
    Json,
    extract::{Path, Query, State},
};
use reqwest::StatusCode;
use uuid::Uuid;

use crate::{
    auth::{claims::Claims, error::AuthError},
    dto::skills::{SkillListResponse, SkillResponse, UserSkillCreateRequest, UserSkillListQuery, UserSkillUpdateRequest},
    services::{
        me_skills_helpers::*,
        skills_helpers::{get_skill_knowledge_info, process_skill_knowledge, skill_to_response_with_knowledge},
    },
    state::SharedState,
};

#[utoipa::path(
    get,
    path = "/me/skills",
    tag = "me",
    params(
        ("limit" = Option<u64>, Query, description = "Items per page (default: 20, max: 100)"),
        ("offset" = Option<u64>, Query, description = "Items to skip (default: 0)"),
        ("isActive" = Option<bool>, Query, description = "Filter by active status"),
    ),
    responses(
        (status = 200, body = SkillListResponse),
        (status = 401, content_type = "application/json"),
    )
)]
pub async fn list_my_skills(
    claims: Claims,
    Query(query): Query<UserSkillListQuery>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<SkillListResponse>), AuthError> {
    let limit = query.limit.unwrap_or(20).min(100);
    let offset = query.offset.unwrap_or(0);

    let (rows, total) = list_user_skills(
        &app_state.database,
        claims.user_id,
        query.is_active,
        limit,
        offset,
    )
    .await?;

    let skills = rows.into_iter().map(user_skill_to_response).collect();
    Ok((StatusCode::OK, Json(SkillListResponse { skills, total, limit, offset })))
}

#[utoipa::path(
    get,
    path = "/me/skills/{id}",
    tag = "me",
    params(("id" = Uuid, Path, description = "Skill id")),
    responses(
        (status = 200, body = SkillResponse),
        (status = 401, content_type = "application/json"),
        (status = 404, content_type = "application/json"),
    )
)]
pub async fn get_my_skill(
    claims: Claims,
    Path(id): Path<Uuid>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<SkillResponse>), AuthError> {
    let skill = get_user_skill_or_404(id, claims.user_id, &app_state.database).await?;
    let knowledge_files = get_skill_knowledge_info(&app_state.database, skill.id).await;
    Ok((StatusCode::OK, Json(skill_to_response_with_knowledge(skill, knowledge_files))))
}

#[utoipa::path(
    post,
    path = "/me/skills",
    tag = "me",
    request_body = UserSkillCreateRequest,
    responses(
        (status = 201, body = SkillResponse),
        (status = 401, content_type = "application/json"),
        (status = 422, content_type = "application/json"),
    )
)]
pub async fn create_my_skill(
    claims: Claims,
    State(app_state): State<SharedState>,
    Json(mut req): Json<UserSkillCreateRequest>,
) -> Result<(StatusCode, Json<SkillResponse>), AuthError> {
    let knowledge_attachment = req.knowledge_attachment.take();
    let skill = create_user_skill(&app_state.database, claims.user_id, req).await?;
    let knowledge_files = if let Some(attachment) = knowledge_attachment {
        process_skill_knowledge(&app_state.database, skill.id, claims.user_id, attachment)
            .await
            .unwrap_or_default()
    } else {
        vec![]
    };
    Ok((StatusCode::CREATED, Json(skill_to_response_with_knowledge(skill, knowledge_files))))
}

#[utoipa::path(
    put,
    path = "/me/skills/{id}",
    tag = "me",
    params(("id" = Uuid, Path, description = "Skill id")),
    request_body = UserSkillUpdateRequest,
    responses(
        (status = 200, body = SkillResponse),
        (status = 401, content_type = "application/json"),
        (status = 404, content_type = "application/json"),
    )
)]
pub async fn update_my_skill(
    claims: Claims,
    Path(id): Path<Uuid>,
    State(app_state): State<SharedState>,
    Json(mut req): Json<UserSkillUpdateRequest>,
) -> Result<(StatusCode, Json<SkillResponse>), AuthError> {
    let knowledge_attachment = req.knowledge_attachment.take();
    let skill = update_user_skill(&app_state.database, id, claims.user_id, req).await?;
    let knowledge_files = if let Some(attachment) = knowledge_attachment {
        process_skill_knowledge(&app_state.database, skill.id, claims.user_id, attachment)
            .await
            .unwrap_or_default()
    } else {
        get_skill_knowledge_info(&app_state.database, skill.id).await
    };
    Ok((StatusCode::OK, Json(skill_to_response_with_knowledge(skill, knowledge_files))))
}

#[utoipa::path(
    delete,
    path = "/me/skills/{id}",
    tag = "me",
    params(("id" = Uuid, Path, description = "Skill id")),
    responses(
        (status = 204),
        (status = 401, content_type = "application/json"),
        (status = 404, content_type = "application/json"),
    )
)]
pub async fn delete_my_skill(
    claims: Claims,
    Path(id): Path<Uuid>,
    State(app_state): State<SharedState>,
) -> Result<StatusCode, AuthError> {
    delete_user_skill(&app_state.database, id, claims.user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
