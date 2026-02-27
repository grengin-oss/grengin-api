use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Permissions::Table)
                    .add_column(
                        ColumnDef::new(Permissions::DescriptionKey)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"UPDATE permissions
SET "descriptionKey" = 'permissions.' || "domain" || '.' || "action" || '.description'
WHERE "descriptionKey" IS NULL OR "descriptionKey" = ''"#,
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Permissions::Table)
                    .drop_column(Permissions::DescriptionKey)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Permissions {
    #[sea_orm(iden = "permissions")]
    Table,
    #[sea_orm(iden = "descriptionKey")]
    DescriptionKey,
}
