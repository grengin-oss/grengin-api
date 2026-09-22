// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::utils::zip::ZipArchive;
use base64::prelude::*;
use chrono::Utc;
use futures_util::future::BoxFuture;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection,
    DatabaseTransaction, EntityTrait, IntoActiveModel, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, TransactionTrait,
};
use std::collections::HashMap;
use std::path::Path;
use uuid::Uuid;

use crate::{
    auth::error::AuthError,
    dto::skills::{
        KnowledgeAttachment, SkillCreateRequest, SkillKnowledgeInfo, SkillResponse,
        SkillToolsConfig, SkillUpdateRequest,
    },
    models::{conversation_skills, files, skill_knowledge, skills},
    services::file_storage::{
        FileWrite, StorageCategory, remove_model_file, safe_file_name, store_file_bytes,
    },
};

pub fn skill_to_response(skill: skills::Model) -> SkillResponse {
    skill_to_response_with_knowledge(skill, vec![])
}

pub fn skill_to_response_with_knowledge(
    skill: skills::Model,
    knowledge_files: Vec<SkillKnowledgeInfo>,
) -> SkillResponse {
    let tools_config = skill
        .tools_config
        .as_ref()
        .map(SkillToolsConfig::from_json)
        .unwrap_or_default();
    SkillResponse {
        id: skill.id,
        identifier: skill.identifier,
        name: skill.name,
        description: skill.description,
        avatar: skill.avatar,
        instructions: skill.instructions,
        tools_config,
        is_builtin: skill.is_builtin,
        is_active: skill.is_active,
        department_id: skill.department_id,
        user_id: skill.user_id,
        created_at: skill.created_at,
        updated_at: skill.updated_at,
        knowledge_files,
    }
}

pub async fn get_skill_or_404(
    id: Uuid,
    db: &DatabaseConnection,
) -> Result<skills::Model, AuthError> {
    skills::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("db find skill error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)
}

pub async fn list_skills_query(
    db: &DatabaseConnection,
    department_id: Option<Uuid>,
    is_active: Option<bool>,
    limit: u64,
    offset: u64,
    own_user_id: Option<Uuid>,
) -> Result<(Vec<skills::Model>, u64), AuthError> {
    // Include org/global skills (user_id IS NULL) plus the caller's own personal skills.
    let mut user_filter = sea_orm::Condition::any().add(skills::Column::UserId.is_null());
    if let Some(uid) = own_user_id {
        user_filter = user_filter.add(skills::Column::UserId.eq(uid));
    }
    let mut select = skills::Entity::find().filter(user_filter);

    if let Some(dept_id) = department_id {
        select = select.filter(
            sea_orm::Condition::any()
                .add(skills::Column::DepartmentId.eq(dept_id))
                .add(skills::Column::DepartmentId.is_null()),
        );
    }
    if let Some(active) = is_active {
        select = select.filter(skills::Column::IsActive.eq(active));
    }

    select = select.order_by_asc(skills::Column::Name);

    let total = select.clone().count(db).await.map_err(|e| {
        eprintln!("db skill count error: {e}");
        AuthError::DbTimeout
    })?;
    let rows = select
        .offset(offset)
        .limit(limit)
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("db skill list error: {e}");
            AuthError::DbTimeout
        })?;

    Ok((rows, total))
}

/// Load skills for a stream: builtin skills are always included (auto), plus
/// conversation-linked and transient skill ids. Returns deduplicated active skills.
pub async fn load_skills_for_stream(
    db: &DatabaseConnection,
    conversation_id: Uuid,
    transient_skill_ids: &[Uuid],
) -> Vec<skills::Model> {
    let builtin_ids: Vec<Uuid> = skills::Entity::find()
        .select_only()
        .column(skills::Column::Id)
        .filter(skills::Column::IsBuiltin.eq(true))
        .filter(skills::Column::IsActive.eq(true))
        .into_tuple::<Uuid>()
        .all(db)
        .await
        .unwrap_or_default();

    let linked_ids: Vec<Uuid> = conversation_skills::Entity::find()
        .select_only()
        .column(conversation_skills::Column::SkillId)
        .filter(conversation_skills::Column::ConversationId.eq(conversation_id))
        .into_tuple::<Uuid>()
        .all(db)
        .await
        .unwrap_or_default();

    let mut all_ids: Vec<Uuid> = builtin_ids;
    for id in linked_ids {
        if !all_ids.contains(&id) {
            all_ids.push(id);
        }
    }
    for id in transient_skill_ids {
        if !all_ids.contains(id) {
            all_ids.push(*id);
        }
    }

    skills::Entity::find()
        .filter(skills::Column::Id.is_in(all_ids))
        .filter(skills::Column::IsActive.eq(true))
        .all(db)
        .await
        .unwrap_or_default()
}

/// Fetch all knowledge rows for a set of skill ids and return their inline
/// content keyed by skill_id. Used at stream time to inject knowledge into
/// the system prompt.
pub async fn load_skill_knowledge_for_stream(
    db: &DatabaseConnection,
    skill_ids: &[Uuid],
) -> HashMap<Uuid, String> {
    if skill_ids.is_empty() {
        return HashMap::new();
    }

    let rows = skill_knowledge::Entity::find()
        .filter(skill_knowledge::Column::SkillId.is_in(skill_ids.to_vec()))
        .order_by_asc(skill_knowledge::Column::CreatedAt)
        .all(db)
        .await
        .unwrap_or_default();

    let mut map: HashMap<Uuid, String> = HashMap::new();
    for row in rows {
        let entry = map.entry(row.skill_id).or_default();
        if !entry.is_empty() {
            entry.push_str("\n\n");
        }
        entry.push_str(&format!("### {}\n{}", row.file_name, row.content));
    }
    map
}

/// Return metadata about all knowledge files attached to a skill.
pub async fn get_skill_knowledge_info(
    db: &DatabaseConnection,
    skill_id: Uuid,
) -> Vec<SkillKnowledgeInfo> {
    skill_knowledge::Entity::find()
        .filter(skill_knowledge::Column::SkillId.eq(skill_id))
        .order_by_asc(skill_knowledge::Column::CreatedAt)
        .all(db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| SkillKnowledgeInfo {
            id: r.id,
            file_name: r.file_name,
            char_count: r.char_count,
            storage_mode: r.storage_mode,
            created_at: r.created_at,
        })
        .collect()
}

struct PreparedSkillKnowledge {
    file_name: String,
    content_type: String,
    bytes: Vec<u8>,
    extracted: Vec<(String, String)>,
}

struct PersistedSkillKnowledge {
    knowledge_files: Vec<SkillKnowledgeInfo>,
    stored_file: files::Model,
}

pub async fn create_managed_skill_with_knowledge(
    db: &DatabaseConnection,
    file_storage_root: &Path,
    user_id: Uuid,
    mut request: SkillCreateRequest,
) -> Result<(skills::Model, Vec<SkillKnowledgeInfo>), AuthError> {
    let attachment = request.knowledge_attachment.take();
    let identifier = request.identifier.trim().to_ascii_lowercase();
    if identifier.is_empty() || identifier.len() > 100 {
        return Err(AuthError::InvalidRequest {
            field: "identifier",
        });
    }
    let name = request.name.trim().to_string();
    if name.is_empty() || name.len() > 100 {
        return Err(AuthError::InvalidRequest { field: "name" });
    }

    let conflict = skills::Entity::find()
        .filter(skills::Column::Identifier.eq(&identifier))
        .one(db)
        .await
        .map_err(|error| {
            eprintln!("db skill lookup error: {error}");
            AuthError::DbTimeout
        })?;
    if conflict.is_some() {
        return Err(AuthError::DbConflict);
    }

    let tools_config = request
        .tools_config
        .map(|config| serde_json::to_value(config).unwrap_or_default());
    let now = Utc::now();
    let row = skills::ActiveModel {
        id: Set(Uuid::new_v4()),
        identifier: Set(identifier),
        name: Set(name),
        description: Set(request.description),
        avatar: Set(request.avatar),
        instructions: Set(request.instructions),
        tools_config: Set(tools_config),
        is_builtin: Set(false),
        is_active: Set(true),
        department_id: Set(request.department_id),
        user_id: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    };

    save_skill_with_knowledge(
        db,
        file_storage_root,
        user_id,
        attachment,
        move |transaction| {
            Box::pin(async move {
                row.insert(transaction).await.map_err(|error| {
                    eprintln!("db create skill error: {error}");
                    AuthError::DbTimeout
                })
            })
        },
    )
    .await
}

pub async fn update_managed_skill_with_knowledge(
    db: &DatabaseConnection,
    file_storage_root: &Path,
    skill_id: Uuid,
    user_id: Uuid,
    mut request: SkillUpdateRequest,
) -> Result<(skills::Model, Vec<SkillKnowledgeInfo>), AuthError> {
    let attachment = request.knowledge_attachment.take();
    let replaces_knowledge = attachment.is_some();
    let (skill, knowledge_files) = save_skill_with_knowledge(
        db,
        file_storage_root,
        user_id,
        attachment,
        move |transaction| {
            Box::pin(async move {
                let skill = skills::Entity::find_by_id(skill_id)
                    .one(transaction)
                    .await
                    .map_err(|error| {
                        eprintln!("db find skill error: {error}");
                        AuthError::DbTimeout
                    })?
                    .ok_or(AuthError::ResourceNotFound)?;
                let mut active = skill.into_active_model();
                apply_managed_skill_update(&mut active, request)?;
                active.update(transaction).await.map_err(|error| {
                    eprintln!("db update skill error: {error}");
                    AuthError::DbTimeout
                })
            })
        },
    )
    .await?;

    if replaces_knowledge {
        Ok((skill, knowledge_files))
    } else {
        let knowledge_files = get_skill_knowledge_info(db, skill.id).await;
        Ok((skill, knowledge_files))
    }
}

fn apply_managed_skill_update(
    active: &mut skills::ActiveModel,
    request: SkillUpdateRequest,
) -> Result<(), AuthError> {
    if let Some(name) = request.name {
        let name = name.trim().to_string();
        if name.is_empty() || name.len() > 100 {
            return Err(AuthError::InvalidRequest { field: "name" });
        }
        active.name = Set(name);
    }
    if let Some(description) = request.description {
        active.description = Set(Some(description));
    }
    if let Some(avatar) = request.avatar {
        active.avatar = Set(Some(avatar));
    }
    if let Some(instructions) = request.instructions {
        active.instructions = Set(Some(instructions));
    }
    if let Some(config) = request.tools_config {
        active.tools_config = Set(Some(serde_json::to_value(config).unwrap_or_default()));
    }
    if let Some(is_active) = request.is_active {
        active.is_active = Set(is_active);
    }
    if let Some(department_id) = request.department_id {
        active.department_id = Set(Some(department_id));
    }
    active.updated_at = Set(Utc::now());
    Ok(())
}

pub(crate) async fn save_skill_with_knowledge<F>(
    db: &DatabaseConnection,
    file_storage_root: &Path,
    user_id: Uuid,
    attachment: Option<KnowledgeAttachment>,
    save_skill: F,
) -> Result<(skills::Model, Vec<SkillKnowledgeInfo>), AuthError>
where
    F: for<'a> FnOnce(&'a DatabaseTransaction) -> BoxFuture<'a, Result<skills::Model, AuthError>>,
{
    let prepared = attachment.map(prepare_skill_knowledge).transpose()?;
    let transaction = db.begin().await.map_err(|error| {
        eprintln!("db begin skill transaction error: {error}");
        AuthError::DbTimeout
    })?;

    let skill = match save_skill(&transaction).await {
        Ok(skill) => skill,
        Err(error) => {
            if let Err(rollback_error) = transaction.rollback().await {
                eprintln!("db rollback skill transaction error: {rollback_error}");
            }
            return Err(error);
        }
    };

    let persisted = if let Some(prepared) = prepared {
        match persist_skill_knowledge(&transaction, file_storage_root, skill.id, user_id, prepared)
            .await
        {
            Ok(persisted) => Some(persisted),
            Err(error) => {
                if let Err(rollback_error) = transaction.rollback().await {
                    eprintln!("db rollback skill knowledge error: {rollback_error}");
                }
                return Err(error);
            }
        }
    } else {
        None
    };

    if let Err(error) = transaction.commit().await {
        if let Some(persisted) = &persisted {
            cleanup_skill_file(file_storage_root, &persisted.stored_file).await;
        }
        eprintln!("db commit skill transaction error: {error}");
        return Err(AuthError::DbTimeout);
    }

    Ok((
        skill,
        persisted
            .map(|persisted| persisted.knowledge_files)
            .unwrap_or_default(),
    ))
}

fn prepare_skill_knowledge(
    attachment: KnowledgeAttachment,
) -> Result<PreparedSkillKnowledge, AuthError> {
    let storage_file_name = safe_file_name(&attachment.file_name)
        .ok_or(AuthError::InvalidRequest {
            field: "knowledge_attachment.file_name",
        })?
        .to_string();
    let bytes = BASE64_STANDARD
        .decode(attachment.data.trim())
        .map_err(|_| AuthError::InvalidRequest {
            field: "knowledge_attachment.data",
        })?;

    let extracted = extract_knowledge_text(&bytes, &attachment.content_type, &storage_file_name)
        .map_err(|_| AuthError::InvalidRequest {
            field: "knowledge_attachment",
        })?;

    Ok(PreparedSkillKnowledge {
        file_name: storage_file_name,
        content_type: attachment.content_type,
        bytes,
        extracted,
    })
}

async fn persist_skill_knowledge(
    transaction: &DatabaseTransaction,
    file_storage_root: &Path,
    skill_id: Uuid,
    user_id: Uuid,
    prepared: PreparedSkillKnowledge,
) -> Result<PersistedSkillKnowledge, AuthError> {
    let stored_file = store_file_bytes(
        transaction,
        file_storage_root,
        user_id,
        FileWrite {
            id: Uuid::new_v4(),
            category: StorageCategory::Skill,
            name: &prepared.file_name,
            content_type: &prepared.content_type,
            bytes: &prepared.bytes,
            description: Some("skill knowledge attachment".to_string()),
            metadata: None,
        },
    )
    .await
    .map_err(map_file_storage_error)?;

    let result =
        replace_skill_knowledge_rows(transaction, skill_id, stored_file.id, prepared.extracted)
            .await;
    match result {
        Ok(knowledge_files) => Ok(PersistedSkillKnowledge {
            knowledge_files,
            stored_file,
        }),
        Err(error) => {
            cleanup_skill_file(file_storage_root, &stored_file).await;
            Err(error)
        }
    }
}

async fn replace_skill_knowledge_rows<C>(
    db: &C,
    skill_id: Uuid,
    file_id: Uuid,
    extracted: Vec<(String, String)>,
) -> Result<Vec<SkillKnowledgeInfo>, AuthError>
where
    C: ConnectionTrait,
{
    skill_knowledge::Entity::delete_many()
        .filter(skill_knowledge::Column::SkillId.eq(skill_id))
        .exec(db)
        .await
        .map_err(|e| {
            eprintln!("db delete skill knowledge error: {e}");
            AuthError::DbTimeout
        })?;

    let now = Utc::now();
    let mut inserted = Vec::with_capacity(extracted.len());

    for (file_name, content) in extracted {
        let char_count = content.chars().count() as i32;
        let row = skill_knowledge::ActiveModel {
            id: Set(Uuid::new_v4()),
            skill_id: Set(skill_id),
            file_id: Set(Some(file_id)),
            file_name: Set(file_name.clone()),
            content: Set(content),
            char_count: Set(char_count),
            storage_mode: Set("inline".to_string()),
            created_at: Set(now),
        };
        let saved = row.insert(db).await.map_err(|e| {
            eprintln!("db insert skill knowledge error: {e}");
            AuthError::DbTimeout
        })?;
        inserted.push(SkillKnowledgeInfo {
            id: saved.id,
            file_name: saved.file_name,
            char_count: saved.char_count,
            storage_mode: saved.storage_mode,
            created_at: saved.created_at,
        });
    }

    Ok(inserted)
}

fn map_file_storage_error(error: crate::error::AppError) -> AuthError {
    match error {
        crate::error::AppError::DbUnavailable | crate::error::AppError::DbTimeout => {
            AuthError::DbTimeout
        }
        crate::error::AppError::ValidationMissingField { .. }
        | crate::error::AppError::ValidationEmptyField { .. } => AuthError::InvalidRequest {
            field: "knowledge_attachment.file_name",
        },
        _ => AuthError::ServiceTemporarilyUnavailable,
    }
}

async fn cleanup_skill_file(file_storage_root: &Path, stored_file: &files::Model) {
    if let Err(error) = remove_model_file(file_storage_root, stored_file).await {
        eprintln!("skill knowledge file rollback failed: {error:?}");
    }
}

/// Extract (file_name, text_content) pairs from raw bytes.
/// Single .md → one pair. ZIP → one pair per .md file inside.
fn extract_knowledge_text(
    bytes: &[u8],
    content_type: &str,
    file_name: &str,
) -> Result<Vec<(String, String)>, String> {
    let mime = content_type.to_lowercase();
    if mime == "application/zip" || file_name.ends_with(".zip") {
        return extract_zip_markdown(bytes);
    }
    // Treat everything else as plain text / markdown.
    let text = String::from_utf8_lossy(bytes).trim().to_string();
    if text.is_empty() {
        return Err("empty file".to_string());
    }
    Ok(vec![(file_name.to_string(), text)])
}

fn extract_zip_markdown(bytes: &[u8]) -> Result<Vec<(String, String)>, String> {
    let archive = ZipArchive::new(bytes).map_err(|e| e.to_string())?;
    let mut results = Vec::new();

    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().to_string();
        if entry.is_dir() || !name.ends_with(".md") {
            continue;
        }
        let mut buf = String::new();
        entry.read_to_string(&mut buf).map_err(|e| e.to_string())?;
        let text = buf.trim().to_string();
        if !text.is_empty() {
            // Use only the filename component, not the full path inside the zip.
            let short_name = name.rsplit('/').next().unwrap_or(&name).to_string();
            results.push((short_name, text));
        }
    }

    if results.is_empty() {
        return Err("zip contained no .md files".to_string());
    }
    Ok(results)
}

pub async fn link_skill_to_conversation(
    db: &DatabaseConnection,
    conversation_id: Uuid,
    skill_id: Uuid,
) -> Result<conversation_skills::Model, AuthError> {
    let existing = conversation_skills::Entity::find()
        .filter(conversation_skills::Column::ConversationId.eq(conversation_id))
        .filter(conversation_skills::Column::SkillId.eq(skill_id))
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("db conversation_skills check error: {e}");
            AuthError::DbTimeout
        })?;

    if let Some(row) = existing {
        return Ok(row);
    }

    let row = conversation_skills::ActiveModel {
        id: Set(Uuid::new_v4()),
        conversation_id: Set(conversation_id),
        skill_id: Set(skill_id),
        created_at: Set(Utc::now()),
    };

    row.insert(db).await.map_err(|e| {
        eprintln!("db link skill error: {e}");
        AuthError::DbTimeout
    })
}

pub async fn unlink_skill_from_conversation(
    db: &DatabaseConnection,
    conversation_id: Uuid,
    skill_id: Uuid,
) -> Result<(), AuthError> {
    use sea_orm::ModelTrait;

    let row = conversation_skills::Entity::find()
        .filter(conversation_skills::Column::ConversationId.eq(conversation_id))
        .filter(conversation_skills::Column::SkillId.eq(skill_id))
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("db find conversation_skill error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    row.delete(db).await.map_err(|e| {
        eprintln!("db unlink skill error: {e}");
        AuthError::DbTimeout
    })?;

    Ok(())
}

pub async fn list_conversation_skills(
    db: &DatabaseConnection,
    conversation_id: Uuid,
) -> Result<Vec<(conversation_skills::Model, skills::Model)>, AuthError> {
    let links = conversation_skills::Entity::find()
        .filter(conversation_skills::Column::ConversationId.eq(conversation_id))
        .order_by_asc(conversation_skills::Column::CreatedAt)
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("db list conversation skills error: {e}");
            AuthError::DbTimeout
        })?;

    let skill_ids: Vec<Uuid> = links.iter().map(|l| l.skill_id).collect();
    let skill_map: HashMap<Uuid, skills::Model> = skills::Entity::find()
        .filter(skills::Column::Id.is_in(skill_ids))
        .all(db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|s| (s.id, s))
        .collect();

    let pairs = links
        .into_iter()
        .filter_map(|link| {
            let skill = skill_map.get(&link.skill_id)?.clone();
            Some((link, skill))
        })
        .collect();

    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::{save_skill_with_knowledge, skills};
    use crate::{auth::error::AuthError, dto::skills::KnowledgeAttachment};
    use chrono::Utc;
    use sea_orm::DatabaseConnection;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use uuid::Uuid;

    #[tokio::test]
    async fn invalid_knowledge_is_rejected_before_the_skill_transaction() {
        let db = DatabaseConnection::Disconnected;
        let save_called = Arc::new(AtomicBool::new(false));
        let called = save_called.clone();
        let result = save_skill_with_knowledge(
            &db,
            std::path::Path::new("/unused"),
            Uuid::new_v4(),
            Some(knowledge_attachment("../unsafe.md")),
            move |_| {
                Box::pin(async move {
                    called.store(true, Ordering::SeqCst);
                    Ok(skill_model(Uuid::new_v4()))
                })
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(AuthError::InvalidRequest {
                field: "knowledge_attachment.file_name"
            })
        ));
        assert!(!save_called.load(Ordering::SeqCst));
    }

    fn knowledge_attachment(file_name: &str) -> KnowledgeAttachment {
        KnowledgeAttachment {
            file_name: file_name.to_string(),
            content_type: "text/markdown".to_string(),
            data: "aGVsbG8=".to_string(),
        }
    }

    fn skill_model(id: Uuid) -> skills::Model {
        let now = Utc::now();
        skills::Model {
            id,
            identifier: format!("skill-{id}"),
            name: "Skill".to_string(),
            description: None,
            avatar: None,
            instructions: None,
            tools_config: None,
            is_builtin: false,
            is_active: true,
            department_id: None,
            user_id: None,
            created_at: now,
            updated_at: now,
        }
    }
}
