// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect, Select};
use uuid::Uuid;

use crate::{
    error::AppError,
    models::{conversation_projects, conversations, projects},
    services::project_helpers::{build_visibility_condition, fetch_member_project_ids},
};

// Conversations are private to their owner; GET /chat/{id} applies the same rule
// and there is no admin or project-member override.
pub fn can_access_conversation(owner_id: Uuid, requester_id: Uuid) -> bool {
    owner_id == requester_id
}

pub async fn find_active_conversation_for_user(
    db: &DatabaseConnection,
    conversation_id: Uuid,
    user_id: Uuid,
) -> Result<conversations::Model, AppError> {
    let conversation = conversations::Entity::find_by_id(conversation_id)
        .filter(conversations::Column::ArchivedAt.is_null())
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("conversation lookup error: {e}");
            AppError::DbTimeout
        })?
        .ok_or(AppError::DbNotFound)?;
    if !can_access_conversation(conversation.user_id, user_id) {
        return Err(AppError::DbNotFound);
    }
    Ok(conversation)
}

pub async fn load_readable_linked_projects(
    db: &DatabaseConnection,
    conversation_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<projects::Model>, AppError> {
    let linked_ids = conversation_projects::Entity::find()
        .select_only()
        .column(conversation_projects::Column::ProjectId)
        .filter(conversation_projects::Column::ConversationId.eq(conversation_id))
        .into_tuple::<Uuid>()
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("linked projects lookup error: {e}");
            AppError::DbTimeout
        })?;
    if linked_ids.is_empty() {
        return Ok(Vec::new());
    }
    let member_project_ids = fetch_member_project_ids(user_id, db)
        .await
        .map_err(|_| AppError::DbTimeout)?;
    readable_projects_query(linked_ids, user_id, member_project_ids)
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("readable linked projects lookup error: {e}");
            AppError::DbTimeout
        })
}

fn readable_projects_query(
    project_ids: Vec<Uuid>,
    user_id: Uuid,
    member_project_ids: Vec<Uuid>,
) -> Select<projects::Entity> {
    projects::Entity::find()
        .filter(projects::Column::Id.is_in(project_ids))
        .filter(build_visibility_condition(user_id, member_project_ids))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DbBackend, QueryTrait};

    #[test]
    fn owner_can_access_own_conversation() {
        let owner = Uuid::new_v4();
        assert!(can_access_conversation(owner, owner));
    }

    #[test]
    fn other_user_cannot_access_conversation() {
        assert!(!can_access_conversation(Uuid::new_v4(), Uuid::new_v4()));
    }

    #[test]
    fn nil_requester_cannot_access_conversation() {
        assert!(!can_access_conversation(Uuid::new_v4(), Uuid::nil()));
    }

    #[test]
    fn linked_projects_are_limited_to_projects_the_user_can_read() {
        let user_id = Uuid::new_v4();
        let linked = Uuid::new_v4();
        let membership = Uuid::new_v4();
        let sql = readable_projects_query(vec![linked], user_id, vec![membership])
            .build(DbBackend::Postgres)
            .to_string();

        assert!(sql.contains(&format!("\"projects\".\"id\" IN ('{linked}')")));
        assert!(sql.contains(&format!("\"projects\".\"ownerId\" = '{user_id}'")));
        assert!(sql.contains("\"projects\".\"visibility\" = 'team'"));
        assert!(sql.contains(&format!("\"projects\".\"id\" IN ('{membership}')")));
        assert!(sql.contains(" OR "));
    }

    #[test]
    fn private_linked_projects_without_membership_need_ownership() {
        let user_id = Uuid::new_v4();
        let sql = readable_projects_query(vec![Uuid::new_v4()], user_id, Vec::new())
            .build(DbBackend::Postgres)
            .to_string();

        assert!(sql.contains(&format!("\"projects\".\"ownerId\" = '{user_id}'")));
        assert!(sql.contains("\"projects\".\"visibility\" = 'team'"));
        assert_eq!(sql.matches(" IN (").count(), 1);
    }
}
