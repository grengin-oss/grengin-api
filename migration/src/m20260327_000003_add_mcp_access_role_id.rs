use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.has_table("mcp_access_policies").await?
            && !manager.has_column("mcp_access_policies", "roleId").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("mcp_access_policies"))
                        .add_column(ColumnDef::new(Alias::new("roleId")).uuid().null())
                        .to_owned(),
                )
                .await?;
        }

        if manager.has_table("mcp_access_policies").await? {
            manager
                .get_connection()
                .execute_unprepared(
                    r#"UPDATE "mcp_access_policies" p
                       SET "roleId" = r."id"
                       FROM "roles" r
                       WHERE p."roleId" IS NULL
                         AND p."roleName" IS NOT NULL
                         AND r."name" = p."roleName";"#,
                )
                .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.has_table("mcp_access_policies").await?
            && manager.has_column("mcp_access_policies", "roleId").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("mcp_access_policies"))
                        .drop_column(Alias::new("roleId"))
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }
}
