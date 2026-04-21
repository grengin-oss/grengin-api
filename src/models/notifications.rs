use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "notifications", rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[sea_orm(primary_key, unique)]
    pub id: Uuid,
    pub user_id: Uuid,
    pub department_id: Option<Uuid>,
    pub kind: String,
    pub title: String,
    pub body: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub payload: serde_json::Value,
    pub period_start: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub read_at: Option<DateTime<Utc>>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::users::Entity",
        from = "Column::UserId",
        to = "super::users::Column::Id"
    )]
    Users,
    #[sea_orm(
        belongs_to = "super::departments::Entity",
        from = "Column::DepartmentId",
        to = "super::departments::Column::Id"
    )]
    Departments,
}

impl Related<super::users::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Users.def()
    }
}

impl Related<super::departments::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Departments.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
