use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sea_orm::entity::prelude::*;
use sea_orm::FromQueryResult;

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
   pub archived_at:Option<DateTime<Utc>>,
   // Total message count
   pub message_count: i32,
   // Total tokens used across all messages in this session.
   pub total_tokens: i64,
   pub total_cost: Decimal,
    #[sea_orm(column_type = "JsonBinary", nullable)]
   pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, FromQueryResult, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationWithCount {
    #[sea_orm(from_alias = "id")]
    pub id: Uuid,
    #[sea_orm(from_alias = "userId")]
    pub user_id: Uuid,
    #[sea_orm(from_alias = "title")]
    pub title: Option<String>,
    #[sea_orm(from_alias = "modelProvider")]
    pub model_provider: String,
    #[sea_orm(from_alias = "modelName")]
    pub model_name: String,
    #[sea_orm(from_alias = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[sea_orm(from_alias = "updatedAt")]
    pub updated_at: DateTime<Utc>,
    #[sea_orm(from_alias = "lastMessageAt")]
    pub last_message_at: Option<DateTime<Utc>>,
    #[sea_orm(from_alias = "archivedAt")]
    pub archived_at: Option<DateTime<Utc>>,
    #[sea_orm(from_alias = "messageCount")]
    pub message_count: i64,
    #[sea_orm(from_alias = "totalTokens")]
    pub total_tokens: i64,
    #[sea_orm(from_alias = "totalCost")]
    pub total_cost: Decimal,
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