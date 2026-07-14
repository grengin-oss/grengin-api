use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    PaginatorTrait,
};
use uuid::Uuid;

use crate::{
    auth::error::AuthError,
    dto::skills::{SkillResponse, SkillToolsConfig},
    models::{conversation_skills, skills},
};

pub fn skill_to_response(skill: skills::Model) -> SkillResponse {
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
        system_role: skill.system_role,
        tools_config,
        is_builtin: skill.is_builtin,
        is_active: skill.is_active,
        department_id: skill.department_id,
        created_at: skill.created_at,
        updated_at: skill.updated_at,
    }
}

pub async fn get_skill_or_404(id: Uuid, db: &DatabaseConnection) -> Result<skills::Model, AuthError> {
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
) -> Result<(Vec<skills::Model>, u64), AuthError> {
    let mut select = skills::Entity::find();

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
    let rows = select.offset(offset).limit(limit).all(db).await.map_err(|e| {
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

pub async fn link_skill_to_conversation(
    db: &DatabaseConnection,
    conversation_id: Uuid,
    skill_id: Uuid,
) -> Result<conversation_skills::Model, AuthError> {
    use chrono::Utc;
    use sea_orm::{ActiveModelTrait, ActiveValue::Set};

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
    let skill_map: std::collections::HashMap<Uuid, skills::Model> = skills::Entity::find()
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
