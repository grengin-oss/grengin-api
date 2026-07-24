use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "project_source_chunks", rename_all = "camelCase")]
pub struct Model {
    #[sea_orm(primary_key, unique)]
    pub id: Uuid,
    pub project_source_id: Uuid,
    pub project_id: Uuid,
    pub chunk_index: i32,
    pub content: String,
    pub provider: String,
    pub model: String,
    pub dimensions: Option<i32>,
    // Stored as pgvector text format "[f1,f2,...]"; String used because SeaORM
    // has no pgvector type — the ::vector cast requires raw SQL on insert/query.
    pub embedding: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::project_sources::Entity",
        from = "Column::ProjectSourceId",
        to = "super::project_sources::Column::Id"
    )]
    ProjectSource,
    #[sea_orm(
        belongs_to = "super::projects::Entity",
        from = "Column::ProjectId",
        to = "super::projects::Column::Id"
    )]
    Project,
}

impl Related<super::project_sources::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ProjectSource.def()
    }
}

impl Related<super::projects::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Project.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
