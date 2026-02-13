use serde::{Deserialize, Serialize};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "role_permissions", rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub role_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub permission_id: Uuid,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(belongs_to = "super::roles::Entity", from = "Column::RoleId", to = "super::roles::Column::Id")]
    Roles,
    #[sea_orm(belongs_to = "super::permissions::Entity", from = "Column::PermissionId", to = "super::permissions::Column::Id")]
    Permissions,
}

impl Related<super::roles::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Roles.def()
    }
}

impl Related<super::permissions::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Permissions.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
