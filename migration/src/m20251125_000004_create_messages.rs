use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Create table
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
                    .col(
                        ColumnDef::new(Messages::ConversationId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Messages::PreviousMessageId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        // Stored as lowercase text: "user" | "assistant" | "system"
                        ColumnDef::new(Messages::Role)
                            .string() // DB text/varchar
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Messages::MessageContent)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Messages::ModelProvider)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Messages::ModelName)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Messages::RequestTokens)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Messages::ResponseTokens)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        // JSON array payloads; store as jsonb with default [] for non-null Vec<..>
                        ColumnDef::new(Messages::ToolsCalls)
                            .array(ColumnType::JsonBinary)
                            .not_null()
                    )
                    .col(
                        ColumnDef::new(Messages::ToolsResults)
                            .array(ColumnType::JsonBinary)
                            .not_null()
                    )
                    .col(
                        ColumnDef::new(Messages::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Messages::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Messages::TotalTokens)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Messages::Latency)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Messages::Cost)
                            .decimal_len(18, 6)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Messages::Metadata)
                            .json_binary()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // FKs
        // messages.conversationId -> conversations.id
        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk_messages_conversation_id")
                    .from(Messages::Table, Messages::ConversationId)
                    .to(Conversations::Table, Conversations::Id)
                    .on_delete(ForeignKeyAction::Cascade)  // delete messages with their conversation
                    .on_update(ForeignKeyAction::NoAction)
                    .to_owned(),
            )
            .await?;

        // Self-reference: messages.previousMessageId -> messages.id
        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk_messages_previous_message_id")
                    .from(Messages::Table, Messages::PreviousMessageId)
                    .to(Messages::Table, Messages::Id)
                    .on_delete(ForeignKeyAction::Restrict)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        // Indexes
        manager
            .create_index(
                Index::create()
                    .name("idx_messages_conversationId")
                    .table(Messages::Table)
                    .col(Messages::ConversationId)
                    .to_owned(),
            )
            .await?;

        // Enforce one-to-one chain on previousMessageId
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
                    .name("idx_messages_updatedAt")
                    .table(Messages::Table)
                    .col(Messages::UpdatedAt)
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

#[derive(Iden)]
enum Messages {
    Table,
    /// id
    #[iden = "id"]
    Id,
    /// conversationId
    #[iden = "conversationId"]
    ConversationId,
    /// previousMessageId
    #[iden = "previousMessageId"]
    PreviousMessageId,
    /// role
    #[iden = "role"]
    Role,
    /// messageContent
    #[iden = "messageContent"]
    MessageContent,
    /// modelProvider
    #[iden = "modelProvider"]
    ModelProvider,
    /// modelName
    #[iden = "modelName"]
    ModelName,
    /// requestTokens
    #[iden = "requestTokens"]
    RequestTokens,
    /// responseTokens
    #[iden = "responseTokens"]
    ResponseTokens,
    /// toolsCalls
    #[iden = "toolsCalls"]
    ToolsCalls,
    /// toolsResults
    #[iden = "toolsResults"]
    ToolsResults,
    /// createdAt
    #[iden = "createdAt"]
    CreatedAt,
    /// updatedAt
    #[iden = "updatedAt"]
    UpdatedAt,
    /// totalTokens
    #[iden = "totalTokens"]
    TotalTokens,
    /// latency
    #[iden = "latency"]
    Latency,
    /// cost
    #[iden = "cost"]
    Cost,
    /// metadata
    #[iden = "metadata"]
    Metadata,
}

#[derive(Iden)]
enum Conversations {
    #[iden = "conversations"]
    Table,
    #[iden = "id"]
    Id,
}
