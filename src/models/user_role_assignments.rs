use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "user_role_assignments", rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[sea_orm(primary_key, unique)]
    pub id: Uuid,
    pub user_id: Uuid,
    pub role_id: Uuid,
    pub scope_department_id: Option<Uuid>,
    pub assigned_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::users::Entity",
        from = "Column::UserId",
        to = "super::users::Column::Id"
    )]
    Users,
    #[sea_orm(
        belongs_to = "super::roles::Entity",
        from = "Column::RoleId",
        to = "super::roles::Column::Id"
    )]
    Roles,
    #[sea_orm(
        belongs_to = "super::departments::Entity",
        from = "Column::ScopeDepartmentId",
        to = "super::departments::Column::Id"
    )]
    ScopeDepartments,
    #[sea_orm(
        belongs_to = "super::users::Entity",
        from = "Column::AssignedBy",
        to = "super::users::Column::Id"
    )]
    AssignedBy,
}

impl Related<super::users::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Users.def()
    }
}

impl Related<super::roles::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Roles.def()
    }
}

impl Related<super::departments::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ScopeDepartments.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
