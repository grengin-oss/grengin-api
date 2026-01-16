use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sea_orm::{entity::prelude::*};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "departments", rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[sea_orm(primary_key, unique)]
    pub id: Uuid,
    pub name: String,
    pub description:String,
    pub parent_id:Option<Uuid>, // parent department_id as foreign key for hierrachial design
    pub depth:i32,
     #[sea_orm(column_type = "LTree")]
    pub path:String, //Ltree for postgress
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::users::Entity")]
    Users
}

impl Related<super::messages::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Users.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
