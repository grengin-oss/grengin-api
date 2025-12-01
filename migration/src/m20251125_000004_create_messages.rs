use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // messages
        manager
            .create_table(
                Table::create()
                    .table(Messages::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Messages::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Messages::ConversationId).uuid().not_null())
                    .col(ColumnDef::new(Messages::PreviousMessageId).uuid().not_null())
                    // PromptRole stored as lowercase string per your DeriveActiveEnum attributes
                    .col(ColumnDef::new(Messages::Role).string().not_null())
                    .col(ColumnDef::new(Messages::MessageContent).text().not_null())
                    .col(ColumnDef::new(Messages::ModelProvider).string().not_null())
                    .col(ColumnDef::new(Messages::ModelName).string().not_null())
                    .col(ColumnDef::new(Messages::RequestTokens).integer().not_null())
                    .col(ColumnDef::new(Messages::ResponseTokens).integer().not_null())
                    .col(
                        ColumnDef::new(Messages::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Messages::TotalTokens).integer().not_null())
                    .col(ColumnDef::new(Messages::Latency).integer().not_null())
                    // USD decimal; pick ample precision/scale
                    .col(ColumnDef::new(Messages::Cost).decimal_len(20, 10).not_null())
                    .col(ColumnDef::new(Messages::Metadata).json_binary().null())
                    .to_owned(),
            )
            .await?;

        // FK: messages.conversationId -> conversations.id
        manager
            .create_foreign_key(
                ForeignKey::create()
                    .from(Messages::Table, Messages::ConversationId)
                    .to(Conversations::Table, Conversations::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        // Self-FK (one-to-one): messages.previousMessageId -> messages.id
        // Non-null + UNIQUE enforces a strict 1:1 previous->current relationship
        manager
            .create_foreign_key(
                ForeignKey::create()
                    .from(Messages::Table, Messages::PreviousMessageId)
                    .to(Messages::Table, Messages::Id)
                    .on_delete(ForeignKeyAction::Restrict) // prevents deleting a previous message while it's referenced
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        // Indexes (and 1:1 uniqueness on previousMessageId)
        manager
            .create_index(
                Index::create()
                    .name("idx_messages_conversationId")
                    .table(Messages::Table)
                    .col(Messages::ConversationId)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("uidx_messages_previousMessageId")
                    .table(Messages::Table)
                    .col(Messages::PreviousMessageId)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_messages_createdAt")
                    .table(Messages::Table)
                    .col(Messages::CreatedAt)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_messages_role")
                    .table(Messages::Table)
                    .col(Messages::Role)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Messages::Table).to_owned())
            .await
    }
}


#[derive(DeriveIden)]
enum Messages {
    #[sea_orm(iden = "messages")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "conversationId")]
    ConversationId,
    #[sea_orm(iden = "previousMessageId")]
    PreviousMessageId,
    #[sea_orm(iden = "role")]
    Role,
    #[sea_orm(iden = "messageContent")]
    MessageContent,
    #[sea_orm(iden = "modelProvider")]
    ModelProvider,
    #[sea_orm(iden = "modelName")]
    ModelName,
    #[sea_orm(iden = "requestTokens")]
    RequestTokens,
    #[sea_orm(iden = "responseTokens")]
    ResponseTokens,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "totalTokens")]
    TotalTokens,
    #[sea_orm(iden = "latency")]
    Latency,
    #[sea_orm(iden = "cost")]
    Cost,
    #[sea_orm(iden = "metadata")]
    Metadata,
}

#[derive(DeriveIden)]
enum Conversations {
    #[sea_orm(iden = "conversations")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
}