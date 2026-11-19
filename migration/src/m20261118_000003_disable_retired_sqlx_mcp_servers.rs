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

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(UP)
            .await
            .map(|_| ())
    }

    // Rolling back leaves these servers disabled: nothing records which of them an admin had
    // already disabled, and re-enabling those would undo a deliberate choice. Admins re-enable
    // the ones they want once a build that ships sqlx-mcp is back.
    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
