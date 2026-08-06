// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            r#"CREATE INDEX IF NOT EXISTS idx_messages_fts
               ON messages USING GIN (to_tsvector('english', "messageContent"))"#,
        )
        .await?;
        db.execute_unprepared(
            r#"CREATE INDEX IF NOT EXISTS idx_conversations_title_fts
               ON conversations USING GIN (to_tsvector('english', coalesce(title, '')))"#,
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS idx_messages_fts")
            .await?;
        db.execute_unprepared("DROP INDEX IF EXISTS idx_conversations_title_fts")
            .await?;
        Ok(())
    }
}
