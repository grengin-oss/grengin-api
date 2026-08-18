// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::error::AuthError,
    dto::projects::{
        ProjectChatResponse, ProjectMcpServerResponse, ProjectResponse, ProjectSourceResponse,
    },
    models::{
        conversation_projects, conversations, mcp_servers, project_mcp_servers, project_members,
        project_sources, project_sources::ProcessingStatus, projects, projects::ProjectVisibility,
    },
};
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use std::collections::HashMap;
use uuid::Uuid;

pub async fn get_project_or_404(
    id: Uuid,
    db: &DatabaseConnection,
) -> Result<projects::Model, AuthError> {
    projects::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("db find project error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)
}

pub async fn ensure_project_read_access(
    user_id: Uuid,
    project: &projects::Model,
    db: &DatabaseConnection,
) -> Result<(), AuthError> {
    if project.owner_id == user_id || project.visibility == ProjectVisibility::Team {
        return Ok(());
    }
    let is_member = project_members::Entity::find()
        .filter(project_members::Column::ProjectId.eq(project.id))
        .filter(project_members::Column::UserId.eq(user_id))
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("db member check error: {e}");
            AuthError::DbTimeout
        })?
        .is_some();
    if is_member {
        Ok(())
    } else {
        Err(AuthError::PermissionDenied)
    }
}

pub fn ensure_project_owner(user_id: Uuid, project: &projects::Model) -> Result<(), AuthError> {
    if project.owner_id == user_id {
        Ok(())
    } else {
        Err(AuthError::PermissionDenied)
    }
}

pub async fn ensure_project_write_access(
    user_id: Uuid,
    project: &projects::Model,
    db: &DatabaseConnection,
) -> Result<(), AuthError> {
    if project.owner_id == user_id {
        return Ok(());
    }
    let member = project_members::Entity::find()
        .filter(project_members::Column::ProjectId.eq(project.id))
        .filter(project_members::Column::UserId.eq(user_id))
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("db member check error: {e}");
            AuthError::DbTimeout
        })?;
    match member {
        Some(m) if m.role == "admin" => Ok(()),
        _ => Err(AuthError::PermissionDenied),
    }
}

pub fn build_visibility_condition(user_id: Uuid, member_project_ids: Vec<Uuid>) -> Condition {
    let mut cond = Condition::any()
        .add(projects::Column::OwnerId.eq(user_id))
        .add(projects::Column::Visibility.eq(ProjectVisibility::Team));
    if !member_project_ids.is_empty() {
        cond = cond.add(projects::Column::Id.is_in(member_project_ids));
    }
    cond
}

pub async fn fetch_member_project_ids(
    user_id: Uuid,
    db: &DatabaseConnection,
) -> Result<Vec<Uuid>, AuthError> {
    project_members::Entity::find()
        .select_only()
        .column(project_members::Column::ProjectId)
        .filter(project_members::Column::UserId.eq(user_id))
        .into_tuple::<Uuid>()
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("db member ids error: {e}");
            AuthError::DbTimeout
        })
}

pub async fn fetch_counts(
    project_ids: &[Uuid],
    db: &DatabaseConnection,
) -> Result<(HashMap<Uuid, i64>, HashMap<Uuid, i64>, HashMap<Uuid, i64>), AuthError> {
    if project_ids.is_empty() {
        return Ok((HashMap::new(), HashMap::new(), HashMap::new()));
    }

    let chat_rows: Vec<(Uuid, i64)> = conversation_projects::Entity::find()
        .select_only()
        .column(conversation_projects::Column::ProjectId)
        .column_as(conversation_projects::Column::Id.count(), "cnt")
        .filter(conversation_projects::Column::ProjectId.is_in(project_ids.to_vec()))
        .group_by(conversation_projects::Column::ProjectId)
        .into_tuple::<(Uuid, i64)>()
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("db chat count error: {e}");
            AuthError::DbTimeout
        })?;

    let source_rows: Vec<(Uuid, i64)> = project_sources::Entity::find()
        .select_only()
        .column(project_sources::Column::ProjectId)
        .column_as(project_sources::Column::Id.count(), "cnt")
        .filter(project_sources::Column::ProjectId.is_in(project_ids.to_vec()))
        .group_by(project_sources::Column::ProjectId)
        .into_tuple::<(Uuid, i64)>()
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("db source count error: {e}");
            AuthError::DbTimeout
        })?;

    let member_rows: Vec<(Uuid, i64)> = project_members::Entity::find()
        .select_only()
        .column(project_members::Column::ProjectId)
        .column_as(project_members::Column::Id.count(), "cnt")
        .filter(project_members::Column::ProjectId.is_in(project_ids.to_vec()))
        .group_by(project_members::Column::ProjectId)
        .into_tuple::<(Uuid, i64)>()
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("db member count error: {e}");
            AuthError::DbTimeout
        })?;

    Ok((
        chat_rows.into_iter().collect(),
        source_rows.into_iter().collect(),
        member_rows.into_iter().collect(),
    ))
}

pub fn to_project_response(
    model: projects::Model,
    chat_counts: &HashMap<Uuid, i64>,
    source_counts: &HashMap<Uuid, i64>,
    member_counts: &HashMap<Uuid, i64>,
) -> ProjectResponse {
    ProjectResponse {
        id: model.id,
        name: model.name,
        description: model.description.unwrap_or_default(),
        category: model.category,
        visibility: model.visibility,
        owner_id: model.owner_id,
        chat_count: *chat_counts.get(&model.id).unwrap_or(&0),
        source_count: *source_counts.get(&model.id).unwrap_or(&0),
        member_count: *member_counts.get(&model.id).unwrap_or(&0),
        last_activity_at: model.last_activity_at,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

pub fn source_to_response(s: project_sources::Model) -> ProjectSourceResponse {
    ProjectSourceResponse {
        id: s.id,
        project_id: s.project_id,
        file_name: s.file_name,
        file_type: s.file_type,
        file_size: s.file_size,
        origin: s.origin,
        uploaded_at: s.uploaded_at,
        file_id: s.file_id,
        processing_status: ProcessingStatus::try_from(s.processing_status)
            .unwrap_or(ProcessingStatus::Error),
        processing_error: s.processing_error,
    }
}

pub async fn fetch_project_sources(
    project_id: Uuid,
    db: &DatabaseConnection,
) -> Result<Vec<ProjectSourceResponse>, AuthError> {
    let sources = project_sources::Entity::find()
        .filter(project_sources::Column::ProjectId.eq(project_id))
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("db sources fetch error: {e}");
            AuthError::DbTimeout
        })?;
    Ok(sources.into_iter().map(source_to_response).collect())
}

pub async fn fetch_project_chats(
    project_id: Uuid,
    db: &DatabaseConnection,
) -> Result<Vec<ProjectChatResponse>, AuthError> {
    let conv_ids: Vec<Uuid> = conversation_projects::Entity::find()
        .select_only()
        .column(conversation_projects::Column::ConversationId)
        .filter(conversation_projects::Column::ProjectId.eq(project_id))
        .into_tuple::<Uuid>()
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("db conv_ids fetch error: {e}");
            AuthError::DbTimeout
        })?;

    if conv_ids.is_empty() {
        return Ok(vec![]);
    }

    let convs = conversations::Entity::find()
        .filter(conversations::Column::Id.is_in(conv_ids))
        .order_by_desc(conversations::Column::UpdatedAt)
        .limit(50)
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("db chats fetch error: {e}");
            AuthError::DbTimeout
        })?;

    Ok(convs
        .into_iter()
        .map(|c| ProjectChatResponse {
            id: c.id,
            title: c.title,
            message_count: c.message_count,
            created_at: c.created_at,
            updated_at: c.updated_at,
        })
        .collect())
}

pub fn is_valid_category(s: &str) -> bool {
    crate::models::projects::VALID_CATEGORIES.contains(&s)
}

pub async fn fetch_project_mcp_servers(
    project_id: Uuid,
    db: &DatabaseConnection,
) -> Result<Vec<ProjectMcpServerResponse>, AuthError> {
    let rows = project_mcp_servers::Entity::find()
        .filter(project_mcp_servers::Column::ProjectId.eq(project_id))
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("db project mcp servers fetch error: {e}");
            AuthError::DbTimeout
        })?;

    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let server_ids: Vec<Uuid> = rows.iter().map(|r| r.server_id).collect();
    let servers = mcp_servers::Entity::find()
        .filter(mcp_servers::Column::Id.is_in(server_ids))
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("db mcp servers fetch error: {e}");
            AuthError::DbTimeout
        })?;

    let server_map: HashMap<Uuid, mcp_servers::Model> =
        servers.into_iter().map(|s| (s.id, s)).collect();

    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let server = server_map.get(&row.server_id)?;
            Some(ProjectMcpServerResponse {
                id: row.id,
                server_id: row.server_id,
                name: server.name.clone(),
                description: server.description.clone(),
                added_at: row.created_at,
            })
        })
        .collect())
}
