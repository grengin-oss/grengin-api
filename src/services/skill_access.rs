// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm::{ColumnTrait, Condition, DatabaseConnection};
use uuid::Uuid;

use crate::{auth::error::AuthError, models::skills, services::skills_helpers::get_skill_or_404};

pub fn is_skill_visible_to(skill: &skills::Model, user_id: Uuid) -> bool {
    skill.user_id.is_none_or(|owner_id| owner_id == user_id)
}

pub fn visible_skills_condition(user_id: Option<Uuid>) -> Condition {
    let condition = Condition::any().add(skills::Column::UserId.is_null());
    match user_id {
        Some(user_id) => condition.add(skills::Column::UserId.eq(user_id)),
        None => condition,
    }
}

pub async fn get_visible_skill_or_404(
    id: Uuid,
    user_id: Uuid,
    db: &DatabaseConnection,
) -> Result<skills::Model, AuthError> {
    let skill = get_skill_or_404(id, db).await?;
    if is_skill_visible_to(&skill, user_id) {
        Ok(skill)
    } else {
        Err(AuthError::ResourceNotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::{is_skill_visible_to, visible_skills_condition};
    use crate::models::skills;
    use chrono::Utc;
    use sea_orm::{DbBackend, EntityTrait, QueryFilter, QueryTrait};
    use uuid::Uuid;

    fn skill(department_id: Option<Uuid>, user_id: Option<Uuid>) -> skills::Model {
        let now = Utc::now();
        skills::Model {
            id: Uuid::new_v4(),
            identifier: "skill".to_string(),
            name: "Skill".to_string(),
            description: None,
            avatar: None,
            instructions: None,
            tools_config: None,
            is_builtin: false,
            is_active: true,
            department_id,
            user_id,
            created_at: now,
            updated_at: now,
        }
    }

    fn visible_skills_sql(user_id: Option<Uuid>) -> String {
        skills::Entity::find()
            .filter(visible_skills_condition(user_id))
            .build(DbBackend::Postgres)
            .to_string()
    }

    #[test]
    fn personal_skill_is_visible_to_its_owner() {
        let owner = Uuid::new_v4();
        assert!(is_skill_visible_to(&skill(None, Some(owner)), owner));
    }

    #[test]
    fn personal_skill_is_hidden_from_other_users() {
        assert!(!is_skill_visible_to(
            &skill(None, Some(Uuid::new_v4())),
            Uuid::new_v4()
        ));
    }

    #[test]
    fn org_and_department_skills_stay_visible_to_every_user() {
        let user = Uuid::new_v4();
        assert!(is_skill_visible_to(&skill(None, None), user));
        assert!(is_skill_visible_to(
            &skill(Some(Uuid::new_v4()), None),
            user
        ));
    }

    #[test]
    fn skill_listing_includes_shared_skills_and_only_the_callers_personal_skills() {
        let user = Uuid::new_v4();
        let sql = visible_skills_sql(Some(user));
        assert!(sql.contains(&format!(
            r#""skills"."userId" IS NULL OR "skills"."userId" = '{user}'"#
        )));
    }

    #[test]
    fn unknown_requester_sees_no_personal_skills() {
        let sql = visible_skills_sql(None);
        assert!(sql.contains(r#""skills"."userId" IS NULL"#));
        assert!(!sql.contains(r#""skills"."userId" ="#));
    }
}
