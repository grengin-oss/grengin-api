use axum::{
    Json,
    extract::{Path, Query, State},
};
use chrono::Utc;
use reqwest::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter};
use uuid::Uuid;

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
        permissions::PERMISSION_SKILLS_MANAGE,
    },
    dto::skills::{
        ConversationSkillResponse, LinkSkillRequest, SkillCreateRequest, SkillListQuery,
        SkillListResponse, SkillResponse, SkillUpdateRequest,
    },
    models::{conversations, skills},
    services::{
        authorization::{AuthorizationService, PermissionScopeMode},
        skills_helpers::*,
    },
    state::SharedState,
};

#[utoipa::path(
    get,
    path = "/skills",
    tag = "skills",
    params(
        ("limit" = Option<u64>, Query, description = "Items per page (default: 20, max: 100)"),
        ("offset" = Option<u64>, Query, description = "Items to skip (default: 0)"),
        ("departmentId" = Option<Uuid>, Query, description = "Filter by department (includes global skills)"),
        ("isActive" = Option<bool>, Query, description = "Filter by active status"),
    ),
    responses(
        (status = 200, body = SkillListResponse),
        (status = 401, content_type = "application/json", body = Error),
    )
)]
pub async fn list_skills(
    claims: Claims,
    Query(query): Query<SkillListQuery>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<SkillListResponse>), AuthError> {
    let limit = query.limit.unwrap_or(20).min(100);
    let offset = query.offset.unwrap_or(0);

    let (rows, total) = list_skills_query(
        &app_state.database,
        query.department_id,
        query.is_active,
        limit,
        offset,
        Some(claims.user_id),
    )
    .await?;

    let skills = rows.into_iter().map(skill_to_response).collect();
    Ok((StatusCode::OK, Json(SkillListResponse { skills, total, limit, offset })))
}

#[utoipa::path(
    get,
    path = "/skills/{id}",
    tag = "skills",
    params(("id" = Uuid, Path, description = "Skill id")),
    responses(
        (status = 200, body = SkillResponse),
        (status = 401, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
    )
)]
pub async fn get_skill(
    _claims: Claims,
    Path(id): Path<Uuid>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<SkillResponse>), AuthError> {
    let skill = get_skill_or_404(id, &app_state.database).await?;
    let knowledge_files = get_skill_knowledge_info(&app_state.database, skill.id).await;
    Ok((StatusCode::OK, Json(skill_to_response_with_knowledge(skill, knowledge_files))))
}

#[utoipa::path(
    post,
    path = "/admin/skills",
    tag = "skills",
    request_body = SkillCreateRequest,
    responses(
        (status = 201, body = SkillResponse),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 409, content_type = "application/json", body = Error),
    )
)]
pub async fn create_skill(
    claims: Claims,
    State(app_state): State<SharedState>,
    Json(mut req): Json<SkillCreateRequest>,
) -> Result<(StatusCode, Json<SkillResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_SKILLS_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let knowledge_attachment = req.knowledge_attachment.take();

    let identifier = req.identifier.trim().to_ascii_lowercase();
    if identifier.is_empty() || identifier.len() > 100 {
        return Err(AuthError::InvalidRequest { field: "identifier" });
    }
    let name = req.name.trim().to_string();
    if name.is_empty() || name.len() > 100 {
        return Err(AuthError::InvalidRequest { field: "name" });
    }

    let conflict = skills::Entity::find()
        .filter(skills::Column::Identifier.eq(&identifier))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db skill lookup error: {e}");
            AuthError::DbTimeout
        })?;
    if conflict.is_some() {
        return Err(AuthError::DbConflict);
    }

    let tools_json = req
        .tools_config
        .map(|c| serde_json::to_value(c).unwrap_or_default());

    let now = Utc::now();
    let row = skills::ActiveModel {
        id: Set(Uuid::new_v4()),
        identifier: Set(identifier),
        name: Set(name),
        description: Set(req.description),
        avatar: Set(req.avatar),
        system_role: Set(req.system_role),
        tools_config: Set(tools_json),
        is_builtin: Set(false),
        is_active: Set(true),
        department_id: Set(req.department_id),
        user_id: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    };

    let skill = row.insert(&app_state.database).await.map_err(|e| {
        eprintln!("db create skill error: {e}");
        AuthError::DbTimeout
    })?;

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
    path = "/admin/skills/{id}",
    tag = "skills",
    params(("id" = Uuid, Path, description = "Skill id")),
    request_body = SkillUpdateRequest,
    responses(
        (status = 200, body = SkillResponse),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
    )
)]
pub async fn update_skill(
    claims: Claims,
    Path(id): Path<Uuid>,
    State(app_state): State<SharedState>,
    Json(mut req): Json<SkillUpdateRequest>,
) -> Result<(StatusCode, Json<SkillResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_SKILLS_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let knowledge_attachment = req.knowledge_attachment.take();

    let skill = get_skill_or_404(id, &app_state.database).await?;
    let mut active: skills::ActiveModel = skill.into_active_model();

    if let Some(name) = req.name {
        let name = name.trim().to_string();
        if name.is_empty() || name.len() > 100 {
            return Err(AuthError::InvalidRequest { field: "name" });
        }
        active.name = Set(name);
    }
    if let Some(desc) = req.description {
        active.description = Set(Some(desc));
    }
    if let Some(avatar) = req.avatar {
        active.avatar = Set(Some(avatar));
    }
    if let Some(system_role) = req.system_role {
        active.system_role = Set(Some(system_role));
    }
    if let Some(config) = req.tools_config {
        active.tools_config = Set(Some(serde_json::to_value(config).unwrap_or_default()));
    }
    if let Some(is_active) = req.is_active {
        active.is_active = Set(is_active);
    }
    if let Some(dept) = req.department_id {
        active.department_id = Set(Some(dept));
    }
    active.updated_at = Set(Utc::now());

    let skill = active.update(&app_state.database).await.map_err(|e| {
        eprintln!("db update skill error: {e}");
        AuthError::DbTimeout
    })?;

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
    path = "/admin/skills/{id}",
    tag = "skills",
    params(("id" = Uuid, Path, description = "Skill id")),
    responses(
        (status = 204),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
    )
)]
pub async fn delete_skill(
    claims: Claims,
    Path(id): Path<Uuid>,
    State(app_state): State<SharedState>,
) -> Result<StatusCode, AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_SKILLS_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let skill = get_skill_or_404(id, &app_state.database).await?;
    if skill.is_builtin {
        return Err(AuthError::PermissionDenied);
    }

    skills::Entity::delete_by_id(id).exec(&app_state.database).await.map_err(|e| {
        eprintln!("db delete skill error: {e}");
        AuthError::DbTimeout
    })?;

    Ok(StatusCode::NO_CONTENT)
}

// ── Conversation skill endpoints ──────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/conversations/{conversation_id}/skills",
    tag = "skills",
    params(("conversation_id" = Uuid, Path, description = "Conversation id")),
    responses(
        (status = 200, body = Vec<ConversationSkillResponse>),
        (status = 401, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
    )
)]
pub async fn list_conversation_skill_links(
    claims: Claims,
    Path(conversation_id): Path<Uuid>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<Vec<ConversationSkillResponse>>), AuthError> {
    conversations::Entity::find_by_id(conversation_id)
        .filter(conversations::Column::UserId.eq(claims.user_id))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db conversation lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    let pairs = list_conversation_skills(&app_state.database, conversation_id).await?;

    let resp = pairs
        .into_iter()
        .map(|(link, skill)| ConversationSkillResponse {
            id: link.id,
            conversation_id: link.conversation_id,
            skill: skill_to_response(skill),
            created_at: link.created_at,
        })
        .collect();

    Ok((StatusCode::OK, Json(resp)))
}

#[utoipa::path(
    post,
    path = "/conversations/{conversation_id}/skills",
    tag = "skills",
    params(("conversation_id" = Uuid, Path, description = "Conversation id")),
    request_body = LinkSkillRequest,
    responses(
        (status = 201, body = ConversationSkillResponse),
        (status = 401, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
    )
)]
pub async fn link_skill(
    claims: Claims,
    Path(conversation_id): Path<Uuid>,
    State(app_state): State<SharedState>,
    Json(req): Json<LinkSkillRequest>,
) -> Result<(StatusCode, Json<ConversationSkillResponse>), AuthError> {
    conversations::Entity::find_by_id(conversation_id)
        .filter(conversations::Column::UserId.eq(claims.user_id))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db conversation lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    let skill = get_skill_or_404(req.skill_id, &app_state.database).await?;
    let link =
        link_skill_to_conversation(&app_state.database, conversation_id, skill.id).await?;

    Ok((
        StatusCode::CREATED,
        Json(ConversationSkillResponse {
            id: link.id,
            conversation_id: link.conversation_id,
            skill: skill_to_response(skill),
            created_at: link.created_at,
        }),
    ))
}

#[utoipa::path(
    delete,
    path = "/conversations/{conversation_id}/skills/{skill_id}",
    tag = "skills",
    params(
        ("conversation_id" = Uuid, Path, description = "Conversation id"),
        ("skill_id" = Uuid, Path, description = "Skill id"),
    ),
    responses(
        (status = 204),
        (status = 401, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
    )
)]
pub async fn unlink_skill(
    claims: Claims,
    Path((conversation_id, skill_id)): Path<(Uuid, Uuid)>,
    State(app_state): State<SharedState>,
) -> Result<StatusCode, AuthError> {
    conversations::Entity::find_by_id(conversation_id)
        .filter(conversations::Column::UserId.eq(claims.user_id))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db conversation lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    unlink_skill_from_conversation(&app_state.database, conversation_id, skill_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
