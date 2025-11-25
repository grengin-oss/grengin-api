use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(OAuthSessions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OAuthSessions::State)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(OAuthSessions::PkceVerifier)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OAuthSessions::Nonce)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OAuthSessions::RedirectTo)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(OAuthSessions::CreatedOn)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(OAuthSessions::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum OAuthSessions {
    #[sea_orm(iden = "oauth_sessions")]
    Table,

    // camelCase column names (because of `rename_all = "camelCase"`)
    #[sea_orm(iden = "state")]
    State,
    #[sea_orm(iden = "pkceVerifier")]
    PkceVerifier,
    #[sea_orm(iden = "nonce")]
    Nonce,
    #[sea_orm(iden = "redirectTo")]
    RedirectTo,
    #[sea_orm(iden = "createdOn")]
    CreatedOn,
}
