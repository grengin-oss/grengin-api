use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "usage_logs", rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[sea_orm(primary_key, unique, indexed)]
    pub id: Uuid,
    pub identifier: String,
    #[sea_orm(indexed)]
    pub user_id: Uuid,
    #[sea_orm(indexed)]
    pub model_provider: String,
    #[sea_orm(indexed)]
    pub model_name: String,
    pub conversation_id: Option<Uuid>,
    pub request_tokens: i64,
    pub response_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: Decimal,
    pub latency_ms: i64,
    #[sea_orm(indexed)]
    pub timestamp: DateTime<Utc>,
    #[sea_orm(indexed)]
    pub department: Option<String>,
    #[sea_orm(indexed)]
    pub status: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub error_message: Option<String>,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub metadata: Option<serde_json::Value>,
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
        belongs_to = "super::conversations::Entity",
        from = "Column::ConversationId",
        to = "super::conversations::Column::Id"
    )]
    Conversations,
}

impl Related<super::users::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Users.def()
    }
}

impl Related<super::conversations::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Conversations.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
