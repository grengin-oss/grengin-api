use chrono::{Utc,DateTime};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sso_providers", rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[sea_orm(primary_key,indexed, column_type = "Uuid")]
    pub id: Uuid,
    #[sea_orm(indexed,unique)]
    pub provider: String,
    pub name: String,
    pub tenant_id:Option<String>, // default common for azure
    pub client_id: String,
    pub client_secret: String,
    pub issuer_url: String,
    pub redirect_url:String, // new field add
    pub allowed_domains: Vec<String>,
    pub is_enabled: bool,
    pub is_default: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
