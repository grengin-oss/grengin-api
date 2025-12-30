use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sea_orm::entity::prelude::*;
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq,EnumIter, DeriveActiveEnum, Serialize, Deserialize,ToSchema)]
#[sea_orm(rs_type = "String",db_type = "String(StringLen::None)",rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]   
pub enum ApiKeyStatus{
  Valid,
  Invalid,
  NotValidated,
  NotConfigured,
}

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "ai_engines", rename_all="camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
 #[sea_orm(primary_key, unique, indexed)]
   pub id:Uuid,
   pub org_id:Uuid,
   pub display_name:String,
   pub is_enabled:bool,
 #[sea_orm(indexed)]
   pub engine_key:String,
   pub api_key_status:ApiKeyStatus,
   pub api_key:Option<String>,
   pub whitelist_models:Vec<String>,
   pub default_model:String,
   pub api_key_validated_at:Option<DateTime<Utc>>,
   pub created_at:DateTime<Utc>,
   pub updated_at:DateTime<Utc>
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_one = "super::organizations::Entity", belongs_to = "super::organizations::Entity",from = "Column::OrgId",to = "super::organizations::Column::Id")]
    Organizations
}

impl Related<super::organizations::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Organizations.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}