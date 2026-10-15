// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::error::AuthError,
    dto::projects::{
        ProjectChatResponse, ProjectMcpServerResponse, ProjectResponse, ProjectSourceResponse,
    },
    models::{
        conversation_projects, conversations, files, files::FileUploadStatus, mcp_servers,
        project_mcp_servers, project_members, project_members::ProjectMemberRole, project_sources,
        project_sources::ProcessingStatus, projects, projects::ProjectVisibility,
    },
};
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect,
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

async fn find_project_member(
    user_id: Uuid,
    project_id: Uuid,
    db: &DatabaseConnection,
) -> Result<Option<project_members::Model>, AuthError> {
    project_members::Entity::find()
        .filter(project_members::Column::ProjectId.eq(project_id))
        .filter(project_members::Column::UserId.eq(user_id))
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("db member check error: {e}");
            AuthError::DbTimeout
        })
}

pub async fn ensure_project_write_access(
    user_id: Uuid,
    project: &projects::Model,
    db: &DatabaseConnection,
) -> Result<(), AuthError> {
    if project.owner_id == user_id {
        return Ok(());
    }
    let role = find_project_member(user_id, project.id, db)
        .await?
        .and_then(|m| ProjectMemberRole::try_from(m.role).ok());
    if can_write_project(user_id, project, role) {
        Ok(())
    } else {
        Err(AuthError::PermissionDenied)
    }
}

pub async fn ensure_project_content_access(
    user_id: Uuid,
    project: &projects::Model,
    db: &DatabaseConnection,
) -> Result<(), AuthError> {
    if project.owner_id == user_id {
        return Ok(());
    }
    let is_member = find_project_member(user_id, project.id, db)
        .await?
        .is_some();
    if can_edit_project_content(user_id, project, is_member) {
        Ok(())
    } else {
        Err(AuthError::PermissionDenied)
    }
}

// Sources and artifacts are shared work: any explicit member may edit them, but
// users who can only read a team project may not.
fn can_edit_project_content(user_id: Uuid, project: &projects::Model, is_member: bool) -> bool {
    project.owner_id == user_id || is_member
}

fn can_write_project(
    user_id: Uuid,
    project: &projects::Model,
    member_role: Option<ProjectMemberRole>,
) -> bool {
    project.owner_id == user_id || member_role.is_some_and(ProjectMemberRole::can_write)
}

pub async fn ensure_source_file_attachable(
    user_id: Uuid,
    project_id: Uuid,
    file_id: Uuid,
    db: &DatabaseConnection,
) -> Result<(), AuthError> {
    let file = files::Entity::find_by_id(file_id)
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("db source file lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;
    let already_in_project = file.user_id != user_id
        && project_sources::Entity::find()
            .filter(project_sources::Column::ProjectId.eq(project_id))
            .filter(project_sources::Column::FileId.eq(file_id))
            .count(db)
            .await
            .map_err(|e| {
                eprintln!("db source file attachment check error: {e}");
                AuthError::DbTimeout
            })?
            > 0;
    if can_attach_source_file(user_id, &file, already_in_project) {
        Ok(())
    } else {
        Err(AuthError::ResourceNotFound)
    }
}

fn can_attach_source_file(user_id: Uuid, file: &files::Model, already_in_project: bool) -> bool {
    file.status == FileUploadStatus::Uploaded && (file.user_id == user_id || already_in_project)
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

#[cfg(test)]
mod tests {
    use super::{can_attach_source_file, can_edit_project_content, can_write_project};
    use crate::models::{
        files, files::FileUploadStatus, project_members::ProjectMemberRole, projects,
        projects::ProjectVisibility,
    };
    use chrono::Utc;
    use uuid::Uuid;

    fn project(owner_id: Uuid, visibility: ProjectVisibility) -> projects::Model {
        let now = Utc::now();
        projects::Model {
            id: Uuid::new_v4(),
            name: "Project".to_string(),
            description: None,
            category: "research".to_string(),
            visibility,
            owner_id,
            instructions: None,
            last_activity_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn file(owner_id: Uuid, status: FileUploadStatus) -> files::Model {
        let now = Utc::now();
        files::Model {
            id: Uuid::new_v4(),
            user_id: owner_id,
            name: "doc.md".to_string(),
            content_type: "text/markdown".to_string(),
            size: 1,
            local_path: "/unused".to_string(),
            description: None,
            url: None,
            sha256: None,
            status,
            created_at: now,
            updated_at: now,
            metadata: None,
        }
    }

    #[test]
    fn project_owner_can_write_private_and_team_projects() {
        let owner = Uuid::new_v4();
        for visibility in [ProjectVisibility::Private, ProjectVisibility::Team] {
            assert!(can_write_project(owner, &project(owner, visibility), None));
        }
    }

    #[test]
    fn non_member_cannot_write_team_project_they_can_read() {
        let project = project(Uuid::new_v4(), ProjectVisibility::Team);
        assert!(!can_write_project(Uuid::new_v4(), &project, None));
    }

    #[test]
    fn plain_member_cannot_write_private_or_team_project() {
        for visibility in [ProjectVisibility::Private, ProjectVisibility::Team] {
            let project = project(Uuid::new_v4(), visibility);
            assert!(!can_write_project(
                Uuid::new_v4(),
                &project,
                Some(ProjectMemberRole::Member)
            ));
        }
    }

    #[test]
    fn owner_and_admin_role_members_can_write_project() {
        let project = project(Uuid::new_v4(), ProjectVisibility::Private);
        for role in [ProjectMemberRole::Owner, ProjectMemberRole::Admin] {
            assert!(can_write_project(Uuid::new_v4(), &project, Some(role)));
        }
    }

    #[test]
    fn unknown_member_role_is_not_parsed_into_a_write_role() {
        assert!(ProjectMemberRole::try_from("editor".to_string()).is_err());
        assert_eq!(
            ProjectMemberRole::try_from("owner".to_string()),
            Ok(ProjectMemberRole::Owner)
        );
    }

    #[test]
    fn project_owner_can_edit_sources_and_artifacts() {
        let owner = Uuid::new_v4();
        for visibility in [ProjectVisibility::Private, ProjectVisibility::Team] {
            assert!(can_edit_project_content(
                owner,
                &project(owner, visibility),
                false
            ));
        }
    }

    #[test]
    fn any_explicit_member_can_edit_sources_and_artifacts() {
        let project = project(Uuid::new_v4(), ProjectVisibility::Team);
        assert!(can_edit_project_content(Uuid::new_v4(), &project, true));
    }

    #[test]
    fn non_member_who_can_read_a_team_project_cannot_edit_its_sources() {
        let project = project(Uuid::new_v4(), ProjectVisibility::Team);
        assert!(!can_edit_project_content(Uuid::new_v4(), &project, false));
    }

    #[test]
    fn user_can_attach_their_own_uploaded_file() {
        let user = Uuid::new_v4();
        assert!(can_attach_source_file(
            user,
            &file(user, FileUploadStatus::Uploaded),
            false
        ));
    }

    #[test]
    fn user_cannot_attach_another_users_file() {
        assert!(!can_attach_source_file(
            Uuid::new_v4(),
            &file(Uuid::new_v4(), FileUploadStatus::Uploaded),
            false
        ));
    }

    #[test]
    fn user_can_reattach_a_file_already_in_the_project() {
        assert!(can_attach_source_file(
            Uuid::new_v4(),
            &file(Uuid::new_v4(), FileUploadStatus::Uploaded),
            true
        ));
    }

    #[test]
    fn deleted_file_cannot_be_attached() {
        let user = Uuid::new_v4();
        assert!(!can_attach_source_file(
            user,
            &file(user, FileUploadStatus::Deleted),
            false
        ));
        assert!(!can_attach_source_file(
            user,
            &file(Uuid::new_v4(), FileUploadStatus::Deleted),
            true
        ));
    }
}
