// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use std::path::Path;
use uuid::Uuid;

use crate::{
    error::AppError,
    models::{artifacts, conversations, files},
    services::file_storage::read_model_bytes,
};

pub struct ArtifactWithContent {
    pub artifact: artifacts::Model,
    pub content: Option<String>,
}

pub async fn get_artifact_owned(
    db: &DatabaseConnection,
    file_storage_root: &Path,
    artifact_id: Uuid,
    user_id: Uuid,
) -> Result<Option<ArtifactWithContent>, AppError> {
    let row = artifacts::Entity::find_by_id(artifact_id)
        .inner_join(conversations::Entity)
        .filter(conversations::Column::UserId.eq(user_id))
        .one(db)
        .await
        .map_err(|error| {
            eprintln!("db artifact lookup failed: {error}");
            AppError::DbTimeout
        })?;

    let artifact = match row {
        Some(a) => a,
        None => return Ok(None),
    };

    let file = files::Entity::find_by_id(artifact.file_id)
        .one(db)
        .await
        .map_err(|error| {
            eprintln!("db artifact file lookup failed: {error}");
            AppError::DbTimeout
        })?;
    let content = match file {
        Some(file) => match read_model_bytes(file_storage_root, &file).await {
            Ok(bytes) => String::from_utf8(bytes).ok(),
            Err(AppError::ResourceNotFound) => None,
            Err(error) => return Err(error),
        },
        None => None,
    };

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
) -> Result<Option<artifacts::Model>, AppError> {
    let row = artifacts::Entity::find_by_id(artifact_id)
        .inner_join(conversations::Entity)
        .filter(conversations::Column::UserId.eq(user_id))
        .one(db)
        .await
        .map_err(|error| {
            eprintln!("db artifact lookup failed: {error}");
            AppError::DbTimeout
        })?;

    let artifact = match row {
        Some(a) => a,
        None => return Ok(None),
    };

    // Files can also back project sources and chat attachments. Removing this association
    // must not destroy their bytes; explicit file deletion remains the owner's operation.
    artifacts::Entity::delete_by_id(artifact.id)
        .exec(db)
        .await
        .map_err(|error| {
            eprintln!("db artifact delete failed: {error}");
            AppError::DbTimeout
        })?;

    Ok(Some(artifact))
}
