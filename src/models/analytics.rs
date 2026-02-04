use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "analytics")]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[sea_orm(primary_key, unique, indexed)]
    pub id: Uuid,
    #[sea_orm(column_name = "cache_key")]
    pub cache_key: String,
    #[sea_orm(column_name = "category")]
    pub category: String,
    #[sea_orm(column_name = "range_start")]
    pub range_start: DateTime<Utc>,
    #[sea_orm(column_name = "range_end")]
    pub range_end: DateTime<Utc>,
    #[sea_orm(column_name = "payload")]
    pub payload: serde_json::Value,
    #[sea_orm(column_name = "created_at")]
    pub created_at: DateTime<Utc>,
    #[sea_orm(column_name = "updated_at")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
