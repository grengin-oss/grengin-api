use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.has_table("mcp_servers").await?
            && manager.has_column("mcp_servers", "accessDefault").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("mcp_servers"))
                        .drop_column(Alias::new("accessDefault"))
                        .to_owned(),
                )
                .await?;
        }

        if manager.has_table("mcp_server_access_rules").await? {
            manager
                .drop_table(
                    Table::drop()
                        .table(Alias::new("mcp_server_access_rules"))
                        .if_exists()
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.has_table("mcp_servers").await?
            && !manager.has_column("mcp_servers", "accessDefault").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("mcp_servers"))
                        .add_column(
                            ColumnDef::new(Alias::new("accessDefault"))
                                .string()
                                .not_null()
                                .default("deny"),
                        )
                        .to_owned(),
                )
                .await?;
        }

        if !manager.has_table("mcp_server_access_rules").await? {
            manager
                .create_table(
                    Table::create()
                        .table(Alias::new("mcp_server_access_rules"))
                        .if_not_exists()
                        .col(
                            ColumnDef::new(Alias::new("id"))
                                .uuid()
                                .not_null()
                                .primary_key(),
                        )
                        .col(ColumnDef::new(Alias::new("serverId")).uuid().not_null())
                        .col(
                            ColumnDef::new(Alias::new("subjectType"))
                                .string()
                                .not_null(),
                        )
                        .col(ColumnDef::new(Alias::new("subjectId")).uuid().not_null())
                        .col(ColumnDef::new(Alias::new("ruleType")).string().not_null())
                        .col(ColumnDef::new(Alias::new("createdBy")).uuid().not_null())
                        .col(
                            ColumnDef::new(Alias::new("createdAt"))
                                .timestamp_with_time_zone()
                                .not_null()
                                .default(Expr::current_timestamp()),
                        )
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }
}
