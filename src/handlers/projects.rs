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
        AddMemberRequest, AddSourceRequest, InstructionsUpdateRequest, LinkProjectRequest,
        ProjectCreateRequest, ProjectDetailResponse, ProjectListQuery, ProjectListResponse,
        ProjectResponse, ProjectSourceResponse, ProjectUpdateRequest, ShareProjectResponse,
    },
    models::{conversation_projects, conversations, project_members, project_sources, projects},
    services::project_helpers::*,
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
    if let Some(vis) = query.visibility.as_deref().filter(|v| !v.is_empty()) {
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

    let visibility = req.visibility.as_deref().unwrap_or("private").trim().to_ascii_lowercase();
    if !is_valid_visibility(&visibility) {
        return Err(AuthError::InvalidRequest { field: "visibility" });
    }

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
        let vis = vis.trim().to_ascii_lowercase();
        if !is_valid_visibility(&vis) {
            return Err(AuthError::InvalidRequest { field: "visibility" });
        }
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

    let inserted = project_sources::ActiveModel {
        id: Set(Uuid::new_v4()),
        project_id: Set(id),
        file_name: Set(req.file_name.trim().to_string()),
        file_type: Set(req.file_type.trim().to_string()),
        file_size: Set(req.file_size),
        origin: Set(origin),
        uploaded_at: Set(Utc::now()),
    }
    .insert(&app_state.database)
    .await
    .map_err(|e| {
        eprintln!("db source insert error: {e}");
        AuthError::DbTimeout
    })?;

    Ok((StatusCode::CREATED, Json(ProjectSourceResponse {
        id: inserted.id,
        project_id: inserted.project_id,
        file_name: inserted.file_name,
        file_type: inserted.file_type,
        file_size: inserted.file_size,
        uploaded_at: inserted.uploaded_at,
    })))
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
