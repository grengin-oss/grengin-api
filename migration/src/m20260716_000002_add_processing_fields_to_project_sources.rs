use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ProjectSources::Table)
                    .add_column(ColumnDef::new(ProjectSources::FileId).uuid().null())
                    .add_column(
                        ColumnDef::new(ProjectSources::ProcessingStatus)
                            .string_len(50)
                            .not_null()
                            .default("pending"),
                    )
                    .add_column(ColumnDef::new(ProjectSources::ProcessingError).text().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ProjectSources::Table)
                    .drop_column(ProjectSources::FileId)
                    .drop_column(ProjectSources::ProcessingStatus)
                    .drop_column(ProjectSources::ProcessingError)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ProjectSources {
    #[sea_orm(iden = "project_sources")]
    Table,
    #[sea_orm(iden = "fileId")]
    FileId,
    #[sea_orm(iden = "processingStatus")]
    ProcessingStatus,
    #[sea_orm(iden = "processingError")]
    ProcessingError,
}
