// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

// The bundled sqlx-mcp server is retired and no longer shipped, so stdio servers that ran it would
// fail on every start; they are disabled instead. The JSON ->> lookup has no SeaORM equivalent here.
const UP: &str = r#"
UPDATE mcp_servers SET enabled = false, "updatedAt" = now()
WHERE "transportType" = 'stdio' AND enabled
  AND "connectionConfig"->>'command' IN ('sqlx-mcp', '/usr/local/bin/sqlx-mcp');
"#;

// Re-enabling cannot tell which of them an admin had disabled before; it restores all of them.
const DOWN: &str = r#"
UPDATE mcp_servers SET enabled = true, "updatedAt" = now()
WHERE "transportType" = 'stdio' AND NOT enabled
  AND "connectionConfig"->>'command' IN ('sqlx-mcp', '/usr/local/bin/sqlx-mcp');
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(UP)
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DOWN)
            .await
            .map(|_| ())
    }
}
