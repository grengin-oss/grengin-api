use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // conversation_summaries: snake_case -> camelCase
        if manager.has_column("conversation_summaries", "conversation_id").await?
            && !manager.has_column("conversation_summaries", "conversationId").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("conversation_summaries"))
                        .rename_column(Alias::new("conversation_id"), Alias::new("conversationId"))
                        .to_owned(),
                )
                .await?;
        }
        if manager.has_column("conversation_summaries", "last_message_id").await?
            && !manager.has_column("conversation_summaries", "lastMessageId").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("conversation_summaries"))
                        .rename_column(Alias::new("last_message_id"), Alias::new("lastMessageId"))
                        .to_owned(),
                )
                .await?;
        }
        if manager.has_column("conversation_summaries", "last_message_at").await?
            && !manager.has_column("conversation_summaries", "lastMessageAt").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("conversation_summaries"))
                        .rename_column(Alias::new("last_message_at"), Alias::new("lastMessageAt"))
                        .to_owned(),
                )
                .await?;
        }
        if manager.has_column("conversation_summaries", "created_at").await?
            && !manager.has_column("conversation_summaries", "createdAt").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("conversation_summaries"))
                        .rename_column(Alias::new("created_at"), Alias::new("createdAt"))
                        .to_owned(),
                )
                .await?;
        }
        if manager.has_column("conversation_summaries", "updated_at").await?
            && !manager.has_column("conversation_summaries", "updatedAt").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("conversation_summaries"))
                        .rename_column(Alias::new("updated_at"), Alias::new("updatedAt"))
                        .to_owned(),
                )
                .await?;
        }

        // message_embeddings: snake_case -> camelCase
        if manager.has_column("message_embeddings", "message_id").await?
            && !manager.has_column("message_embeddings", "messageId").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("message_embeddings"))
                        .rename_column(Alias::new("message_id"), Alias::new("messageId"))
                        .to_owned(),
                )
                .await?;
        }
        if manager.has_column("message_embeddings", "conversation_id").await?
            && !manager.has_column("message_embeddings", "conversationId").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("message_embeddings"))
                        .rename_column(
                            Alias::new("conversation_id"),
                            Alias::new("conversationId"),
                        )
                        .to_owned(),
                )
                .await?;
        }
        if manager.has_column("message_embeddings", "created_at").await?
            && !manager.has_column("message_embeddings", "createdAt").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("message_embeddings"))
                        .rename_column(Alias::new("created_at"), Alias::new("createdAt"))
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // conversation_summaries: camelCase -> snake_case
        if manager.has_column("conversation_summaries", "conversationId").await?
            && !manager.has_column("conversation_summaries", "conversation_id").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("conversation_summaries"))
                        .rename_column(Alias::new("conversationId"), Alias::new("conversation_id"))
                        .to_owned(),
                )
                .await?;
        }
        if manager.has_column("conversation_summaries", "lastMessageId").await?
            && !manager.has_column("conversation_summaries", "last_message_id").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("conversation_summaries"))
                        .rename_column(Alias::new("lastMessageId"), Alias::new("last_message_id"))
                        .to_owned(),
                )
                .await?;
        }
        if manager.has_column("conversation_summaries", "lastMessageAt").await?
            && !manager.has_column("conversation_summaries", "last_message_at").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("conversation_summaries"))
                        .rename_column(Alias::new("lastMessageAt"), Alias::new("last_message_at"))
                        .to_owned(),
                )
                .await?;
        }
        if manager.has_column("conversation_summaries", "createdAt").await?
            && !manager.has_column("conversation_summaries", "created_at").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("conversation_summaries"))
                        .rename_column(Alias::new("createdAt"), Alias::new("created_at"))
                        .to_owned(),
                )
                .await?;
        }
        if manager.has_column("conversation_summaries", "updatedAt").await?
            && !manager.has_column("conversation_summaries", "updated_at").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("conversation_summaries"))
                        .rename_column(Alias::new("updatedAt"), Alias::new("updated_at"))
                        .to_owned(),
                )
                .await?;
        }

        // message_embeddings: camelCase -> snake_case
        if manager.has_column("message_embeddings", "messageId").await?
            && !manager.has_column("message_embeddings", "message_id").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("message_embeddings"))
                        .rename_column(Alias::new("messageId"), Alias::new("message_id"))
                        .to_owned(),
                )
                .await?;
        }
        if manager.has_column("message_embeddings", "conversationId").await?
            && !manager.has_column("message_embeddings", "conversation_id").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("message_embeddings"))
                        .rename_column(
                            Alias::new("conversationId"),
                            Alias::new("conversation_id"),
                        )
                        .to_owned(),
                )
                .await?;
        }
        if manager.has_column("message_embeddings", "createdAt").await?
            && !manager.has_column("message_embeddings", "created_at").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("message_embeddings"))
                        .rename_column(Alias::new("createdAt"), Alias::new("created_at"))
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }
}
