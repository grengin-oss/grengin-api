use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "ai_engines", rename_all="camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
 #[sea_orm(primary_key, unique, indexed)]
   pub id:Uuid,
   pub org_id:Uuid,
   pub display_name:String,
   pub engine_key:String,
   pub api_key_valid:bool,
   pub api_key_preview:String,
   pub whitelist_models:Vec<String>,
   pub default_model:Option<String>,
   pub created_on:DateTime<Utc>,
   pub updated_on:DateTime<Utc>
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}