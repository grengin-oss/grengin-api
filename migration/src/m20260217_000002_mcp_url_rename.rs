use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Rename db_url -> url if the old column exists.
        if manager.has_column("mcp_servers", "db_url").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(McpServers::Table)
                        .rename_column(McpServers::DbUrl, McpServers::Url)
                        .to_owned(),
                )
                .await?;
        } else if !manager.has_column("mcp_servers", "url").await? {
            // If neither exists, add url
            manager
                .alter_table(
                    Table::alter()
                        .table(McpServers::Table)
                        .add_column(ColumnDef::new(McpServers::Url).string().null())
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.has_column("mcp_servers", "url").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(McpServers::Table)
                        .rename_column(McpServers::Url, McpServers::DbUrl)
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
    #[sea_orm(iden = "db_url")]
    DbUrl,
    #[sea_orm(iden = "url")]
    Url,
}
