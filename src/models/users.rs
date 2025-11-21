use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sea_orm::entity::prelude::*;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq,EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String",db_type = "String(StringLen::None)",rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]   
pub enum UserStatus{
    Active,
    InActive,
    Deleted,
    Suspended,
}

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "users", rename_all="camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
 #[sea_orm(primary_key, unique, indexed)]
  pub id:Uuid,
  pub status:UserStatus,
  pub profile_pic_url:Option<String>,
  #[sea_orm(column_type = "Text", unique, indexed)]
  pub email:String,
  pub name:Option<String>,
  #[sea_orm(indexed)]
  pub org_id:Option<Uuid>, // Internal Organization model uuid
  pub google_id:Option<String>,
  pub azure_id:Option<String>,
  pub created_on:DateTime<Utc>,
  pub updated_on:DateTime<Utc>,
  pub last_login_on:DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::organization::Entity",
        from       = "Column::OrgId",
        to         = "super::organization::Column::Id",
        on_delete  = "SetNull"   // or Cascade/Restrict depending on your design
    )]
    Organization,
}

impl Related<super::organization::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Organization.def()
    }
}

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

