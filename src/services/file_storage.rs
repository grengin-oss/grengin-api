// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    dto::files::{Attachment, FileUploadRequest},
    error::AppError,
    models::files::{self, FileUploadStatus},
};
use chrono::Utc;
use openssl::sha::sha256;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, DbErr, EntityTrait, QueryFilter, Statement, TransactionTrait,
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
    pub content_sha256: Option<&'a str>,
    pub description: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

const MAX_FILE_NAME_BYTES: usize = 255;
const UPLOAD_LOCK_SQL: &str = "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))";

pub fn safe_file_name(value: &str) -> Option<&str> {
    if value.trim().is_empty()
        || value.len() > MAX_FILE_NAME_BYTES
        || value.contains(['/', '\\', '\0'])
    {
        return None;
    }
    let mut components = Path::new(value).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Some(value),
        _ => None,
    }
}

fn is_within_root(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).is_ok_and(|relative| {
        relative.components().next().is_some()
            && relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
    })
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
    let path = root
        .join(user_id.to_string())
        .join(category.to_string())
        .join(file_id.to_string())
        .join(file_name);
    if !is_within_root(root, &path) {
        return Err(AppError::ValidationEmptyField { field: "file_name" });
    }
    Ok(path)
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
    let (bytes, content_sha256) = tokio::task::spawn_blocking(move || {
        let content_sha256 = sha256_hex(&bytes);
        (bytes, content_sha256)
    })
    .await
    .map_err(|error| {
        eprintln!("file digest task failed: {error}");
        AppError::ServiceTemporarilyUnavailable
    })?;
    let transaction = db.begin().await.map_err(|error| {
        eprintln!("file upload transaction failed: {error}");
        AppError::DbTimeout
    })?;
    // SeaORM has no typed PostgreSQL advisory-lock operation. This serializes same-user,
    // same-content uploads across API instances until the new file row is committed.
    let lock_key = format!("grengin-upload:{user_id}:{content_sha256}");
    transaction
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            UPLOAD_LOCK_SQL,
            vec![lock_key.into()],
        ))
        .await
        .map_err(|error| {
            eprintln!("file upload lock failed: {error}");
            AppError::DbTimeout
        })?;

    let existing = files::Entity::find()
        .filter(files::Column::UserId.eq(user_id))
        .filter(files::Column::Sha256.eq(&content_sha256))
        .filter(files::Column::Status.eq(FileUploadStatus::Uploaded))
        .all(&transaction)
        .await
        .map_err(|error| {
            eprintln!("file deduplication lookup failed: {error}");
            AppError::DbTimeout
        })?;
    let owner_file_dir = fs::canonicalize(
        root.join(user_id.to_string())
            .join(StorageCategory::File.to_string()),
    )
    .await
    .ok();
    let mut link_source = None;
    for model in existing {
        if let Some(owner_dir) = owner_file_dir.as_deref()
            && let Ok(path) = contained_stored_path(root, &model).await
            && path.starts_with(owner_dir)
        {
            link_source = Some(path);
            break;
        }
    }

    let saved = store_file_bytes_with(
        root,
        user_id,
        FileWrite {
            id: Uuid::new_v4(),
            category: StorageCategory::File,
            name: &file_name,
            content_type: &request.attachment.content_type,
            bytes: &bytes,
            content_sha256: Some(&content_sha256),
            description: request.description,
            metadata: None,
        },
        link_source.as_deref(),
        |active| active.insert(&transaction),
    )
    .await?;
    transaction.commit().await.map_err(|error| {
        // A failed commit can have an unknown outcome; deleting the file here could remove
        // bytes for a row that did commit.
        eprintln!("file upload commit failed: {error}");
        AppError::DbTimeout
    })?;
    Ok(saved)
}

fn sha256_hex(bytes: &[u8]) -> String {
    sha256(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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
    store_file_bytes_with(root, user_id, input, None, |active| active.insert(db)).await
}

async fn store_file_bytes_with<F, Fut>(
    root: &Path,
    user_id: Uuid,
    input: FileWrite<'_>,
    link_source: Option<&Path>,
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
    link_or_write_file(link_source, &local_path, input.bytes).await?;

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
        sha256: Set(input.content_sha256.map(ToOwned::to_owned)),
        status: Set(FileUploadStatus::Uploaded),
        created_at: Set(now),
        updated_at: Set(now),
        metadata: Set(input.metadata),
    };

    match persist(active).await {
        Ok(model) => Ok(model),
        Err(error) => {
            if let Err(cleanup_error) = remove_unpersisted_file(&local_path).await {
                eprintln!("file storage rollback failed: {cleanup_error:?}");
            }
            eprintln!("file metadata insert failed: {error}");
            Err(AppError::DbTimeout)
        }
    }
}

async fn remove_unpersisted_file(path: &Path) -> Result<(), AppError> {
    fs::remove_file(path).await.map_err(|error| {
        eprintln!("unpersisted file cleanup failed: {error}");
        AppError::ServiceTemporarilyUnavailable
    })?;
    if let Some(parent) = path.parent()
        && let Err(error) = fs::remove_dir(parent).await
        && error.kind() != std::io::ErrorKind::DirectoryNotEmpty
    {
        eprintln!("unpersisted file directory cleanup failed: {error}");
    }
    Ok(())
}

async fn link_or_write_file(
    source: Option<&Path>,
    destination: &Path,
    bytes: &[u8],
) -> Result<(), AppError> {
    if let Some(source) = source {
        match fs::hard_link(source, destination).await {
            Ok(()) => return Ok(()),
            Err(error) => eprintln!("file hard link unavailable, storing separate copy: {error}"),
        }
    }
    write_file_atomically(destination, bytes).await
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

pub async fn find_file_for_user(
    db: &DatabaseConnection,
    user_id: Uuid,
    file_id: Uuid,
) -> Result<files::Model, AppError> {
    files::Entity::find_by_id(file_id)
        .filter(files::Column::UserId.eq(user_id))
        .filter(files::Column::Status.eq(FileUploadStatus::Uploaded))
        .one(db)
        .await
        .map_err(|error| {
            eprintln!("file metadata lookup failed: {error}");
            AppError::DbTimeout
        })?
        .ok_or(AppError::ResourceNotFound)
}

pub async fn read_file_for_user(
    db: &DatabaseConnection,
    root: &Path,
    user_id: Uuid,
    file_id: Uuid,
) -> Result<(files::Model, Vec<u8>), AppError> {
    let model = find_file_for_user(db, user_id, file_id).await?;
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
        FileWrite, MAX_FILE_NAME_BYTES, StorageCategory, UPLOAD_LOCK_SQL, is_within_root,
        link_or_write_file, new_file_path, prepare_storage_root, read_model_bytes, safe_file_name,
        sha256_hex, store_file_bytes_with, write_file_atomically,
    };
    use crate::models::files::{ActiveModel, FileUploadStatus, Model};
    use chrono::Utc;
    use std::path::{Path, PathBuf};
    use tokio::fs;
    use uuid::Uuid;

    #[test]
    fn accepts_leaf_names_only() {
        let longest = "a".repeat(MAX_FILE_NAME_BYTES);
        for valid in [
            "notes.txt",
            ".notes",
            "a..b.md",
            "...",
            "report 2024 (final).pdf",
            "résumé.pdf",
            "日本語のメモ.md",
            "C:notes.txt",
            longest.as_str(),
        ] {
            assert_eq!(safe_file_name(valid), Some(valid), "{valid} must pass");
        }
    }

    #[test]
    fn rejects_names_that_could_escape_or_break_the_storage_directory() {
        let too_long = "a".repeat(MAX_FILE_NAME_BYTES + 1);
        let too_long_unicode = "é".repeat(MAX_FILE_NAME_BYTES / 2 + 1);
        for invalid in [
            "",
            "   ",
            ".",
            "..",
            "../notes.txt",
            "../../etc/passwd",
            "dir/notes.txt",
            "/etc/passwd",
            "notes.txt/",
            "notes.txt/.",
            "notes//",
            "..\\..\\windows\\win.ini",
            "dir\\notes.txt",
            "notes\0.txt",
            "\0",
            too_long.as_str(),
            too_long_unicode.as_str(),
        ] {
            assert_eq!(safe_file_name(invalid), None, "{invalid:?} must fail");
        }
    }

    #[test]
    fn containment_accepts_only_plain_descendants_of_the_root() {
        let root = Path::new("/mnt/grengin/files");
        assert!(is_within_root(root, &root.join("user/file/id/notes.txt")));

        for outside in [
            "/mnt/grengin/files",
            "/mnt/grengin/other/notes.txt",
            "/mnt/grengin/files/../other/notes.txt",
            "/mnt/grengin/files/user/../../notes.txt",
            "/etc/passwd",
            "relative/notes.txt",
        ] {
            assert!(
                !is_within_root(root, Path::new(outside)),
                "{outside} must be rejected"
            );
        }
    }

    #[test]
    fn new_paths_reject_names_with_nul_or_backslash() {
        for invalid in ["notes\0.txt", "..\\notes.txt", ""] {
            assert!(
                new_file_path(
                    Path::new("/mnt/grengin/files"),
                    Uuid::nil(),
                    StorageCategory::Skill,
                    Uuid::from_u128(1),
                    invalid
                )
                .is_err(),
                "{invalid:?} must be rejected"
            );
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

    #[test]
    fn content_digest_is_lowercase_sha256() {
        assert_eq!(
            sha256_hex(b"grengin"),
            "a3053fe273bf4f085a250673ab558dace0b88648cc03caa0f180a10e2960cad1"
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
                content_sha256: None,
                description: None,
                metadata: None,
            },
            None,
            |_| async { Err(sea_orm::DbErr::Custom("injected failure".to_string())) },
        )
        .await;

        assert!(result.is_err());
        assert!(!expected.exists());
        fs::remove_dir_all(root).await.expect("root cleanup");
    }

    #[tokio::test]
    async fn repeated_uploads_keep_their_own_rows_and_share_only_bytes() {
        let root = std::env::temp_dir().join(format!("grengin-link-test-{}", Uuid::new_v4()));
        let user_id = Uuid::new_v4();
        let first = store_file_bytes_with(
            &root,
            user_id,
            FileWrite {
                id: Uuid::new_v4(),
                category: StorageCategory::File,
                name: "alpha.txt",
                content_type: "text/plain",
                bytes: b"shared bytes",
                content_sha256: Some("same-digest"),
                description: Some("first description".to_string()),
                metadata: None,
            },
            None,
            |active| async { Ok(model_from_active(active)) },
        )
        .await
        .expect("first upload");
        let second = store_file_bytes_with(
            &root,
            user_id,
            FileWrite {
                id: Uuid::new_v4(),
                category: StorageCategory::File,
                name: "renamed.bin",
                content_type: "application/octet-stream",
                bytes: b"shared bytes",
                content_sha256: Some("same-digest"),
                description: Some("second description".to_string()),
                metadata: None,
            },
            Some(Path::new(&first.local_path)),
            |active| async { Ok(model_from_active(active)) },
        )
        .await
        .expect("second upload");

        assert_ne!(first.id, second.id);
        assert_eq!(first.name, "alpha.txt");
        assert_eq!(second.name, "renamed.bin");
        assert_eq!(second.content_type, "application/octet-stream");
        assert_eq!(second.description.as_deref(), Some("second description"));
        assert_ne!(first.local_path, second.local_path);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let first_meta = fs::metadata(&first.local_path)
                .await
                .expect("first metadata");
            let second_meta = fs::metadata(&second.local_path)
                .await
                .expect("second metadata");
            assert_eq!(
                (first_meta.dev(), first_meta.ino()),
                (second_meta.dev(), second_meta.ino())
            );
        }

        fs::remove_file(&first.local_path)
            .await
            .expect("remove first upload");
        assert_eq!(
            fs::read(&second.local_path).await.expect("second remains"),
            b"shared bytes"
        );
        fs::remove_dir_all(root).await.expect("cleanup");
    }

    #[tokio::test]
    async fn missing_link_source_falls_back_to_an_independent_copy() {
        let root = std::env::temp_dir().join(format!("grengin-link-fallback-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).await.expect("root");
        let destination = root.join("copy.txt");
        link_or_write_file(Some(&root.join("missing.txt")), &destination, b"content")
            .await
            .expect("fallback upload");
        assert_eq!(fs::read(&destination).await.expect("copy"), b"content");
        fs::remove_dir_all(root).await.expect("cleanup");
    }

    #[test]
    fn upload_lock_is_transaction_scoped() {
        assert!(UPLOAD_LOCK_SQL.contains("pg_advisory_xact_lock"));
    }

    #[tokio::test]
    #[ignore = "requires GRENGIN_UPLOAD_TEST_DATABASE_URL pointing at an isolated test database"]
    async fn concurrent_uploads_keep_distinct_rows_and_share_bytes() {
        use super::{find_file_for_user, store_uploaded_file};
        use crate::dto::files::{Attachment, FileUploadRequest};
        use sea_orm::{ColumnTrait, ConnectionTrait, Database, EntityTrait, QueryFilter};

        let url = std::env::var("GRENGIN_UPLOAD_TEST_DATABASE_URL").expect("test database URL");
        assert!(
            url.contains("/grengin_upload_test"),
            "refusing to run upload test outside grengin_upload_test"
        );
        let db = Database::connect(&url)
            .await
            .expect("test database connection");
        db.execute_unprepared(
            r#"CREATE TABLE files (
                id uuid PRIMARY KEY,
                "userId" uuid NOT NULL,
                name text NOT NULL,
                "contentType" text NOT NULL,
                size bigint NOT NULL,
                "localPath" text NOT NULL,
                description text,
                url text,
                sha256 varchar(64),
                status text NOT NULL,
                "createdAt" timestamptz NOT NULL,
                "updatedAt" timestamptz NOT NULL,
                metadata jsonb
            )"#,
        )
        .await
        .expect("test files table");
        db.execute_unprepared(
            r#"CREATE INDEX idx_files_user_sha256_uploaded
               ON files ("userId", sha256)
               WHERE sha256 IS NOT NULL AND status = 'uploaded'"#,
        )
        .await
        .expect("test hash index");

        let root = std::env::temp_dir().join(format!("grengin-upload-db-test-{}", Uuid::new_v4()));
        let owner = Uuid::new_v4();
        let make_request = |name: &str, content_type: &str| FileUploadRequest {
            provider: None,
            description: Some(name.to_string()),
            attachment: Attachment {
                file: Some(b"concurrent content".to_vec()),
                name: name.to_string(),
                content_type: content_type.to_string(),
            },
        };
        let (first, second) = tokio::join!(
            store_uploaded_file(&db, &root, owner, make_request("alpha.txt", "text/plain")),
            store_uploaded_file(
                &db,
                &root,
                owner,
                make_request("renamed.bin", "application/octet-stream")
            )
        );
        let first = first.expect("first upload");
        let second = second.expect("second upload");
        assert_ne!(first.id, second.id);
        assert_ne!(first.local_path, second.local_path);
        assert_eq!(first.name, "alpha.txt");
        assert_eq!(second.name, "renamed.bin");
        assert_eq!(second.content_type, "application/octet-stream");
        assert_eq!(second.description.as_deref(), Some("renamed.bin"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let a = fs::metadata(&first.local_path)
                .await
                .expect("first metadata");
            let b = fs::metadata(&second.local_path)
                .await
                .expect("second metadata");
            assert_eq!((a.dev(), a.ino()), (b.dev(), b.ino()));
        }

        let other = store_uploaded_file(
            &db,
            &root,
            Uuid::new_v4(),
            make_request("other.txt", "text/plain"),
        )
        .await
        .expect("other user upload");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let owner_meta = fs::metadata(&first.local_path)
                .await
                .expect("owner metadata");
            let other_meta = fs::metadata(&other.local_path)
                .await
                .expect("other metadata");
            assert_ne!(
                (owner_meta.dev(), owner_meta.ino()),
                (other_meta.dev(), other_meta.ino())
            );
        }
        assert_eq!(
            crate::models::files::Entity::find()
                .filter(crate::models::files::Column::UserId.eq(owner))
                .all(&db)
                .await
                .expect("owner rows")
                .len(),
            2
        );
        fs::remove_file(&first.local_path)
            .await
            .expect("remove first path");
        let remaining = find_file_for_user(&db, owner, second.id)
            .await
            .expect("independent second row");
        assert_eq!(
            read_model_bytes(&root, &remaining)
                .await
                .expect("second bytes"),
            b"concurrent content"
        );
        db.execute_unprepared("DROP TABLE files")
            .await
            .expect("test table cleanup");
        fs::remove_dir_all(root).await.expect("file cleanup");
    }

    fn model_from_active(active: ActiveModel) -> Model {
        Model {
            id: active.id.unwrap(),
            user_id: active.user_id.unwrap(),
            name: active.name.unwrap(),
            content_type: active.content_type.unwrap(),
            size: active.size.unwrap(),
            local_path: active.local_path.unwrap(),
            description: active.description.unwrap(),
            url: active.url.unwrap(),
            sha256: active.sha256.unwrap(),
            status: active.status.unwrap(),
            created_at: active.created_at.unwrap(),
            updated_at: active.updated_at.unwrap(),
            metadata: active.metadata.unwrap(),
        }
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
            sha256: None,
            status: FileUploadStatus::Uploaded,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            metadata: None,
        }
    }
}
