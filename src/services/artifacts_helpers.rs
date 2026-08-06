// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::models::{artifacts, conversations, files};

pub struct ArtifactWithContent {
    pub artifact: artifacts::Model,
    pub content: Option<String>,
}

pub async fn get_artifact_owned(
    db: &DatabaseConnection,
    artifact_id: Uuid,
    user_id: Uuid,
) -> Result<Option<ArtifactWithContent>, DbErr> {
    let row = artifacts::Entity::find_by_id(artifact_id)
        .inner_join(conversations::Entity)
        .filter(conversations::Column::UserId.eq(user_id))
        .one(db)
        .await?;

    let artifact = match row {
        Some(a) => a,
        None => return Ok(None),
    };

    let file = files::Entity::find_by_id(artifact.file_id).one(db).await?;
    let content = file.and_then(|f| {
        std::fs::read_to_string(&f.local_path).ok()
    });

    Ok(Some(ArtifactWithContent { artifact, content }))
}

pub async fn list_conversation_artifacts_owned(
    db: &DatabaseConnection,
    conversation_id: Uuid,
    user_id: Uuid,
) -> Result<Option<Vec<artifacts::Model>>, DbErr> {
    let conversation = conversations::Entity::find_by_id(conversation_id)
        .filter(conversations::Column::UserId.eq(user_id))
        .one(db)
        .await?;

    if conversation.is_none() {
        return Ok(None);
    }

    let rows = artifacts::Entity::find()
        .filter(artifacts::Column::ConversationId.eq(conversation_id))
        .all(db)
        .await?;

    Ok(Some(rows))
}

pub async fn delete_artifact_owned(
    db: &DatabaseConnection,
    artifact_id: Uuid,
    user_id: Uuid,
) -> Result<Option<artifacts::Model>, DbErr> {
    let row = artifacts::Entity::find_by_id(artifact_id)
        .inner_join(conversations::Entity)
        .filter(conversations::Column::UserId.eq(user_id))
        .one(db)
        .await?;

    let artifact = match row {
        Some(a) => a,
        None => return Ok(None),
    };

    let file = files::Entity::find_by_id(artifact.file_id).one(db).await?;
    if let Some(f) = file {
        let _ = tokio::fs::remove_file(&f.local_path).await;
        if let Some(parent) = std::path::Path::new(&f.local_path).parent() {
            let _ = tokio::fs::remove_dir(parent).await;
        }
        files::Entity::delete_by_id(f.id).exec(db).await?;
    }

    artifacts::Entity::delete_by_id(artifact.id).exec(db).await?;

    Ok(Some(artifact))
}
