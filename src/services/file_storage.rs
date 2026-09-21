// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    dto::files::{Attachment, FileUploadRequest},
    error::AppError,
    models::files::{self, FileUploadStatus},
};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr,
    EntityTrait, QueryFilter,
};
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::future::Future;
use std::path::{Component, Path, PathBuf};
use tokio::fs;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageCategory {
    File,
    Artifact,
    Image,
    Skill,
}

impl Display for StorageCategory {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::File => "file",
            Self::Artifact => "artifact",
            Self::Image => "images",
            Self::Skill => "skill",
        })
    }
}

pub struct FileWrite<'a> {
    pub id: Uuid,
    pub category: StorageCategory,
    pub name: &'a str,
    pub content_type: &'a str,
    pub bytes: &'a [u8],
    pub description: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

pub fn safe_file_name(value: &str) -> Option<&str> {
    let mut components = Path::new(value).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) if !value.is_empty() => Some(value),
        _ => None,
    }
}

pub fn new_file_path(
    root: &Path,
    user_id: Uuid,
    category: StorageCategory,
    file_id: Uuid,
    file_name: &str,
) -> Result<PathBuf, AppError> {
    let file_name =
        safe_file_name(file_name).ok_or(AppError::ValidationEmptyField { field: "file_name" })?;
    Ok(root
        .join(user_id.to_string())
        .join(category.to_string())
        .join(file_id.to_string())
        .join(file_name))
}

pub async fn prepare_storage_root(root: &Path) -> Result<(), AppError> {
    fs::create_dir_all(root).await.map_err(|error| {
        eprintln!("file storage initialization failed: {error}");
        AppError::ServiceTemporarilyUnavailable
    })?;

    let probe = root.join(format!(".grengin-write-probe-{}", Uuid::new_v4()));
    fs::write(&probe, b"ready").await.map_err(|error| {
        eprintln!("file storage write probe failed: {error}");
        AppError::ServiceTemporarilyUnavailable
    })?;
    fs::remove_file(&probe).await.map_err(|error| {
        eprintln!("file storage write probe cleanup failed: {error}");
        AppError::ServiceTemporarilyUnavailable
    })?;
    Ok(())
}

pub async fn store_uploaded_file(
    db: &DatabaseConnection,
    root: &Path,
    user_id: Uuid,
    request: FileUploadRequest,
) -> Result<files::Model, AppError> {
    let file_name = safe_file_name(&request.attachment.name)
        .ok_or(AppError::ValidationEmptyField {
            field: "attachment.name",
        })?
        .to_string();
    let bytes = request.attachment.file.unwrap_or_default();
    store_file_bytes(
        db,
        root,
        user_id,
        FileWrite {
            id: Uuid::new_v4(),
            category: StorageCategory::File,
            name: &file_name,
            content_type: &request.attachment.content_type,
            bytes: &bytes,
            description: request.description,
            metadata: None,
        },
    )
    .await
}

pub async fn store_file_bytes<C>(
    db: &C,
    root: &Path,
    user_id: Uuid,
    input: FileWrite<'_>,
) -> Result<files::Model, AppError>
where
    C: ConnectionTrait,
{
    store_file_bytes_with(root, user_id, input, |active| active.insert(db)).await
}

async fn store_file_bytes_with<F, Fut>(
    root: &Path,
    user_id: Uuid,
    input: FileWrite<'_>,
    persist: F,
) -> Result<files::Model, AppError>
where
    F: FnOnce(files::ActiveModel) -> Fut,
    Fut: Future<Output = Result<files::Model, DbErr>>,
{
    let local_path = new_file_path(root, user_id, input.category, input.id, input.name)?;
    let parent = local_path
        .parent()
        .ok_or(AppError::ServiceTemporarilyUnavailable)?;
    fs::create_dir_all(parent).await.map_err(|error| {
        eprintln!("file storage directory creation failed: {error}");
        AppError::ServiceTemporarilyUnavailable
    })?;
    write_file_atomically(&local_path, input.bytes).await?;

    let now = Utc::now();
    let active = files::ActiveModel {
        id: Set(input.id),
        user_id: Set(user_id),
        name: Set(input.name.to_string()),
        content_type: Set(input.content_type.to_string()),
        size: Set(input.bytes.len() as i64),
        local_path: Set(local_path.to_string_lossy().into_owned()),
        description: Set(input.description),
        url: Set(None),
        status: Set(FileUploadStatus::Uploaded),
        created_at: Set(now),
        updated_at: Set(now),
        metadata: Set(input.metadata),
    };

    match persist(active).await {
        Ok(model) => Ok(model),
        Err(error) => {
            if let Err(cleanup_error) = fs::remove_file(&local_path).await {
                eprintln!("file storage rollback failed: {cleanup_error}");
            }
            eprintln!("file metadata insert failed: {error}");
            Err(AppError::DbTimeout)
        }
    }
}

async fn write_file_atomically(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    let parent = path
        .parent()
        .ok_or(AppError::ServiceTemporarilyUnavailable)?;
    let temporary = parent.join(format!(".grengin-write-{}.tmp", Uuid::new_v4()));

    if let Err(error) = fs::write(&temporary, bytes).await {
        let _ = fs::remove_file(&temporary).await;
        eprintln!("temporary file storage write failed: {error}");
        return Err(AppError::ServiceTemporarilyUnavailable);
    }
    if let Err(error) = fs::rename(&temporary, path).await {
        let _ = fs::remove_file(&temporary).await;
        eprintln!("file storage commit failed: {error}");
        return Err(AppError::ServiceTemporarilyUnavailable);
    }
    Ok(())
}

pub async fn read_file_for_user(
    db: &DatabaseConnection,
    root: &Path,
    user_id: Uuid,
    file_id: Uuid,
) -> Result<(files::Model, Vec<u8>), AppError> {
    let model = files::Entity::find_by_id(file_id)
        .filter(files::Column::UserId.eq(user_id))
        .filter(files::Column::Status.eq(FileUploadStatus::Uploaded))
        .one(db)
        .await
        .map_err(|error| {
            eprintln!("file metadata lookup failed: {error}");
            AppError::DbTimeout
        })?
        .ok_or(AppError::ResourceNotFound)?;
    let bytes = read_model_bytes(root, &model).await?;
    Ok((model, bytes))
}

pub async fn read_attachments_for_user(
    db: &DatabaseConnection,
    root: &Path,
    user_id: Uuid,
    file_ids: &[Uuid],
) -> Result<HashMap<Uuid, Attachment>, AppError> {
    if file_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let unique_ids = file_ids.iter().copied().collect::<HashSet<_>>();
    let models = files::Entity::find()
        .filter(files::Column::Id.is_in(unique_ids.iter().copied()))
        .filter(files::Column::UserId.eq(user_id))
        .filter(files::Column::Status.eq(FileUploadStatus::Uploaded))
        .all(db)
        .await
        .map_err(|error| {
            eprintln!("file metadata batch lookup failed: {error}");
            AppError::DbTimeout
        })?;
    if models.len() != unique_ids.len() {
        return Err(AppError::ResourceNotFound);
    }

    let mut attachments = HashMap::with_capacity(models.len());
    for model in models {
        let bytes = read_model_bytes(root, &model).await?;
        attachments.insert(
            model.id,
            Attachment {
                file: Some(bytes),
                name: model.name,
                content_type: model.content_type,
            },
        );
    }
    Ok(attachments)
}

pub async fn read_model_bytes(root: &Path, model: &files::Model) -> Result<Vec<u8>, AppError> {
    let canonical_path = contained_stored_path(root, model).await?;
    fs::read(canonical_path).await.map_err(|error| {
        eprintln!("stored file read failed: {error}");
        AppError::ServiceTemporarilyUnavailable
    })
}

pub async fn remove_model_file(root: &Path, model: &files::Model) -> Result<(), AppError> {
    let canonical_path = contained_stored_path(root, model).await?;
    fs::remove_file(&canonical_path).await.map_err(|error| {
        eprintln!("stored file removal failed: {error}");
        AppError::ServiceTemporarilyUnavailable
    })?;
    if let Some(parent) = canonical_path.parent()
        && let Err(error) = fs::remove_dir(parent).await
        && error.kind() != std::io::ErrorKind::DirectoryNotEmpty
    {
        eprintln!("stored file directory cleanup failed: {error}");
    }
    Ok(())
}

async fn contained_stored_path(root: &Path, model: &files::Model) -> Result<PathBuf, AppError> {
    let canonical_root = fs::canonicalize(root).await.map_err(|error| {
        eprintln!("file storage root unavailable: {error}");
        AppError::ServiceTemporarilyUnavailable
    })?;
    let canonical_path = fs::canonicalize(&model.local_path).await.map_err(|error| {
        eprintln!("stored file unavailable: {error}");
        AppError::ResourceNotFound
    })?;
    if !canonical_path.starts_with(&canonical_root) {
        eprintln!("stored file path is outside the configured storage root");
        return Err(AppError::ResourceNotFound);
    }
    Ok(canonical_path)
}

#[cfg(test)]
mod tests {
    use super::{
        FileWrite, StorageCategory, new_file_path, prepare_storage_root, read_model_bytes,
        safe_file_name, store_file_bytes_with, write_file_atomically,
    };
    use crate::models::files::{FileUploadStatus, Model};
    use chrono::Utc;
    use std::path::{Path, PathBuf};
    use tokio::fs;
    use uuid::Uuid;

    #[test]
    fn accepts_leaf_names_only() {
        assert_eq!(safe_file_name("notes.txt"), Some("notes.txt"));
        assert_eq!(safe_file_name(".notes"), Some(".notes"));

        for invalid in [
            "",
            ".",
            "..",
            "../notes.txt",
            "dir/notes.txt",
            "/etc/passwd",
        ] {
            assert_eq!(safe_file_name(invalid), None, "{invalid} must fail");
        }
    }

    #[test]
    fn new_paths_stay_under_the_configured_root() {
        let user_id = Uuid::nil();
        let file_id = Uuid::from_u128(1);
        assert_eq!(
            new_file_path(
                Path::new("/mnt/grengin/files"),
                user_id,
                StorageCategory::File,
                file_id,
                "notes.txt"
            )
            .expect("safe path"),
            PathBuf::from(format!(
                "/mnt/grengin/files/{user_id}/file/{file_id}/notes.txt"
            ))
        );
        assert!(
            new_file_path(
                Path::new("/mnt/grengin/files"),
                user_id,
                StorageCategory::File,
                file_id,
                "../notes.txt"
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn storage_probe_and_legacy_paths_stay_contained() {
        let root = std::env::temp_dir().join(format!("grengin-storage-test-{}", Uuid::new_v4()));
        prepare_storage_root(&root).await.expect("writable root");

        let nested = root.join("legacy/nested/notes.txt");
        fs::create_dir_all(nested.parent().expect("parent"))
            .await
            .expect("legacy directory");
        fs::write(&nested, b"legacy").await.expect("legacy file");
        let model = file_model(nested.to_string_lossy().into_owned());
        assert_eq!(
            read_model_bytes(&root, &model).await.expect("legacy read"),
            b"legacy"
        );

        let outside = std::env::temp_dir().join(format!("grengin-outside-{}", Uuid::new_v4()));
        fs::write(&outside, b"outside").await.expect("outside file");
        assert!(
            read_model_bytes(&root, &file_model(outside.to_string_lossy().into_owned()))
                .await
                .is_err()
        );

        fs::remove_file(outside).await.expect("outside cleanup");
        fs::remove_dir_all(root).await.expect("root cleanup");
    }

    #[tokio::test]
    async fn storage_probe_rejects_a_non_directory_root() {
        let root = std::env::temp_dir().join(format!("grengin-storage-file-{}", Uuid::new_v4()));
        fs::write(&root, b"not a directory")
            .await
            .expect("root file");
        assert!(prepare_storage_root(&root).await.is_err());
        fs::remove_file(root).await.expect("root file cleanup");
    }

    #[tokio::test]
    async fn failed_atomic_commit_removes_the_temporary_file() {
        let root = std::env::temp_dir().join(format!("grengin-atomic-test-{}", Uuid::new_v4()));
        let destination = root.join("destination");
        fs::create_dir_all(&destination)
            .await
            .expect("destination directory");

        assert!(
            write_file_atomically(&destination, b"content")
                .await
                .is_err()
        );
        let mut entries = fs::read_dir(&root).await.expect("root listing");
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.expect("directory entry") {
            names.push(entry.file_name());
        }
        assert_eq!(names, vec![std::ffi::OsString::from("destination")]);

        fs::remove_dir_all(root).await.expect("root cleanup");
    }

    #[tokio::test]
    async fn failed_metadata_insert_removes_the_written_file() {
        let root = std::env::temp_dir().join(format!("grengin-rollback-test-{}", Uuid::new_v4()));
        let user_id = Uuid::new_v4();
        let file_id = Uuid::new_v4();
        let expected = new_file_path(&root, user_id, StorageCategory::File, file_id, "notes.txt")
            .expect("expected path");

        let result = store_file_bytes_with(
            &root,
            user_id,
            FileWrite {
                id: file_id,
                category: StorageCategory::File,
                name: "notes.txt",
                content_type: "text/plain",
                bytes: b"content",
                description: None,
                metadata: None,
            },
            |_| async { Err(sea_orm::DbErr::Custom("injected failure".to_string())) },
        )
        .await;

        assert!(result.is_err());
        assert!(!expected.exists());
        fs::remove_dir_all(root).await.expect("root cleanup");
    }

    fn file_model(local_path: String) -> Model {
        Model {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            name: "../legacy/notes.txt".to_string(),
            content_type: "text/plain".to_string(),
            size: 6,
            local_path,
            description: None,
            url: None,
            status: FileUploadStatus::Uploaded,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            metadata: None,
        }
    }
}
