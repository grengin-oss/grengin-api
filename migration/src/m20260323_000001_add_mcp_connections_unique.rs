// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, Statement};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Remove duplicates before adding a unique index.
        let backend = manager.get_database_backend();
        let dedupe_sql = r#"
            DELETE FROM "mcp_connections"
            WHERE "id" IN (
                SELECT "id" FROM (
                    SELECT
                        "id",
                        ROW_NUMBER() OVER (
                            PARTITION BY "userId", "serverId"
                            ORDER BY "updatedAt" DESC, "createdAt" DESC
                        ) AS rn
                    FROM "mcp_connections"
                ) t
                WHERE t.rn > 1
            );
        "#;
        manager
            .get_connection()
            .execute(Statement::from_string(backend, dedupe_sql))
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq_mcp_connections_user_server")
                    .table(McpConnections::Table)
                    .col(McpConnections::UserId)
                    .col(McpConnections::ServerId)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("uq_mcp_connections_user_server")
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum McpConnections {
    #[iden = "mcp_connections"]
    Table,
    #[iden = "userId"]
    UserId,
    #[iden = "serverId"]
    ServerId,
}
