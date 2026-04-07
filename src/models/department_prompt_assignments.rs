use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "department_prompt_assignments", rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[sea_orm(primary_key, unique)]
    pub id: Uuid,
    pub department_id: Uuid,
    pub prompt_id: Uuid,
    pub priority: i32,
    pub assigned_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(belongs_to = "super::departments::Entity", from = "Column::DepartmentId", to = "super::departments::Column::Id")]
    Departments,
    #[sea_orm(belongs_to = "super::role_prompts::Entity", from = "Column::PromptId", to = "super::role_prompts::Column::Id")]
    RolePrompts,
    #[sea_orm(belongs_to = "super::users::Entity", from = "Column::AssignedBy", to = "super::users::Column::Id")]
    Users,
}

impl Related<super::departments::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Departments.def()
    }
}

impl Related<super::role_prompts::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::RolePrompts.def()
    }
}

impl Related<super::users::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Users.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
