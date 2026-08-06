// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, Query, State},
};
use chrono::Utc;
use migration::extension::postgres::PgExpr;
use reqwest::StatusCode;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, Condition, EntityTrait, IntoActiveModel,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
};
use uuid::Uuid;

use crate::{
    auth::{claims::Claims, error::{AuthError, Error}},
    dto::projects::{
        AddMcpServerRequest, AddMemberRequest, AddSourceRequest, ArtifactCreateRequest,
        ArtifactUpdateRequest, InstructionsUpdateRequest, LinkProjectRequest, MemberSearchQuery,
        ProjectCreateRequest, ProjectDetailResponse, ProjectListQuery, ProjectListResponse,
        ProjectMcpServerResponse, ProjectMemberResponse, ProjectResponse, ProjectSourceResponse,
        ProjectUpdateRequest, ShareProjectResponse, UserSearchItem, UserSearchResponse,
    },
    models::{
        conversation_projects, conversations, mcp_servers, project_mcp_servers, project_members,
        project_sources, projects, projects::ProjectVisibility, users,
    },
    models::project_sources::ProcessingStatus,
    services::{
        project_helpers::*,
        project_source_processing::{delete_source_chunks, spawn_process_source, write_artifact_file},
    },
    state::SharedState,
};

#[utoipa::path(
    get,
    path = "/projects",
    tag = "projects",
    params(
        ("limit" = Option<u64>, Query, description = "Items per page (default: 20, max: 100)"),
        ("offset" = Option<u64>, Query, description = "Items to skip (default: 0)"),
        ("search" = Option<String>, Query, description = "Filter by name or description"),
        ("category" = Option<String>, Query, description = "Filter by category"),
        ("visibility" = Option<String>, Query, description = "Filter by visibility"),
    ),
    responses(
        (status = 200, body = ProjectListResponse),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
    )
)]
pub async fn list_projects(
    claims: Claims,
    Query(query): Query<ProjectListQuery>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<ProjectListResponse>), AuthError> {
    let limit = query.limit.unwrap_or(20).min(100);
    let offset = query.offset.unwrap_or(0);

    let member_ids = fetch_member_project_ids(claims.user_id, &app_state.database).await?;
    let visibility_cond = build_visibility_condition(claims.user_id, member_ids);

    let mut select = projects::Entity::find().filter(visibility_cond);

    if let Some(search) = query.search.as_deref().filter(|v| !v.is_empty()) {
        select = select.filter(
            Condition::any()
                .add(projects::Column::Name.into_expr().ilike(format!("%{search}%")))
                .add(projects::Column::Description.into_expr().ilike(format!("%{search}%"))),
        );
    }
    if let Some(category) = query.category.as_deref().filter(|v| !v.is_empty()) {
        select = select.filter(projects::Column::Category.eq(category));
    }
    if let Some(vis) = query.visibility {
        select = select.filter(projects::Column::Visibility.eq(vis));
    }

    select = select.order_by_desc(projects::Column::UpdatedAt);

    let total = select.clone().count(&app_state.database).await.map_err(|e| {
        eprintln!("db project count error: {e}");
        AuthError::DbTimeout
    })?;
    let rows = select.offset(offset).limit(limit).all(&app_state.database).await.map_err(|e| {
        eprintln!("db project list error: {e}");
        AuthError::DbTimeout
    })?;

    let project_ids: Vec<Uuid> = rows.iter().map(|p| p.id).collect();
    let (chat_counts, source_counts, member_counts) =
        fetch_counts(&project_ids, &app_state.database).await?;

    let projects = rows
        .into_iter()
        .map(|p| to_project_response(p, &chat_counts, &source_counts, &member_counts))
        .collect();

    Ok((StatusCode::OK, Json(ProjectListResponse { projects, total, limit, offset })))
}

#[utoipa::path(
    post,
    path = "/projects",
    tag = "projects",
    request_body = ProjectCreateRequest,
    responses(
        (status = 201, body = ProjectResponse),
        (status = 400, content_type = "application/json", body = Error, description = "Invalid field value"),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
    )
)]
pub async fn create_project(
    claims: Claims,
    State(app_state): State<SharedState>,
    Json(req): Json<ProjectCreateRequest>,
) -> Result<(StatusCode, Json<ProjectResponse>), AuthError> {
    let name = req.name.trim().to_string();
    if name.is_empty() || name.len() > 100 {
        return Err(AuthError::InvalidRequest { field: "name" });
    }

    let category = req.category.as_deref().unwrap_or("research").trim().to_ascii_lowercase();
    if !is_valid_category(&category) {
        return Err(AuthError::InvalidRequest { field: "category" });
    }

    let visibility = req.visibility.unwrap_or(ProjectVisibility::Private);

    let now = Utc::now();
    let inserted = projects::ActiveModel {
        id: Set(Uuid::new_v4()),
        name: Set(name),
        description: Set({
            let d = req.description.map(|d| d.trim().to_string()).filter(|d| !d.is_empty());
            if d.as_deref().map(|s| s.len() > 500).unwrap_or(false) {
                return Err(AuthError::InvalidRequest { field: "description" });
            }
            d
        }),
        category: Set(category),
        visibility: Set(visibility),
        owner_id: Set(claims.user_id),
        instructions: Set(None),
        last_activity_at: Set(Some(now)),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&app_state.database)
    .await
    .map_err(|e| {
        eprintln!("db project insert error: {e}");
        AuthError::DbTimeout
    })?;

    let empty: HashMap<Uuid, i64> = HashMap::new();
    Ok((StatusCode::CREATED, Json(to_project_response(inserted, &empty, &empty, &empty))))
}

#[utoipa::path(
    get,
    path = "/projects/{id}",
    tag = "projects",
    params(("id" = Uuid, Path, description = "Project id")),
    responses(
        (status = 200, body = ProjectResponse),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 403, content_type = "application/json", body = Error, description = "Access denied"),
        (status = 404, content_type = "application/json", body = Error, description = "Project not found"),
    )
)]
pub async fn get_project(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<ProjectResponse>), AuthError> {
    let project = get_project_or_404(id, &app_state.database).await?;
    ensure_project_read_access(claims.user_id, &project, &app_state.database).await?;
    let (chat_counts, source_counts, member_counts) =
        fetch_counts(&[id], &app_state.database).await?;
    Ok((StatusCode::OK, Json(to_project_response(project, &chat_counts, &source_counts, &member_counts))))
}

#[utoipa::path(
    get,
    path = "/projects/{id}/detail",
    tag = "projects",
    params(("id" = Uuid, Path, description = "Project id")),
    responses(
        (status = 200, body = ProjectDetailResponse),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 403, content_type = "application/json", body = Error, description = "Access denied"),
        (status = 404, content_type = "application/json", body = Error, description = "Project not found"),
    )
)]
pub async fn get_project_detail(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<ProjectDetailResponse>), AuthError> {
    let project = get_project_or_404(id, &app_state.database).await?;
    ensure_project_read_access(claims.user_id, &project, &app_state.database).await?;

    let (chat_counts, source_counts, member_counts) =
        fetch_counts(&[id], &app_state.database).await?;
    let sources = fetch_project_sources(id, &app_state.database).await?;
    let chats = fetch_project_chats(id, &app_state.database).await?;
    let mcp_servers = fetch_project_mcp_servers(id, &app_state.database).await?;

    Ok((StatusCode::OK, Json(ProjectDetailResponse {
        id: project.id,
        name: project.name,
        description: project.description.unwrap_or_default(),
        category: project.category,
        visibility: project.visibility,
        owner_id: project.owner_id,
        instructions: project.instructions.unwrap_or_default(),
        chat_count: *chat_counts.get(&id).unwrap_or(&0),
        source_count: *source_counts.get(&id).unwrap_or(&0),
        member_count: *member_counts.get(&id).unwrap_or(&0),
        last_activity_at: project.last_activity_at,
        created_at: project.created_at,
        updated_at: project.updated_at,
        sources,
        chats,
        mcp_servers,
    })))
}

#[utoipa::path(
    patch,
    path = "/projects/{id}",
    tag = "projects",
    params(("id" = Uuid, Path, description = "Project id")),
    request_body = ProjectUpdateRequest,
    responses(
        (status = 200, body = ProjectResponse),
        (status = 400, content_type = "application/json", body = Error, description = "Invalid field value"),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 403, content_type = "application/json", body = Error, description = "Only project owner can update"),
        (status = 404, content_type = "application/json", body = Error, description = "Project not found"),
    )
)]
pub async fn update_project(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(req): Json<ProjectUpdateRequest>,
) -> Result<(StatusCode, Json<ProjectResponse>), AuthError> {
    let project = get_project_or_404(id, &app_state.database).await?;
    ensure_project_owner(claims.user_id, &project)?;

    let mut active = project.into_active_model();

    if let Some(name) = req.name {
        let name = name.trim().to_string();
        if name.is_empty() || name.len() > 100 {
            return Err(AuthError::InvalidRequest { field: "name" });
        }
        active.name = Set(name);
    }
    if let Some(desc) = req.description {
        let desc = desc.trim().to_string();
        if desc.len() > 500 {
            return Err(AuthError::InvalidRequest { field: "description" });
        }
        active.description = Set(if desc.is_empty() { None } else { Some(desc) });
    }
    if let Some(cat) = req.category {
        let cat = cat.trim().to_ascii_lowercase();
        if !is_valid_category(&cat) {
            return Err(AuthError::InvalidRequest { field: "category" });
        }
        active.category = Set(cat);
    }
    if let Some(vis) = req.visibility {
        active.visibility = Set(vis);
    }

    active.updated_at = Set(Utc::now());
    let updated = active.update(&app_state.database).await.map_err(|e| {
        eprintln!("db project update error: {e}");
        AuthError::DbTimeout
    })?;

    let (chat_counts, source_counts, member_counts) =
        fetch_counts(&[id], &app_state.database).await?;
    Ok((StatusCode::OK, Json(to_project_response(updated, &chat_counts, &source_counts, &member_counts))))
}

#[utoipa::path(
    delete,
    path = "/projects/{id}",
    tag = "projects",
    params(("id" = Uuid, Path, description = "Project id")),
    responses(
        (status = 204, description = "Project deleted"),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 403, content_type = "application/json", body = Error, description = "Only project owner can delete"),
        (status = 404, content_type = "application/json", body = Error, description = "Project not found"),
    )
)]
pub async fn delete_project(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AuthError> {
    let project = get_project_or_404(id, &app_state.database).await?;
    ensure_project_owner(claims.user_id, &project)?;

    project_sources::Entity::delete_many()
        .filter(project_sources::Column::ProjectId.eq(id))
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db sources delete error: {e}");
            AuthError::DbTimeout
        })?;

    projects::Entity::delete_by_id(id)
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db project delete error: {e}");
            AuthError::DbTimeout
        })?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/projects/{id}/members",
    tag = "projects",
    params(("id" = Uuid, Path, description = "Project id")),
    request_body = AddMemberRequest,
    responses(
        (status = 201, description = "Member added"),
        (status = 400, content_type = "application/json", body = Error, description = "Invalid role"),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 403, content_type = "application/json", body = Error, description = "Only project owner can manage members"),
        (status = 404, content_type = "application/json", body = Error, description = "Project not found"),
        (status = 409, content_type = "application/json", body = Error, description = "User is already a member"),
    )
)]
pub async fn add_project_member(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(req): Json<AddMemberRequest>,
) -> Result<StatusCode, AuthError> {
    let project = get_project_or_404(id, &app_state.database).await?;
    ensure_project_owner(claims.user_id, &project)?;

    if req.user_id == project.owner_id {
        return Err(AuthError::DbConflict);
    }

    let role = req.role.as_deref().unwrap_or("member").trim().to_ascii_lowercase();
    if !matches!(role.as_str(), "member" | "owner") {
        return Err(AuthError::InvalidRequest { field: "role" });
    }

    let existing = project_members::Entity::find()
        .filter(project_members::Column::ProjectId.eq(id))
        .filter(project_members::Column::UserId.eq(req.user_id))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db member check error: {e}");
            AuthError::DbTimeout
        })?;

    if existing.is_some() {
        return Err(AuthError::DbConflict);
    }

    project_members::ActiveModel {
        id: Set(Uuid::new_v4()),
        project_id: Set(id),
        user_id: Set(req.user_id),
        role: Set(role),
        created_at: Set(Utc::now()),
    }
    .insert(&app_state.database)
    .await
    .map_err(|e| {
        eprintln!("db member insert error: {e}");
        AuthError::DbTimeout
    })?;

    Ok(StatusCode::CREATED)
}

#[utoipa::path(
    delete,
    path = "/projects/{id}/members/{user_id}",
    tag = "projects",
    params(
        ("id" = Uuid, Path, description = "Project id"),
        ("user_id" = Uuid, Path, description = "User id to remove"),
    ),
    responses(
        (status = 204, description = "Member removed"),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 403, content_type = "application/json", body = Error, description = "Only project owner can manage members"),
        (status = 404, content_type = "application/json", body = Error, description = "Project or member not found"),
    )
)]
pub async fn remove_project_member(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path((id, member_user_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AuthError> {
    let project = get_project_or_404(id, &app_state.database).await?;
    ensure_project_owner(claims.user_id, &project)?;

    let result = project_members::Entity::delete_many()
        .filter(project_members::Column::ProjectId.eq(id))
        .filter(project_members::Column::UserId.eq(member_user_id))
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db member delete error: {e}");
            AuthError::DbTimeout
        })?;

    if result.rows_affected == 0 {
        return Err(AuthError::ResourceNotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    put,
    path = "/projects/{id}/instructions",
    tag = "projects",
    params(("id" = Uuid, Path, description = "Project id")),
    request_body = InstructionsUpdateRequest,
    responses(
        (status = 204, description = "Instructions updated"),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 403, content_type = "application/json", body = Error, description = "Only project owner can update instructions"),
        (status = 404, content_type = "application/json", body = Error, description = "Project not found"),
    )
)]
pub async fn update_project_instructions(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(req): Json<InstructionsUpdateRequest>,
) -> Result<StatusCode, AuthError> {
    let project = get_project_or_404(id, &app_state.database).await?;
    ensure_project_owner(claims.user_id, &project)?;

    let instructions = req.instructions.trim().to_string();
    let mut active = project.into_active_model();
    active.instructions = Set(if instructions.is_empty() { None } else { Some(instructions) });
    active.updated_at = Set(Utc::now());
    active.update(&app_state.database).await.map_err(|e| {
        eprintln!("db instructions update error: {e}");
        AuthError::DbTimeout
    })?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/projects/{id}/sources",
    tag = "projects",
    params(("id" = Uuid, Path, description = "Project id")),
    request_body = AddSourceRequest,
    responses(
        (status = 201, body = ProjectSourceResponse),
        (status = 400, content_type = "application/json", body = Error, description = "Invalid origin"),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 403, content_type = "application/json", body = Error, description = "Only project owner can add sources"),
        (status = 404, content_type = "application/json", body = Error, description = "Project not found"),
    )
)]
pub async fn add_project_source(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(req): Json<AddSourceRequest>,
) -> Result<(StatusCode, Json<ProjectSourceResponse>), AuthError> {
    let project = get_project_or_404(id, &app_state.database).await?;
    ensure_project_read_access(claims.user_id, &project, &app_state.database).await?;

    let origin = req.origin.as_deref().unwrap_or("uploaded").trim().to_ascii_lowercase();
    if !matches!(origin.as_str(), "uploaded" | "artifact") {
        return Err(AuthError::InvalidRequest { field: "origin" });
    }

    let processing_status = if req.file_id.is_some() {
        ProcessingStatus::Pending
    } else {
        ProcessingStatus::NoFile
    };

    let inserted = project_sources::ActiveModel {
        id: Set(Uuid::new_v4()),
        project_id: Set(id),
        file_name: Set(req.file_name.trim().to_string()),
        file_type: Set(req.file_type.trim().to_string()),
        file_size: Set(req.file_size),
        origin: Set(origin),
        uploaded_at: Set(Utc::now()),
        file_id: Set(req.file_id),
        processing_status: Set(processing_status.to_string()),
        processing_error: Set(None),
    }
    .insert(&app_state.database)
    .await
    .map_err(|e| {
        eprintln!("db source insert error: {e}");
        AuthError::DbTimeout
    })?;

    if let Some(fid) = inserted.file_id {
        spawn_process_source(app_state, inserted.id, inserted.project_id, fid);
    }

    Ok((StatusCode::CREATED, Json(source_to_response(inserted))))
}

#[utoipa::path(
    delete,
    path = "/projects/{id}/sources/{source_id}",
    tag = "projects",
    params(
        ("id" = Uuid, Path, description = "Project id"),
        ("source_id" = Uuid, Path, description = "Source id"),
    ),
    responses(
        (status = 204, description = "Source deleted"),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 403, content_type = "application/json", body = Error, description = "Only project owner can delete sources"),
        (status = 404, content_type = "application/json", body = Error, description = "Project or source not found"),
    )
)]
pub async fn delete_project_source(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path((id, source_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AuthError> {
    let project = get_project_or_404(id, &app_state.database).await?;
    ensure_project_owner(claims.user_id, &project)?;

    let result = project_sources::Entity::delete_many()
        .filter(project_sources::Column::ProjectId.eq(id))
        .filter(project_sources::Column::Id.eq(source_id))
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db source delete error: {e}");
            AuthError::DbTimeout
        })?;

    if result.rows_affected == 0 {
        return Err(AuthError::ResourceNotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/projects/{id}/share",
    tag = "projects",
    params(("id" = Uuid, Path, description = "Project id")),
    responses(
        (status = 200, body = ShareProjectResponse),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 403, content_type = "application/json", body = Error, description = "Only project owner can share"),
        (status = 404, content_type = "application/json", body = Error, description = "Project not found"),
    )
)]
pub async fn share_project(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<ShareProjectResponse>), AuthError> {
    let project = get_project_or_404(id, &app_state.database).await?;
    ensure_project_owner(claims.user_id, &project)?;

    let base_url = std::env::var("REDIRECT_URL")
        .unwrap_or_else(|_| "http://localhost:3000".to_string());
    let share_url = format!("{}/projects/{}", base_url.trim_end_matches('/'), id);

    Ok((StatusCode::OK, Json(ShareProjectResponse { share_url })))
}

#[utoipa::path(
    post,
    path = "/conversations/{conversation_id}/projects",
    tag = "projects",
    params(("conversation_id" = Uuid, Path, description = "Conversation id")),
    request_body = LinkProjectRequest,
    responses(
        (status = 201, description = "Project linked to conversation"),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 403, content_type = "application/json", body = Error, description = "Access denied"),
        (status = 404, content_type = "application/json", body = Error, description = "Conversation or project not found"),
        (status = 409, content_type = "application/json", body = Error, description = "Already linked"),
    )
)]
pub async fn link_project_to_conversation(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(conversation_id): Path<Uuid>,
    Json(req): Json<LinkProjectRequest>,
) -> Result<StatusCode, AuthError> {
    conversations::Entity::find_by_id(conversation_id)
        .filter(conversations::Column::UserId.eq(claims.user_id))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db conversation find error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    let project = get_project_or_404(req.project_id, &app_state.database).await?;
    ensure_project_read_access(claims.user_id, &project, &app_state.database).await?;

    let existing = conversation_projects::Entity::find()
        .filter(conversation_projects::Column::ConversationId.eq(conversation_id))
        .filter(conversation_projects::Column::ProjectId.eq(req.project_id))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db link check error: {e}");
            AuthError::DbTimeout
        })?;

    if existing.is_some() {
        return Err(AuthError::DbConflict);
    }

    conversation_projects::ActiveModel {
        id: Set(Uuid::new_v4()),
        conversation_id: Set(conversation_id),
        project_id: Set(req.project_id),
        created_at: Set(Utc::now()),
    }
    .insert(&app_state.database)
    .await
    .map_err(|e| {
        eprintln!("db link insert error: {e}");
        AuthError::DbTimeout
    })?;

    Ok(StatusCode::CREATED)
}

#[utoipa::path(
    delete,
    path = "/conversations/{conversation_id}/projects/{project_id}",
    tag = "projects",
    params(
        ("conversation_id" = Uuid, Path, description = "Conversation id"),
        ("project_id" = Uuid, Path, description = "Project id"),
    ),
    responses(
        (status = 204, description = "Project unlinked from conversation"),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 404, content_type = "application/json", body = Error, description = "Conversation or link not found"),
    )
)]
pub async fn unlink_project_from_conversation(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path((conversation_id, project_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AuthError> {
    conversations::Entity::find_by_id(conversation_id)
        .filter(conversations::Column::UserId.eq(claims.user_id))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db conversation find error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    let result = conversation_projects::Entity::delete_many()
        .filter(conversation_projects::Column::ConversationId.eq(conversation_id))
        .filter(conversation_projects::Column::ProjectId.eq(project_id))
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db link delete error: {e}");
            AuthError::DbTimeout
        })?;

    if result.rows_affected == 0 {
        return Err(AuthError::ResourceNotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/projects/{id}/members",
    tag = "projects",
    params(("id" = Uuid, Path, description = "Project id")),
    responses(
        (status = 200, body = Vec<ProjectMemberResponse>),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 403, content_type = "application/json", body = Error, description = "Access denied"),
        (status = 404, content_type = "application/json", body = Error, description = "Project not found"),
    )
)]
pub async fn list_project_members(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<Vec<ProjectMemberResponse>>), AuthError> {
    let project = get_project_or_404(id, &app_state.database).await?;
    ensure_project_read_access(claims.user_id, &project, &app_state.database).await?;

    let members = project_members::Entity::find()
        .filter(project_members::Column::ProjectId.eq(id))
        .order_by_asc(project_members::Column::CreatedAt)
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db member list error: {e}");
            AuthError::DbTimeout
        })?;

    if members.is_empty() {
        return Ok((StatusCode::OK, Json(Vec::new())));
    }

    let user_ids: Vec<Uuid> = members.iter().map(|m| m.user_id).collect();
    let user_rows = users::Entity::find()
        .filter(users::Column::Id.is_in(user_ids))
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db user fetch error: {e}");
            AuthError::DbTimeout
        })?;

    let user_map: HashMap<Uuid, users::Model> = user_rows.into_iter().map(|u| (u.id, u)).collect();

    let result = members
        .into_iter()
        .filter_map(|m| {
            let user = user_map.get(&m.user_id)?;
            Some(ProjectMemberResponse {
                id: m.id,
                user_id: m.user_id,
                name: user.name.clone(),
                email: user.email.clone(),
                picture: user.picture.clone(),
                role: m.role,
                joined_at: m.created_at,
            })
        })
        .collect();

    Ok((StatusCode::OK, Json(result)))
}

#[utoipa::path(
    get,
    path = "/projects/{id}/members/search",
    tag = "projects",
    params(
        ("id" = Uuid, Path, description = "Project id"),
        ("q" = Option<String>, Query, description = "Search query (name or email)"),
        ("limit" = Option<u64>, Query, description = "Max results (default: 20, max: 50)"),
    ),
    responses(
        (status = 200, body = UserSearchResponse),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 403, content_type = "application/json", body = Error, description = "Only project owner can search users"),
        (status = 404, content_type = "application/json", body = Error, description = "Project not found"),
    )
)]
pub async fn search_users_for_project(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(id): Path<Uuid>,
    Query(query): Query<MemberSearchQuery>,
) -> Result<(StatusCode, Json<UserSearchResponse>), AuthError> {
    let project = get_project_or_404(id, &app_state.database).await?;
    ensure_project_owner(claims.user_id, &project)?;

    let q = query.q.as_deref().unwrap_or("").trim().to_string();
    if q.is_empty() {
        return Ok((StatusCode::OK, Json(UserSearchResponse { users: Vec::new() })));
    }

    let limit = query.limit.unwrap_or(20).min(50);

    let member_user_ids: Vec<Uuid> = project_members::Entity::find()
        .select_only()
        .column(project_members::Column::UserId)
        .filter(project_members::Column::ProjectId.eq(id))
        .into_tuple::<Uuid>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db member ids error: {e}");
            AuthError::DbTimeout
        })?;

    let mut excluded = member_user_ids;
    excluded.push(project.owner_id);
    excluded.dedup();

    let pattern = format!("%{q}%");
    let mut user_query = users::Entity::find()
        .filter(
            Condition::any()
                .add(users::Column::Name.into_expr().ilike(&pattern))
                .add(users::Column::Email.into_expr().ilike(&pattern)),
        )
        .limit(limit);

    if !excluded.is_empty() {
        user_query = user_query.filter(users::Column::Id.is_not_in(excluded));
    }

    let found = user_query.all(&app_state.database).await.map_err(|e| {
        eprintln!("db user search error: {e}");
        AuthError::DbTimeout
    })?;

    let result = found
        .into_iter()
        .map(|u| UserSearchItem { id: u.id, name: u.name, email: u.email, picture: u.picture })
        .collect();

    Ok((StatusCode::OK, Json(UserSearchResponse { users: result })))
}

#[utoipa::path(
    get,
    path = "/projects/{id}/artifacts",
    tag = "projects",
    params(("id" = Uuid, Path, description = "Project id")),
    responses(
        (status = 200, body = Vec<ProjectSourceResponse>),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 403, content_type = "application/json", body = Error, description = "Access denied"),
        (status = 404, content_type = "application/json", body = Error, description = "Project not found"),
    )
)]
pub async fn list_project_artifacts(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<Vec<ProjectSourceResponse>>), AuthError> {
    let project = get_project_or_404(id, &app_state.database).await?;
    ensure_project_read_access(claims.user_id, &project, &app_state.database).await?;

    let artifacts = project_sources::Entity::find()
        .filter(project_sources::Column::ProjectId.eq(id))
        .filter(project_sources::Column::Origin.eq("artifact"))
        .order_by_desc(project_sources::Column::UploadedAt)
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db artifacts fetch error: {e}");
            AuthError::DbTimeout
        })?;

    let result = artifacts.into_iter().map(source_to_response).collect();

    Ok((StatusCode::OK, Json(result)))
}

#[utoipa::path(
    post,
    path = "/projects/{id}/artifacts",
    tag = "projects",
    params(("id" = Uuid, Path, description = "Project id")),
    request_body = ArtifactCreateRequest,
    responses(
        (status = 201, body = ProjectSourceResponse),
        (status = 400, content_type = "application/json", body = Error, description = "Invalid or missing fields"),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 403, content_type = "application/json", body = Error, description = "Access denied"),
        (status = 404, content_type = "application/json", body = Error, description = "Project not found"),
    )
)]
pub async fn add_project_artifact(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(req): Json<ArtifactCreateRequest>,
) -> Result<(StatusCode, Json<ProjectSourceResponse>), AuthError> {
    let project = get_project_or_404(id, &app_state.database).await?;
    ensure_project_read_access(claims.user_id, &project, &app_state.database).await?;

    let title = req.title.trim().to_string();
    if title.is_empty() {
        return Err(AuthError::InvalidRequest { field: "title" });
    }
    if req.content.is_empty() {
        return Err(AuthError::InvalidRequest { field: "content" });
    }

    let content_type = req.content_type.trim().to_ascii_lowercase();
    let ext = match content_type.as_str() {
        "text/html" => "html",
        "text/markdown" => "md",
        _ => return Err(AuthError::InvalidRequest { field: "contentType" }),
    };

    let sanitized: String = title
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("_");
    let file_name = format!("{sanitized}.{ext}");
    let file_size = req.content.len() as i64;

    let (file_uuid, _) = write_artifact_file(
        &app_state.database,
        claims.user_id,
        &file_name,
        &content_type,
        &req.content,
    )
    .await
    .map_err(|_| AuthError::DbTimeout)?;

    let inserted = project_sources::ActiveModel {
        id: Set(Uuid::new_v4()),
        project_id: Set(id),
        file_name: Set(file_name),
        file_type: Set(content_type),
        file_size: Set(file_size),
        origin: Set("artifact".to_string()),
        uploaded_at: Set(Utc::now()),
        file_id: Set(Some(file_uuid)),
        processing_status: Set(ProcessingStatus::Pending.to_string()),
        processing_error: Set(None),
    }
    .insert(&app_state.database)
    .await
    .map_err(|e| {
        eprintln!("db artifact insert error: {e}");
        AuthError::DbTimeout
    })?;

    spawn_process_source(app_state, inserted.id, inserted.project_id, file_uuid);

    Ok((StatusCode::CREATED, Json(source_to_response(inserted))))
}

// --- artifact CRUD ---

#[utoipa::path(
    get,
    path = "/projects/{id}/artifacts/{artifact_id}",
    tag = "projects",
    params(
        ("id" = Uuid, Path, description = "Project id"),
        ("artifact_id" = Uuid, Path, description = "Artifact id"),
    ),
    responses(
        (status = 200, body = ProjectSourceResponse),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 403, content_type = "application/json", body = Error, description = "Access denied"),
        (status = 404, content_type = "application/json", body = Error, description = "Artifact not found"),
    )
)]
pub async fn get_project_artifact(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path((id, artifact_id)): Path<(Uuid, Uuid)>,
) -> Result<(StatusCode, Json<ProjectSourceResponse>), AuthError> {
    let project = get_project_or_404(id, &app_state.database).await?;
    ensure_project_read_access(claims.user_id, &project, &app_state.database).await?;

    let artifact = project_sources::Entity::find()
        .filter(project_sources::Column::Id.eq(artifact_id))
        .filter(project_sources::Column::ProjectId.eq(id))
        .filter(project_sources::Column::Origin.eq("artifact"))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db artifact fetch error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    Ok((StatusCode::OK, Json(source_to_response(artifact))))
}

#[utoipa::path(
    put,
    path = "/projects/{id}/artifacts/{artifact_id}",
    tag = "projects",
    params(
        ("id" = Uuid, Path, description = "Project id"),
        ("artifact_id" = Uuid, Path, description = "Artifact id"),
    ),
    request_body = ArtifactUpdateRequest,
    responses(
        (status = 200, body = ProjectSourceResponse),
        (status = 400, content_type = "application/json", body = Error, description = "Invalid fields"),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 403, content_type = "application/json", body = Error, description = "Access denied"),
        (status = 404, content_type = "application/json", body = Error, description = "Artifact not found"),
    )
)]
pub async fn update_project_artifact(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path((id, artifact_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<ArtifactUpdateRequest>,
) -> Result<(StatusCode, Json<ProjectSourceResponse>), AuthError> {
    let project = get_project_or_404(id, &app_state.database).await?;
    ensure_project_read_access(claims.user_id, &project, &app_state.database).await?;

    let artifact = project_sources::Entity::find()
        .filter(project_sources::Column::Id.eq(artifact_id))
        .filter(project_sources::Column::ProjectId.eq(id))
        .filter(project_sources::Column::Origin.eq("artifact"))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db artifact fetch error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    use sea_orm::IntoActiveModel;
    let mut active = artifact.clone().into_active_model();

    let new_content_type = req.content_type.as_deref().map(|ct| ct.trim().to_ascii_lowercase());
    if let Some(ref ct) = new_content_type {
        if !matches!(ct.as_str(), "text/html" | "text/markdown") {
            return Err(AuthError::InvalidRequest { field: "content_type" });
        }
        active.file_type = Set(ct.clone());
    }

    let resolved_content_type = new_content_type
        .as_deref()
        .unwrap_or(&artifact.file_type)
        .to_string();
    let ext = if resolved_content_type == "text/html" { "html" } else { "md" };

    let new_title = req.title.as_deref().map(|t| t.trim().to_string());
    if let Some(ref title) = new_title {
        if title.is_empty() {
            return Err(AuthError::InvalidRequest { field: "title" });
        }
        let sanitized: String = title
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("_");
        active.file_name = Set(format!("{sanitized}.{ext}"));
    }

    let mut retrigger_file_id: Option<Uuid> = None;

    if let Some(ref content) = req.content {
        let file_name = active.file_name.clone().unwrap();
        let (file_uuid, _) = write_artifact_file(
            &app_state.database,
            claims.user_id,
            &file_name,
            &resolved_content_type,
            content,
        )
        .await
        .map_err(|_| AuthError::DbTimeout)?;

        active.file_id = Set(Some(file_uuid));
        active.file_size = Set(content.len() as i64);
        active.processing_status = Set(ProcessingStatus::Pending.to_string());
        active.processing_error = Set(None);
        retrigger_file_id = Some(file_uuid);
    }

    let updated = active.update(&app_state.database).await.map_err(|e| {
        eprintln!("db artifact update error: {e}");
        AuthError::DbTimeout
    })?;

    if let Some(fid) = retrigger_file_id {
        spawn_process_source(app_state, updated.id, updated.project_id, fid);
    }

    Ok((StatusCode::OK, Json(source_to_response(updated))))
}

#[utoipa::path(
    delete,
    path = "/projects/{id}/artifacts/{artifact_id}",
    tag = "projects",
    params(
        ("id" = Uuid, Path, description = "Project id"),
        ("artifact_id" = Uuid, Path, description = "Artifact id"),
    ),
    responses(
        (status = 204, description = "Artifact deleted"),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 403, content_type = "application/json", body = Error, description = "Access denied"),
        (status = 404, content_type = "application/json", body = Error, description = "Artifact not found"),
    )
)]
pub async fn delete_project_artifact(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path((id, artifact_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AuthError> {
    let project = get_project_or_404(id, &app_state.database).await?;
    ensure_project_read_access(claims.user_id, &project, &app_state.database).await?;

    let artifact = project_sources::Entity::find()
        .filter(project_sources::Column::Id.eq(artifact_id))
        .filter(project_sources::Column::ProjectId.eq(id))
        .filter(project_sources::Column::Origin.eq("artifact"))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db artifact fetch error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    delete_source_chunks(&app_state.database, artifact_id).await.map_err(|_| AuthError::DbTimeout)?;

    project_sources::Entity::delete_by_id(artifact_id)
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db artifact delete error: {e}");
            AuthError::DbTimeout
        })?;

    if let Some(fid) = artifact.file_id {
        if let Ok(Some(file)) = crate::models::files::Entity::find_by_id(fid)
            .one(&app_state.database)
            .await
        {
            let _ = tokio::fs::remove_file(&file.local_path).await;
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

// --- project MCP server helpers ---


// --- project MCP server CRUD ---

#[utoipa::path(
    get,
    path = "/projects/{id}/mcp-servers",
    tag = "projects",
    params(("id" = Uuid, Path, description = "Project id")),
    responses(
        (status = 200, body = Vec<ProjectMcpServerResponse>),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 404, content_type = "application/json", body = Error, description = "Project not found"),
    )
)]
pub async fn list_project_mcp_servers(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<Vec<ProjectMcpServerResponse>>), AuthError> {
    let project = get_project_or_404(id, &app_state.database).await?;
    ensure_project_read_access(claims.user_id, &project, &app_state.database).await?;
    let result = fetch_project_mcp_servers(id, &app_state.database).await?;
    Ok((StatusCode::OK, Json(result)))
}

#[utoipa::path(
    post,
    path = "/projects/{id}/mcp-servers",
    tag = "projects",
    params(("id" = Uuid, Path, description = "Project id")),
    request_body = AddMcpServerRequest,
    responses(
        (status = 201, body = ProjectMcpServerResponse),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 403, content_type = "application/json", body = Error, description = "Only project owner/admin can modify"),
        (status = 404, content_type = "application/json", body = Error, description = "Project or MCP server not found"),
        (status = 409, content_type = "application/json", body = Error, description = "Server already attached"),
    )
)]
pub async fn add_project_mcp_server(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(req): Json<AddMcpServerRequest>,
) -> Result<(StatusCode, Json<ProjectMcpServerResponse>), AuthError> {
    let project = get_project_or_404(id, &app_state.database).await?;
    ensure_project_write_access(claims.user_id, &project, &app_state.database).await?;

    let server = mcp_servers::Entity::find_by_id(req.server_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db mcp server lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    let existing = project_mcp_servers::Entity::find()
        .filter(project_mcp_servers::Column::ProjectId.eq(id))
        .filter(project_mcp_servers::Column::ServerId.eq(req.server_id))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db project mcp server check error: {e}");
            AuthError::DbTimeout
        })?;

    if existing.is_some() {
        return Err(AuthError::DbConflict);
    }

    let now = Utc::now();
    let inserted = project_mcp_servers::ActiveModel {
        id: Set(Uuid::new_v4()),
        project_id: Set(id),
        server_id: Set(req.server_id),
        created_at: Set(now),
    }
    .insert(&app_state.database)
    .await
    .map_err(|e| {
        eprintln!("db project mcp server insert error: {e}");
        AuthError::DbTimeout
    })?;

    Ok((StatusCode::CREATED, Json(ProjectMcpServerResponse {
        id: inserted.id,
        server_id: inserted.server_id,
        name: server.name,
        description: server.description,
        added_at: inserted.created_at,
    })))
}

#[utoipa::path(
    delete,
    path = "/projects/{id}/mcp-servers/{server_id}",
    tag = "projects",
    params(
        ("id" = Uuid, Path, description = "Project id"),
        ("server_id" = Uuid, Path, description = "MCP server id"),
    ),
    responses(
        (status = 204),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token"),
        (status = 403, content_type = "application/json", body = Error, description = "Only project owner/admin can modify"),
        (status = 404, content_type = "application/json", body = Error, description = "Not found"),
    )
)]
pub async fn remove_project_mcp_server(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path((id, server_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AuthError> {
    let project = get_project_or_404(id, &app_state.database).await?;
    ensure_project_write_access(claims.user_id, &project, &app_state.database).await?;

    let row = project_mcp_servers::Entity::find()
        .filter(project_mcp_servers::Column::ProjectId.eq(id))
        .filter(project_mcp_servers::Column::ServerId.eq(server_id))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db project mcp server find error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    project_mcp_servers::Entity::delete_by_id(row.id)
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db project mcp server delete error: {e}");
            AuthError::DbTimeout
        })?;

    Ok(StatusCode::NO_CONTENT)
}
