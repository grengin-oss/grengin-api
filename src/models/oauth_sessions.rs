use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "oauth_sessions", rename_all="camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model{
   #[sea_orm(primary_key, unique, indexed)]
   pub state: String,
   pub pkce_verifier: String,
   pub nonce: String,
   pub redirect_uri: Option<String>,
   pub created_at:DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {
       fn new() -> Self {
        use sea_orm::Set;
        Self {
            created_at: Set(Utc::now()),
            ..ActiveModelTrait::default()
        }
    }
}