use chrono::{DateTime, Utc};
use sea_orm::{JsonValue, entity::prelude::*};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "skills", rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[sea_orm(primary_key, unique, indexed)]
    pub id: Uuid,
    #[sea_orm(unique)]
    pub identifier: String,
    pub name: String,
    pub description: Option<String>,
    pub avatar: Option<String>,
    pub system_role: Option<String>,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub tools_config: Option<JsonValue>,
    pub is_builtin: bool,
    pub is_active: bool,
    pub department_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::departments::Entity",
        from = "Column::DepartmentId",
        to = "super::departments::Column::Id"
    )]
    Department,
    #[sea_orm(
        belongs_to = "super::users::Entity",
        from = "Column::UserId",
        to = "super::users::Column::Id"
    )]
    User,
}

impl Related<super::departments::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Department.def()
    }
}

impl Related<super::users::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
