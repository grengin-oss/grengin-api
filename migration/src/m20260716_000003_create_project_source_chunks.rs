use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ProjectSourceChunks::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ProjectSourceChunks::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ProjectSourceChunks::ProjectSourceId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ProjectSourceChunks::ProjectId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ProjectSourceChunks::ChunkIndex)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ProjectSourceChunks::Content)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ProjectSourceChunks::Provider)
                            .string_len(50)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ProjectSourceChunks::Model)
                            .string_len(200)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ProjectSourceChunks::Dimensions)
                            .integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ProjectSourceChunks::Embedding)
                            .custom(Alias::new("vector"))
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ProjectSourceChunks::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"ALTER TABLE "project_source_chunks"
                   ALTER COLUMN "embedding" TYPE vector(1536);"#,
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq-project-source-chunks-source-chunk")
                    .table(ProjectSourceChunks::Table)
                    .col(ProjectSourceChunks::ProjectSourceId)
                    .col(ProjectSourceChunks::ChunkIndex)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-project-source-chunks-project")
                    .table(ProjectSourceChunks::Table)
                    .col(ProjectSourceChunks::ProjectId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-project-source-chunks-source")
                    .from(
                        ProjectSourceChunks::Table,
                        ProjectSourceChunks::ProjectSourceId,
                    )
                    .to(ProjectSources::Table, ProjectSources::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-project-source-chunks-project")
                    .from(ProjectSourceChunks::Table, ProjectSourceChunks::ProjectId)
                    .to(Projects::Table, Projects::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"CREATE INDEX IF NOT EXISTS "idx-project-source-chunks-embedding"
                   ON "project_source_chunks"
                   USING ivfflat ("embedding" vector_cosine_ops);"#,
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"DROP INDEX IF EXISTS "idx-project-source-chunks-embedding";"#,
            )
            .await?;

        manager
            .drop_table(Table::drop().table(ProjectSourceChunks::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ProjectSourceChunks {
    #[sea_orm(iden = "project_source_chunks")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "projectSourceId")]
    ProjectSourceId,
    #[sea_orm(iden = "projectId")]
    ProjectId,
    #[sea_orm(iden = "chunkIndex")]
    ChunkIndex,
    #[sea_orm(iden = "content")]
    Content,
    #[sea_orm(iden = "provider")]
    Provider,
    #[sea_orm(iden = "model")]
    Model,
    #[sea_orm(iden = "dimensions")]
    Dimensions,
    #[sea_orm(iden = "embedding")]
    Embedding,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
}

#[derive(DeriveIden)]
enum ProjectSources {
    #[sea_orm(iden = "project_sources")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
}

#[derive(DeriveIden)]
enum Projects {
    #[sea_orm(iden = "projects")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
}
