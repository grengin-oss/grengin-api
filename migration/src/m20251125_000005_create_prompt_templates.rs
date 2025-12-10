use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum PromptTemplates {
    #[sea_orm(iden = "prompt_templates")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "userId")]
    UserId,
    #[sea_orm(iden = "name")]
    Name,
    #[sea_orm(iden = "modelProvider")]
    ModelProvider,
    #[sea_orm(iden = "modelName")]
    ModelName,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "updatedAt")]
    UpdatedAt,
    #[sea_orm(iden = "usageCounter")]
    UsageCounter,
    #[sea_orm(iden = "description")]
    Description,
    #[sea_orm(iden = "category")]
    Category,
    #[sea_orm(iden = "promptText")]
    PromptText,
    #[sea_orm(iden = "publicFlag")]
    PublicFlag,
    #[sea_orm(iden = "systemFlagTemplate")]
    SystemFlagTemplate,
    #[sea_orm(iden = "metadata")]
    Metadata,
}

#[derive(DeriveIden)]
enum Users {
    #[sea_orm(iden = "users")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Table
        manager
            .create_table(
                Table::create()
                    .table(PromptTemplates::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PromptTemplates::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(PromptTemplates::UserId).uuid().null())
                    .col(ColumnDef::new(PromptTemplates::Name).string().not_null())
                    .col(ColumnDef::new(PromptTemplates::ModelProvider).string().not_null())
                    .col(ColumnDef::new(PromptTemplates::ModelName).string().not_null())
                    .col(
                        ColumnDef::new(PromptTemplates::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PromptTemplates::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(PromptTemplates::UsageCounter).integer().not_null())
                    .col(ColumnDef::new(PromptTemplates::Description).text().not_null())
                    .col(ColumnDef::new(PromptTemplates::Category).string().not_null())
                    .col(ColumnDef::new(PromptTemplates::PromptText).text().not_null())
                    .col(ColumnDef::new(PromptTemplates::PublicFlag).string().not_null())
                    .col(
                        ColumnDef::new(PromptTemplates::SystemFlagTemplate)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(PromptTemplates::Metadata).json_binary().null())
                    .to_owned(),
            )
            .await?;

        // FK: prompt_templates.userId -> users.id (nullable SET NULL)
        manager
            .create_foreign_key(
                ForeignKey::create()
                    .from(PromptTemplates::Table, PromptTemplates::UserId)
                    .to(Users::Table, Users::Id)
                    .on_delete(ForeignKeyAction::SetNull)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        // Indexes
        manager
            .create_index(
                Index::create()
                    .name("idx_prompt_templates_userId")
                    .table(PromptTemplates::Table)
                    .col(PromptTemplates::UserId)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_prompt_templates_createdAt")
                    .table(PromptTemplates::Table)
                    .col(PromptTemplates::CreatedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PromptTemplates::Table).to_owned())
            .await
    }
}
