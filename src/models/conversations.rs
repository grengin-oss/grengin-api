use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "conversations", rename_all="camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model{
   #[sea_orm(primary_key, unique, indexed)]
   pub id: Uuid,
   pub user_id:Uuid,
   pub title:Option<String>,
   pub model_provider: String,
   pub model_name: String,
   pub created_at:DateTime<Utc>,
   pub updated_at: DateTime<Utc>,
   pub last_message_at: Option<DateTime<Utc>>,
   // Total message count
   pub message_count: i32,
   // Total tokens used across all messages in this session.
   pub total_tokens: i32,
    #[sea_orm(column_type = "JsonBinary", nullable)]
   pub metadata: Option<serde_json::Value>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(belongs_to = "super::users::Entity",from = "Column::UserId",to = "super::users::Column::Id")]
    Users,
    #[sea_orm(has_many = "super::messages::Entity")]
    Messages
}

impl Related<super::users::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Users.def()
    }
}

impl Related<super::messages::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Messages.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}