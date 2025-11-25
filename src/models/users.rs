use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sea_orm::entity::prelude::*;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq,EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String",db_type = "String(StringLen::None)",rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]   
pub enum UserStatus{
    Active,
    InActive,
    Deleted,
    Suspended,
}


#[derive(Debug, Clone, PartialEq, Eq,EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String",db_type = "String(StringLen::None)",rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]   
pub enum Roles {
    User,
    Admin,
}

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "users", rename_all="camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
 #[sea_orm(primary_key, unique, indexed)]
  pub id:Uuid,
  pub status:UserStatus,
  pub avatar:Option<String>,
  #[sea_orm(column_type = "Text", unique, indexed)]
  pub email:String,
  pub email_verified:bool,
  pub name:Option<String>,
#[sea_orm(column_type = "Text", unique, indexed, nullable)]
  pub google_id:Option<String>,
#[sea_orm(column_type = "Text", unique, indexed, nullable)]
  pub two_factor_auth:bool,
  pub two_factor_secret:Option<String>,
  pub azure_id:Option<String>,
  pub created_on:DateTime<Utc>,
  pub updated_on:DateTime<Utc>,
  pub last_login_on:DateTime<Utc>,
  #[sea_orm(column_type = "JsonBinary", nullable)]
  pub metadata:Option<serde_json::Value>
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {
    fn new() -> Self {
        use sea_orm::Set;
        Self {
            id: Set(Uuid::new_v4()),
            created_on: Set(Utc::now()),
            updated_on: Set(Utc::now()),
            last_login_on: Set(Utc::now()),
            ..ActiveModelTrait::default()
        }
    }
}

