use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sea_orm::entity::prelude::*;
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize, ToSchema)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum McpAccessDefault {
    Allow,
    Deny,
}

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "mcp_servers", rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[sea_orm(primary_key, unique)]
    pub id: Uuid,
    pub name: String,
    pub access_default: McpAccessDefault,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::mcp_server_access_rules::Entity")]
    AccessRules,
}

impl Related<super::mcp_server_access_rules::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::AccessRules.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
