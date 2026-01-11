use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // 1. Create branding table
        manager
            .create_table(
                Table::create()
                    .table(Branding::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Branding::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Branding::Name).text().not_null())
                    .col(ColumnDef::new(Branding::LogoUrl).text().null())
                    .col(ColumnDef::new(Branding::ColorPrimary).text().not_null())
                    .col(ColumnDef::new(Branding::ColorAccent).text().not_null())
                    .col(ColumnDef::new(Branding::FontFamily).text().not_null())
                    .col(ColumnDef::new(Branding::CreatedAt).timestamp_with_time_zone().not_null().default(Expr::current_timestamp()))
                    .col(ColumnDef::new(Branding::UpdatedAt).timestamp_with_time_zone().not_null().default(Expr::current_timestamp()))
                    .to_owned(),
            )
            .await?;

        // 2. Migrate data: copy name/logoUrl from organizations if exists, else use defaults
        // Default values:
        //   name: "Grengin"
        //   logo_url: NULL (not set by default)
        //   color_primary: "#4079c5"
        //   color_accent: "#2d906b"
        //   font_family: "Coustard"
        db.execute_unprepared(r#"
            INSERT INTO branding (id, name, "logoUrl", "colorPrimary", "colorAccent", "fontFamily", "createdAt", "updatedAt")
            SELECT
                COALESCE((SELECT id FROM organizations LIMIT 1), gen_random_uuid()),
                COALESCE((SELECT name FROM organizations LIMIT 1), 'Grengin'),
                (SELECT "logoUrl" FROM organizations LIMIT 1),
                '#4079c5',
                '#2d906b',
                'Coustard',
                CURRENT_TIMESTAMP,
                CURRENT_TIMESTAMP
        "#).await?;

        // 3. Drop FK and index from sso_providers, then drop column
        manager.drop_foreign_key(ForeignKey::drop().name("fk_sso_providers_org").table(SsoProviders::Table).to_owned()).await?;
        manager.drop_index(Index::drop().name("idx_sso_providers_org_id").table(SsoProviders::Table).to_owned()).await?;
        manager.alter_table(Table::alter().table(SsoProviders::Table).drop_column(SsoProviders::OrgId).to_owned()).await?;

        // 4. Drop FK and index from ai_engines, then drop column
        manager.drop_foreign_key(ForeignKey::drop().name("fk_ai_engines_orgId").table(AiEngines::Table).to_owned()).await?;
        manager.drop_index(Index::drop().name("idx_ai_engines_orgId").table(AiEngines::Table).to_owned()).await?;
        manager.alter_table(Table::alter().table(AiEngines::Table).drop_column(AiEngines::OrgId).to_owned()).await?;

        // 5. Drop FK and index from users, then drop column
        manager.drop_foreign_key(ForeignKey::drop().name("fk-users-orgId-organizations-id").table(Users::Table).to_owned()).await?;
        manager.drop_index(Index::drop().name("idx-users-orgId").table(Users::Table).to_owned()).await?;
        manager.alter_table(Table::alter().table(Users::Table).drop_column(Users::OrgId).to_owned()).await?;

        // 6. Drop organizations table
        manager.drop_table(Table::drop().table(Organizations::Table).to_owned()).await?;

        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // This migration is irreversible
        Err(DbErr::Migration("This migration cannot be reversed. Restore from backup if needed.".to_string()))
    }
}

#[derive(Iden)]
enum Branding {
    #[iden = "branding"]
    Table,
    #[iden = "id"]
    Id,
    #[iden = "name"]
    Name,
    #[iden = "logoUrl"]
    LogoUrl,
    #[iden = "colorPrimary"]
    ColorPrimary,
    #[iden = "colorAccent"]
    ColorAccent,
    #[iden = "fontFamily"]
    FontFamily,
    #[iden = "createdAt"]
    CreatedAt,
    #[iden = "updatedAt"]
    UpdatedAt,
}

#[derive(Iden)]
enum SsoProviders {
    #[iden = "sso_providers"]
    Table,
    #[iden = "orgId"]
    OrgId,
}

#[derive(Iden)]
enum AiEngines {
    #[iden = "ai_engines"]
    Table,
    #[iden = "orgId"]
    OrgId,
}

#[derive(Iden)]
enum Users {
    #[iden = "users"]
    Table,
    #[iden = "orgId"]
    OrgId,
}

#[derive(Iden)]
enum Organizations {
    #[iden = "organizations"]
    Table,
}
