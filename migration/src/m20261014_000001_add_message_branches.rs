// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

// Window functions can't be written as a SeaQuery UPDATE. Each shown user message gets the shown
// message before it as its parent, which turns the shown history into a connected thread; hidden
// messages keep no parent, so they never appear as versions.
const BACKFILL_USER_MESSAGE_PARENTS: &str = r#"
UPDATE messages AS m
SET "previousMessageId" = ordered.previous_id
FROM (
    SELECT id, LAG(id) OVER (PARTITION BY "conversationId" ORDER BY "createdAt", id) AS previous_id
    FROM messages
    WHERE NOT deleted
) AS ordered
WHERE m.id = ordered.id
  AND m.role = 'user'
  AND m."previousMessageId" IS NULL
  AND ordered.previous_id IS NOT NULL
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                TableAlterStatement::new()
                    .table(Messages::Table)
                    .add_column(
                        ColumnDef::new(Messages::InactiveBranch)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;
        // An edit adds a second child under the edited message's parent, so a parent can no
        // longer have only one child.
        manager
            .drop_index(
                Index::drop()
                    .name("uq_messages_previousMessageId")
                    .table(Messages::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_messages_previousMessageId")
                    .table(Messages::Table)
                    .col(Messages::PreviousMessageId)
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(BACKFILL_USER_MESSAGE_PARENTS)
            .await?;

        Ok(())
    }

    // The backfilled parent links stay: they describe the history that was shown and are correct
    // without the branch column too. Restoring one child per message fails while any message has
    // more than one version, so those versions have to be removed first.
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_messages_previousMessageId")
                    .table(Messages::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("uq_messages_previousMessageId")
                    .table(Messages::Table)
                    .col(Messages::PreviousMessageId)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                TableAlterStatement::new()
                    .table(Messages::Table)
                    .drop_column(Messages::InactiveBranch)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(Iden)]
enum Messages {
    #[iden = "messages"]
    Table,
    #[iden = "inactiveBranch"]
    InactiveBranch,
    #[iden = "previousMessageId"]
    PreviousMessageId,
}
