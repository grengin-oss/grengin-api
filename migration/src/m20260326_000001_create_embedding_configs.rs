use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(EmbeddingConfigs::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(EmbeddingConfigs::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(EmbeddingConfigs::Provider)
                            .string_len(50)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EmbeddingConfigs::Model)
                            .string_len(200)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EmbeddingConfigs::Dimensions)
                            .integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(EmbeddingConfigs::IsEnabled)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(EmbeddingConfigs::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EmbeddingConfigs::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq_embedding_configs_provider_model")
                    .table(EmbeddingConfigs::Table)
                    .col(EmbeddingConfigs::Provider)
                    .col(EmbeddingConfigs::Model)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("uq_embedding_configs_provider_model")
                    .to_owned(),
            )
            .await?;

        manager
            .drop_table(Table::drop().table(EmbeddingConfigs::Table).to_owned())
            .await
    }
}

// Table/column idents match entity rename_all="camelCase"
#[derive(DeriveIden)]
enum EmbeddingConfigs {
    #[sea_orm(iden = "embedding_configs")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "provider")]
    Provider,
    #[sea_orm(iden = "model")]
    Model,
    #[sea_orm(iden = "dimensions")]
    Dimensions,
    #[sea_orm(iden = "isEnabled")]
    IsEnabled,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "updatedAt")]
    UpdatedAt,
}
