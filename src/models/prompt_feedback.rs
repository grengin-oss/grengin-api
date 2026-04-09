use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "prompt_feedback", rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[sea_orm(primary_key, unique)]
    pub id: Uuid,
    pub user_id: Uuid,
    pub prompt_id: Option<Uuid>,
    pub rating: i32,
    pub comment: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(belongs_to = "super::users::Entity", from = "Column::UserId", to = "super::users::Column::Id")]
    Users,
    #[sea_orm(belongs_to = "super::role_prompts::Entity", from = "Column::PromptId", to = "super::role_prompts::Column::Id")]
    RolePrompts,
}

impl Related<super::users::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Users.def()
    }
}

impl Related<super::role_prompts::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::RolePrompts.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
