use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
};
use uuid::Uuid;

use crate::{
    auth::error::AuthError,
    dto::skills::{SkillResponse, UserSkillCreateRequest, UserSkillUpdateRequest},
    models::skills,
    services::skills_helpers::skill_to_response,
};

pub async fn list_user_skills(
    db: &DatabaseConnection,
    user_id: Uuid,
    is_active: Option<bool>,
    limit: u64,
    offset: u64,
) -> Result<(Vec<skills::Model>, u64), AuthError> {
    let mut select = skills::Entity::find().filter(skills::Column::UserId.eq(user_id));
    if let Some(active) = is_active {
        select = select.filter(skills::Column::IsActive.eq(active));
    }
    select = select.order_by_asc(skills::Column::Name);

    let total = select.clone().count(db).await.map_err(|e| {
        eprintln!("db user skill count error: {e}");
        AuthError::DbTimeout
    })?;
    let rows = select.offset(offset).limit(limit).all(db).await.map_err(|e| {
        eprintln!("db user skill list error: {e}");
        AuthError::DbTimeout
    })?;

    Ok((rows, total))
}

pub async fn get_user_skill_or_404(
    id: Uuid,
    user_id: Uuid,
    db: &DatabaseConnection,
) -> Result<skills::Model, AuthError> {
    skills::Entity::find_by_id(id)
        .filter(skills::Column::UserId.eq(user_id))
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("db find user skill error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)
}

/// Creates a personal skill. The caller is responsible for processing any
/// `knowledge_attachment` from the request after this returns.
pub async fn create_user_skill(
    db: &DatabaseConnection,
    user_id: Uuid,
    req: UserSkillCreateRequest,
) -> Result<skills::Model, AuthError> {
    let name = req.name.trim().to_string();
    if name.is_empty() || name.len() > 100 {
        return Err(AuthError::InvalidRequest { field: "name" });
    }

    let identifier = format!("user-{user_id}-{}", Uuid::new_v4().simple());
    let tools_json = req.tools_config.map(|c| serde_json::to_value(c).unwrap_or_default());

    let now = Utc::now();
    let row = skills::ActiveModel {
        id: Set(Uuid::new_v4()),
        identifier: Set(identifier),
        name: Set(name),
        description: Set(req.description),
        avatar: Set(req.avatar),
        system_role: Set(req.system_role),
        tools_config: Set(tools_json),
        is_builtin: Set(false),
        is_active: Set(true),
        department_id: Set(None),
        user_id: Set(Some(user_id)),
        created_at: Set(now),
        updated_at: Set(now),
    };

    row.insert(db).await.map_err(|e| {
        eprintln!("db create user skill error: {e}");
        AuthError::DbTimeout
    })
}

/// Updates a personal skill. The caller is responsible for processing any
/// `knowledge_attachment` from the request after this returns.
pub async fn update_user_skill(
    db: &DatabaseConnection,
    id: Uuid,
    user_id: Uuid,
    req: UserSkillUpdateRequest,
) -> Result<skills::Model, AuthError> {
    let skill = get_user_skill_or_404(id, user_id, db).await?;
    use sea_orm::IntoActiveModel;
    let mut active: skills::ActiveModel = skill.into_active_model();

    if let Some(name) = req.name {
        let name = name.trim().to_string();
        if name.is_empty() || name.len() > 100 {
            return Err(AuthError::InvalidRequest { field: "name" });
        }
        active.name = Set(name);
    }
    if let Some(desc) = req.description {
        active.description = Set(Some(desc));
    }
    if let Some(avatar) = req.avatar {
        active.avatar = Set(Some(avatar));
    }
    if let Some(system_role) = req.system_role {
        active.system_role = Set(Some(system_role));
    }
    if let Some(config) = req.tools_config {
        active.tools_config = Set(Some(serde_json::to_value(config).unwrap_or_default()));
    }
    if let Some(is_active) = req.is_active {
        active.is_active = Set(is_active);
    }
    active.updated_at = Set(Utc::now());

    active.update(db).await.map_err(|e| {
        eprintln!("db update user skill error: {e}");
        AuthError::DbTimeout
    })
}

pub async fn delete_user_skill(
    db: &DatabaseConnection,
    id: Uuid,
    user_id: Uuid,
) -> Result<(), AuthError> {
    let _ = get_user_skill_or_404(id, user_id, db).await?;
    skills::Entity::delete_by_id(id).exec(db).await.map_err(|e| {
        eprintln!("db delete user skill error: {e}");
        AuthError::DbTimeout
    })?;
    Ok(())
}

pub fn user_skill_to_response(skill: skills::Model) -> SkillResponse {
    skill_to_response(skill)
}
