// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use base64::prelude::*;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
};
use std::collections::HashMap;
use std::io::{Cursor, Read};
use uuid::Uuid;

use crate::{
    auth::error::AuthError,
    dto::skills::{KnowledgeAttachment, SkillKnowledgeInfo, SkillResponse, SkillToolsConfig},
    models::{conversation_skills, files, files::FileUploadStatus, skill_knowledge, skills},
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

const FILE_STORAGE_ROOT: &str = "/data/files";

/// Decode and store a knowledge attachment for a skill, replacing any existing
/// knowledge rows for that skill. The raw file is persisted to disk and recorded
/// in the `files` table as a backup. Supports `text/markdown` (single .md) and
/// `application/zip` (multiple .md files). Returns stored knowledge info on success.
pub async fn process_skill_knowledge(
    db: &DatabaseConnection,
    skill_id: Uuid,
    user_id: Uuid,
    attachment: KnowledgeAttachment,
) -> Result<Vec<SkillKnowledgeInfo>, AuthError> {
    let bytes = BASE64_STANDARD
        .decode(attachment.data.trim())
        .map_err(|_| AuthError::InvalidRequest {
            field: "knowledge_attachment.data",
        })?;

    let extracted = extract_knowledge_text(&bytes, &attachment.content_type, &attachment.file_name)
        .map_err(|_| AuthError::InvalidRequest {
            field: "knowledge_attachment",
        })?;

    // Persist the raw upload to disk and record it in the files table.
    let file_id = save_knowledge_file_to_disk(db, user_id, &bytes, &attachment).await;

    // Replace all existing knowledge for this skill before inserting new rows.
    skill_knowledge::Entity::delete_many()
        .filter(skill_knowledge::Column::SkillId.eq(skill_id))
        .exec(db)
        .await
        .map_err(|e| {
            eprintln!("db delete skill knowledge error: {e}");
            AuthError::DbTimeout
        })?;

    let now = Utc::now();
    let mut inserted: Vec<SkillKnowledgeInfo> = Vec::new();

    for (file_name, content) in extracted {
        let char_count = content.chars().count() as i32;
        let row = skill_knowledge::ActiveModel {
            id: Set(Uuid::new_v4()),
            skill_id: Set(skill_id),
            file_id: Set(file_id),
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

/// Write the raw file bytes to `/data/files/{user_id}/skill/{file_id}/{name}` and
/// insert a row into the `files` table. Returns the file id on success, None on
/// any I/O or DB error (non-fatal — knowledge text extraction still proceeds).
async fn save_knowledge_file_to_disk(
    db: &DatabaseConnection,
    user_id: Uuid,
    bytes: &[u8],
    attachment: &KnowledgeAttachment,
) -> Option<Uuid> {
    let file_id = Uuid::new_v4();
    let folder = format!("{}/{}/skill/{}", FILE_STORAGE_ROOT, user_id, file_id);
    if let Err(e) = tokio::fs::create_dir_all(&folder).await {
        eprintln!("skill knowledge dir create error: {e}");
        return None;
    }
    let local_path = format!("{}/{}", folder, attachment.file_name);
    if let Err(e) = tokio::fs::write(&local_path, bytes).await {
        eprintln!("skill knowledge file write error: {e}");
        return None;
    }

    let now = Utc::now();
    let row = files::ActiveModel {
        id: Set(file_id),
        user_id: Set(user_id),
        name: Set(attachment.file_name.clone()),
        content_type: Set(attachment.content_type.clone()),
        size: Set(bytes.len() as i64),
        local_path: Set(local_path),
        description: Set(Some("skill knowledge attachment".to_string())),
        url: Set(None),
        status: Set(FileUploadStatus::Uploaded),
        created_at: Set(now),
        updated_at: Set(now),
        metadata: Set(None),
    };

    match row.insert(db).await {
        Ok(saved) => Some(saved.id),
        Err(e) => {
            eprintln!("db insert skill knowledge file record error: {e}");
            None
        }
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
    let cursor = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| e.to_string())?;
    let mut results = Vec::new();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
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
