use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_column("mcp_servers", "icon").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(McpServers::Table)
                        .add_column(ColumnDef::new(McpServers::Icon).string().null())
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.has_column("mcp_servers", "icon").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(McpServers::Table)
                        .drop_column(McpServers::Icon)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[derive(DeriveIden)]
enum McpServers {
    #[sea_orm(iden = "mcp_servers")]
    Table,
    Icon,
}
