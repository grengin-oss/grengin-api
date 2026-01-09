use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "usage_summary_daily", rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[sea_orm(primary_key, unique, indexed)]
    pub id: Uuid,
    #[sea_orm(indexed)]
    pub date: NaiveDate,
    #[sea_orm(indexed)]
    pub user_id: Uuid,
    #[sea_orm(indexed)]
    pub department: Option<String>,
    pub model_provider: String,
    pub model_name: String,
    pub total_requests: i64,
    pub total_tokens: i64,
    pub total_cost: Decimal,
    pub average_latency: Decimal,
    pub success_count: i64,
    pub error_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::users::Entity",
        from = "Column::UserId",
        to = "super::users::Column::Id"
    )]
    Users,
}

impl Related<super::users::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Users.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
