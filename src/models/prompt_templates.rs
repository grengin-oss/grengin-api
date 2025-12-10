use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "prompt_templates", rename_all="camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model{
   #[sea_orm(primary_key, unique, indexed)]
   pub id: Uuid,
   // Null for system refrence
   pub user_id:Option<Uuid>,
   pub name:String,
   pub model_provider: String,
   pub model_name: String,
   pub created_at:DateTime<Utc>,
   pub updated_at: DateTime<Utc>,
   pub usage_counter:i32,
   pub description:String,
   pub category:String,
   pub prompt_text:String,
   pub public_flag:String,
   pub system_flag_template:String,
    #[sea_orm(column_type = "JsonBinary", nullable)]
   pub metadata: Option<serde_json::Value>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(belongs_to = "super::users::Entity",from = "Column::UserId",to = "super::users::Column::Id")]
    Users,
}

impl Related<super::users::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Users.def()
    }
}
impl ActiveModelBehavior for ActiveModel {}