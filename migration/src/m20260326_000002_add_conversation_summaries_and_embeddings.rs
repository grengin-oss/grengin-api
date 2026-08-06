// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(r#"CREATE EXTENSION IF NOT EXISTS vector;"#)
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ConversationSummaries::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ConversationSummaries::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConversationSummaries::ConversationId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationSummaries::Summary)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationSummaries::LastMessageId)
                            .uuid()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConversationSummaries::LastMessageAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConversationSummaries::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationSummaries::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq_conversation_summaries_conversation")
                    .table(ConversationSummaries::Table)
                    .col(ConversationSummaries::ConversationId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk_conversation_summaries_conversation")
                    .from(
                        ConversationSummaries::Table,
                        ConversationSummaries::ConversationId,
                    )
                    .to(Conversations::Table, Conversations::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(MessageEmbeddings::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(MessageEmbeddings::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(MessageEmbeddings::MessageId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MessageEmbeddings::ConversationId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MessageEmbeddings::Role)
                            .string_len(32)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MessageEmbeddings::Provider)
                            .string_len(50)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MessageEmbeddings::Model)
                            .string_len(200)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MessageEmbeddings::Dimensions)
                            .integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(MessageEmbeddings::Embedding)
                            .custom(Alias::new("vector"))
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MessageEmbeddings::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        // pgvector indexes require fixed dimensions
        manager
            .get_connection()
            .execute_unprepared(
                r#"ALTER TABLE "message_embeddings"
                   ALTER COLUMN "embedding" TYPE vector(1536);"#,
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_message_embeddings_conversation")
                    .table(MessageEmbeddings::Table)
                    .col(MessageEmbeddings::ConversationId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq_message_embeddings_message_provider_model")
                    .table(MessageEmbeddings::Table)
                    .col(MessageEmbeddings::MessageId)
                    .col(MessageEmbeddings::Provider)
                    .col(MessageEmbeddings::Model)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk_message_embeddings_message")
                    .from(MessageEmbeddings::Table, MessageEmbeddings::MessageId)
                    .to(Messages::Table, Messages::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk_message_embeddings_conversation")
                    .from(MessageEmbeddings::Table, MessageEmbeddings::ConversationId)
                    .to(Conversations::Table, Conversations::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"CREATE INDEX IF NOT EXISTS idx_message_embeddings_embedding
                   ON "message_embeddings"
                   USING ivfflat ("embedding" vector_cosine_ops);"#,
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(r#"DROP INDEX IF EXISTS idx_message_embeddings_embedding;"#)
            .await?;

        manager
            .drop_table(Table::drop().table(MessageEmbeddings::Table).to_owned())
            .await?;

        manager
            .drop_table(Table::drop().table(ConversationSummaries::Table).to_owned())
            .await
    }
}

// Table/column idents match entity rename_all="camelCase"
#[derive(DeriveIden)]
enum ConversationSummaries {
    #[sea_orm(iden = "conversation_summaries")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "conversationId")]
    ConversationId,
    #[sea_orm(iden = "summary")]
    Summary,
    #[sea_orm(iden = "lastMessageId")]
    LastMessageId,
    #[sea_orm(iden = "lastMessageAt")]
    LastMessageAt,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "updatedAt")]
    UpdatedAt,
}

#[derive(DeriveIden)]
enum MessageEmbeddings {
    #[sea_orm(iden = "message_embeddings")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "messageId")]
    MessageId,
    #[sea_orm(iden = "conversationId")]
    ConversationId,
    #[sea_orm(iden = "role")]
    Role,
    #[sea_orm(iden = "provider")]
    Provider,
    #[sea_orm(iden = "model")]
    Model,
    #[sea_orm(iden = "dimensions")]
    Dimensions,
    #[sea_orm(iden = "embedding")]
    Embedding,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
}

#[derive(Iden)]
enum Conversations {
    #[iden = "conversations"]
    Table,
    Id,
}

#[derive(Iden)]
enum Messages {
    #[iden = "messages"]
    Table,
    Id,
}
