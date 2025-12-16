use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "organizations", rename_all="camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
 #[sea_orm(primary_key, unique, indexed)]
   pub id:Uuid,
   pub name:String,
   pub sso_providers:Vec<String>,
   pub domain:String,
   pub allowed_domains:Vec<String>,
   pub logo_url:Option<String>,
   pub default_engine:String,
   pub default_model:String,
   pub data_retention_days:i64,
   pub require_mfa: bool,
   pub created_on:DateTime<Utc>,
   pub updated_on:DateTime<Utc>
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::users::Entity")]
    Users,
}

impl Related<super::users::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Users.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}