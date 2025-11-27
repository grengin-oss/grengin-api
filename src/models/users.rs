use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sea_orm::entity::prelude::*;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq,EnumIter, DeriveActiveEnum, Serialize, Deserialize,ToSchema)]
#[sea_orm(rs_type = "String",db_type = "String(StringLen::None)",rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]   
pub enum UserStatus{
    Active,
    InActive,
    Deleted,
    Suspended,
}


#[derive(Debug, Clone,Copy, PartialEq, Eq,EnumIter, DeriveActiveEnum, Serialize, Deserialize, ToSchema)]
#[sea_orm(rs_type = "String",db_type = "String(StringLen::None)",rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]   
pub enum UserRole {
    SuperAdmin,
    Admin,
    User,
    Observer,
}

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "users", rename_all="camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
 #[sea_orm(primary_key, unique, indexed)]
  pub id:Uuid,
  pub status:UserStatus,
  pub picture:Option<String>,
  #[sea_orm(column_type = "Text", unique, indexed)]
  pub email:String,
  pub email_verified:bool,
  pub name:Option<String>,
  pub password:Option<String>,
#[sea_orm(column_type = "Text", unique, indexed, nullable)]
  pub google_id:Option<String>,
#[sea_orm(column_type = "Text", unique, indexed, nullable)]
  pub azure_id:Option<String>,
  pub mfa_enabled: bool,
  pub mfa_secret:Option<String>,
  pub created_at:DateTime<Utc>,
  pub updated_at:DateTime<Utc>,
  pub last_login_at:DateTime<Utc>,
  pub password_changed_at:Option<DateTime<Utc>>,
  pub role:UserRole,
  pub hd: Option<String>, //hosted domain of user email
  pub department:Option<String>,
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
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
            last_login_at: Set(Utc::now()),
            ..ActiveModelTrait::default()
        }
    }
}

