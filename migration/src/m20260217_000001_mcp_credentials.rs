// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.has_column("mcp_servers", "organization_id").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(McpServers::Table)
                        .drop_column(McpServers::OrganizationId)
                        .to_owned(),
                )
                .await?;
        }

        if !manager.has_column("mcp_servers", "client_id").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(McpServers::Table)
                        .add_column(ColumnDef::new(McpServers::ClientId).string().null())
                        .to_owned(),
                )
                .await?;
        }

        if !manager.has_column("mcp_servers", "client_secret").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(McpServers::Table)
                        .add_column(ColumnDef::new(McpServers::ClientSecret).text().null())
                        .to_owned(),
                )
                .await?;
        }

        if !manager.has_column("mcp_servers", "db_url").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(McpServers::Table)
                        .add_column(ColumnDef::new(McpServers::DbUrl).string().null())
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(McpServers::Table)
                    .drop_column(McpServers::ClientId)
                    .drop_column(McpServers::ClientSecret)
                    .drop_column(McpServers::DbUrl)
                    .to_owned(),
            )
            .await?;

        // OrganizationId intentionally not re-added; keeping down symmetrical to column removals.
        Ok(())
    }
}

#[derive(DeriveIden)]
enum McpServers {
    #[sea_orm(iden = "mcp_servers")]
    Table,
    OrganizationId,
    ClientId,
    ClientSecret,
    DbUrl,
}
