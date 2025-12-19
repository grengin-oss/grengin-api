use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sea_orm::entity::prelude::*;
use utoipa::ToSchema;

#[derive(Debug, Clone,Copy, PartialEq, Eq,EnumIter, DeriveActiveEnum, Serialize, Deserialize, ToSchema)]
#[sea_orm(rs_type = "String",db_type = "String(StringLen::None)",rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]   
pub enum FileUploadStatus {
   Uploaded,
   Deleted
}

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "files", rename_all="camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model{
   #[sea_orm(primary_key, unique, indexed)]
   pub id: Uuid,
   pub user_id:Uuid,
   pub name:String,
   pub content_type:String,
   pub size:i64,
   pub local_path:String,
   pub description:Option<String>,
   pub url:Option<String>,
   pub status:FileUploadStatus,
    #[sea_orm(column_type = "JsonBinary", nullable)]
   pub created_at:DateTime<Utc>,
   pub updated_at: DateTime<Utc>,
   pub metadata: Option<serde_json::Value>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(belongs_to = "super::users::Entity",from = "Column::UserId",to = "super::users::Column::Id")]
    Users
}

impl Related<super::users::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Users.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}