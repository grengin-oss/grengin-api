use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 1) Create table (NO inline .index(...) for Postgres)
        manager
            .create_table(
                Table::create()
                    .table(Organizations::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Organizations::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Organizations::Name)
                            .text()
                            .not_null()
                            .unique_key(), // already creates an index in Postgres
                    )
                    .col(
                        ColumnDef::new(Organizations::SsoProviders)
                            .array(ColumnType::Text)
                            .not_null(),
                    )
                    .col(ColumnDef::new(Organizations::Domain).text().not_null())
                    .col(
                        ColumnDef::new(Organizations::AllowedDomains)
                            .array(ColumnType::Text)
                            .not_null(),
                    )
                    .col(ColumnDef::new(Organizations::LogoUrl).text().null())
                    .col(ColumnDef::new(Organizations::DefaultEngine).text().not_null())
                    .col(ColumnDef::new(Organizations::DefaultModel).text().not_null())
                    .col(
                        ColumnDef::new(Organizations::DataRetentionDays)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Organizations::RequireMfa).boolean().not_null())
                    .col(
                        ColumnDef::new(Organizations::CreatedOn)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::cust("CURRENT_TIMESTAMP")),
                    )
                    .col(
                        ColumnDef::new(Organizations::UpdatedOn)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::cust("CURRENT_TIMESTAMP")),
                    )
                    .to_owned(),
            )
            .await?;

        // 2) Optional: extra index (NOT needed if Name is unique)
        // manager
        //     .create_index(
        //         Index::create()
        //             .name("idx-organizations-name")
        //             .table(Organizations::Table)
        //             .col(Organizations::Name)
        //             .to_owned(),
        //     )
        //     .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // If you created the optional index above, drop it first:
        // manager
        //     .drop_index(
        //         Index::drop()
        //             .name("idx-organizations-name")
        //             .table(Organizations::Table)
        //             .to_owned(),
        //     )
        //     .await?;

        manager
            .drop_table(Table::drop().table(Organizations::Table).to_owned())
            .await?;

        Ok(())
    }
}

#[derive(Iden)]
enum Organizations {
    #[iden = "organizations"]
    Table,

    #[iden = "id"]
    Id,
    #[iden = "name"]
    Name,

    #[iden = "ssoProviders"]
    SsoProviders,
    #[iden = "domain"]
    Domain,
    #[iden = "allowedDomains"]
    AllowedDomains,
    #[iden = "logoUrl"]
    LogoUrl,
    #[iden = "defaultEngine"]
    DefaultEngine,
    #[iden = "defaultModel"]
    DefaultModel,
    #[iden = "dataRetentionDays"]
    DataRetentionDays,
    #[iden = "requireMfa"]
    RequireMfa,
    #[iden = "createdOn"]
    CreatedOn,
    #[iden = "updatedOn"]
    UpdatedOn,
}
