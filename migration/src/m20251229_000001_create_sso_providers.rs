use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(SsoProviders::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SsoProviders::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(SsoProviders::OrgId).uuid().not_null())
                    .col(ColumnDef::new(SsoProviders::Provider).string().not_null())
                    .col(ColumnDef::new(SsoProviders::Name).string().not_null())
                    .col(ColumnDef::new(SsoProviders::TenantId).string().null())
                    .col(ColumnDef::new(SsoProviders::ClientId).string().not_null())
                    .col(ColumnDef::new(SsoProviders::ClientSecret).text().not_null())
                    .col(ColumnDef::new(SsoProviders::IssuerUrl).text().not_null())
                    .col(
                        ColumnDef::new(SsoProviders::AllowedDomains)
                            .array(ColumnType::Text)
                            .not_null()
                            // keep it simple like your reference migration:
                            // if you want a default, do it at app layer
                            // or use a DB default later with cust() if needed.
                    )
                    .col(
                        ColumnDef::new(SsoProviders::IsEnabled)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(SsoProviders::IsDefault)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(SsoProviders::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(SsoProviders::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_sso_providers_org")
                            .from(SsoProviders::Table, SsoProviders::OrgId)
                            .to(Organizations::Table, Organizations::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_sso_providers_org_id")
                    .table(SsoProviders::Table)
                    .col(SsoProviders::OrgId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq_sso_providers_provider")
                    .table(SsoProviders::Table)
                    .col(SsoProviders::Provider)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(Index::drop().name("uq_sso_providers_provider").to_owned())
            .await?;

        manager
            .drop_index(Index::drop().name("idx_sso_providers_org_id").to_owned())
            .await?;

        manager
            .drop_table(Table::drop().table(SsoProviders::Table).to_owned())
            .await?;

        Ok(())
    }
}

#[derive(Iden)]
enum SsoProviders {
    #[iden = "sso_providers"]
    Table,

    #[iden = "id"]
    Id,

    // match rename_all="camelCase"
    #[iden = "orgId"]
    OrgId,

    #[iden = "provider"]
    Provider,

    #[iden = "name"]
    Name,

    #[iden = "tenantId"]
    TenantId,

    #[iden = "clientId"]
    ClientId,

    #[iden = "clientSecret"]
    ClientSecret,

    #[iden = "issuerUrl"]
    IssuerUrl,

    #[iden = "allowedDomains"]
    AllowedDomains,

    #[iden = "isEnabled"]
    IsEnabled,

    #[iden = "isDefault"]
    IsDefault,

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
