use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AiEngines::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(AiEngines::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(AiEngines::OrgId).uuid().not_null())
                    .col(ColumnDef::new(AiEngines::DisplayName).text().not_null())
                    .col(ColumnDef::new(AiEngines::IsEnabled).boolean().not_null())
                    .col(ColumnDef::new(AiEngines::EngineKey).text().not_null())
                    // ApiKeyStatus is stored as string in your entity (db_type = String)
                    .col(ColumnDef::new(AiEngines::ApiKeyStatus).text().not_null())
                    .col(ColumnDef::new(AiEngines::ApiKey).text().null())
                    // Postgres TEXT[] for Vec<String>
                    .col(
                        ColumnDef::new(AiEngines::WhitelistModels)
                            .array(ColumnType::Text)
                            .not_null()
                            .default(Expr::cust("'{}'::text[]")),
                    )
                    .col(ColumnDef::new(AiEngines::DefaultModel).text().not_null())
                    .col(
                        ColumnDef::new(AiEngines::ApiKeyValidatedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(AiEngines::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(AiEngines::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_ai_engines_orgId")
                            .from(AiEngines::Table, AiEngines::OrgId)
                            .to(Organizations::Table, Organizations::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // `#[sea_orm(indexed)] pub engine_key: String`
        manager
            .create_index(
                Index::create()
                    .name("idx_ai_engines_engineKey")
                    .table(AiEngines::Table)
                    .col(AiEngines::EngineKey)
                    .to_owned(),
            )
            .await?;

        // optional but usually helpful for joins
        manager
            .create_index(
                Index::create()
                    .name("idx_ai_engines_orgId")
                    .table(AiEngines::Table)
                    .col(AiEngines::OrgId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_ai_engines_engineKey")
                    .table(AiEngines::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_ai_engines_orgId")
                    .table(AiEngines::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_table(Table::drop().table(AiEngines::Table).if_exists().to_owned())
            .await?;

        Ok(())
    }
}

// Table/column idents match your entity's `rename_all="camelCase"`
#[derive(Iden)]
enum AiEngines {
    #[iden = "ai_engines"]
    Table,

    #[iden = "id"]
    Id,

    #[iden = "orgId"]
    OrgId,

    #[iden = "displayName"]
    DisplayName,

    #[iden = "isEnabled"]
    IsEnabled,

    #[iden = "engineKey"]
    EngineKey,

    #[iden = "apiKeyStatus"]
    ApiKeyStatus,

    #[iden = "apiKey"]
    ApiKey,

    #[iden = "whitelistModels"]
    WhitelistModels,

    #[iden = "defaultModel"]
    DefaultModel,

    #[iden = "apiKeyValidatedAt"]
    ApiKeyValidatedAt,

    #[iden = "createdAt"]
    CreatedAt,

    #[iden = "updatedAt"]
    UpdatedAt,
}

#[derive(Iden)]
enum Organizations {
    #[iden = "organizations"]
    Table,
    #[iden = "id"]
    Id,
}