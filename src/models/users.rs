use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sea_orm::entity::prelude::*;
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq,EnumIter, DeriveActiveEnum, Serialize, Deserialize,ToSchema)]
#[sea_orm(rs_type = "String",db_type = "String(StringLen::None)",rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]   
pub enum UserStatus{
    Active,
    Deactivated,
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
 #[sea_orm(nullable, indexed)]
  pub org_id:Option<Uuid>,
  pub status:UserStatus,
  pub picture:Option<String>,
  #[sea_orm(column_type = "Text", indexed)]
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
  pub hd: Option<String>, //hosted domain of user email/website
  pub department:Option<String>,
  #[sea_orm(column_type = "JsonBinary", nullable)]
  pub metadata:Option<serde_json::Value>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::conversations::Entity")]
    Conversations,
    #[sea_orm(has_many = "super::files::Entity")]
    Files,
    #[sea_orm(has_one = "super::organizations::Entity", belongs_to = "super::organizations::Entity",from = "Column::OrgId",to = "super::organizations::Column::Id")]
    Organizations,
}

impl Related<super::conversations::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Conversations.def()
    }
}

impl Related<super::files::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Files.def()
    }
}

impl Related<super::organizations::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Organizations.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

