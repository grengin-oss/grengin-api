use chrono::{DateTime, Utc};
use sea_orm::{entity::prelude::*, JsonValue};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "mcp_connections", rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[sea_orm(primary_key, unique)]
    pub id: Uuid,
    pub user_id: Uuid,
    pub server_id: Uuid,
    pub server_name: String,
    pub connected: bool,
    pub connected_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub scopes: Option<JsonValue>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub token_type: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
