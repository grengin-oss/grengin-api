use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sea_orm::entity::prelude::*;
use utoipa::ToSchema;

#[derive(Debug, Clone,Copy, PartialEq, Eq,EnumIter, DeriveActiveEnum, Serialize, Deserialize, ToSchema)]
#[sea_orm(rs_type = "String",db_type = "String(StringLen::None)",rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]   
pub enum PromptRole {
   User,
   Assistant,
   System
}

#[derive(Clone, Debug, PartialEq, Eq , DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "messages", rename_all="camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model{
   #[sea_orm(primary_key, unique, indexed)]
   pub id: Uuid,
   pub conversation_id:Uuid,
   // Self refrence one to one
   pub previous_message_id:Uuid,
   pub role:PromptRole,
   pub message_content:String,
   pub model_provider: String,
   pub model_name: String,
   pub request_tokens:i32,
   pub response_tokens:i32,
   pub created_at:DateTime<Utc>,
   // Total tokens used across all messages in this session.
   pub total_tokens: i32,
   // Latency in milliseconds
   pub latency:i32,
   // Cost in USD
   pub cost:Decimal,
    #[sea_orm(column_type = "JsonBinary", nullable)]
   pub metadata: Option<serde_json::Value>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(belongs_to = "super::conversations::Entity",from = "Column::ConversationId",to = "super::conversations::Column::Id")]
    Conversations,
    #[sea_orm(has_one = "super::messages::Entity", belongs_to = "super::messages::Entity",from = "Column::PreviousMessageId",to = "super::messages::Column::Id")]
    Messages,
}

impl Related<super::conversations::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Conversations.def()
    }
}

impl Related<super::messages::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Messages.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}