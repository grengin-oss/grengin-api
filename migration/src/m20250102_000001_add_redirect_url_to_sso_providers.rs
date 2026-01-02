use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(SsoProviders::Table)
                    .add_column(
                        ColumnDef::new(SsoProviders::RedirectUrl)
                            .text()
                            .not_null()
                            .default(""), // safe for existing rows
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(SsoProviders::Table)
                    .drop_column(SsoProviders::RedirectUrl)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(Iden)]
enum SsoProviders {
    #[iden = "sso_providers"]
    Table,
    #[iden = "redirectUrl"]
    RedirectUrl, // <- new field
}


