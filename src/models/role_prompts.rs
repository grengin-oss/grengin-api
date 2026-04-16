use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "role_prompts", rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[sea_orm(primary_key, unique)]
    pub id: Uuid,
    pub name: String,
    pub role_id: Uuid,
    pub prompt_text: String,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub variables: Option<serde_json::Value>,
    pub is_system: bool,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub usage_count: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::roles::Entity",
        from = "Column::RoleId",
        to = "super::roles::Column::Id"
    )]
    Roles,
    #[sea_orm(
        belongs_to = "super::users::Entity",
        from = "Column::CreatedBy",
        to = "super::users::Column::Id"
    )]
    Users,
    #[sea_orm(has_many = "super::department_prompt_assignments::Entity")]
    DepartmentPromptAssignments,
    #[sea_orm(has_many = "super::user_prompt_preferences::Entity")]
    UserPromptPreferences,
    #[sea_orm(has_many = "super::prompt_feedback::Entity")]
    PromptFeedback,
}

impl Related<super::roles::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Roles.def()
    }
}

impl Related<super::users::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Users.def()
    }
}

impl Related<super::department_prompt_assignments::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::DepartmentPromptAssignments.def()
    }
}

impl Related<super::user_prompt_preferences::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::UserPromptPreferences.def()
    }
}

impl Related<super::prompt_feedback::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::PromptFeedback.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
