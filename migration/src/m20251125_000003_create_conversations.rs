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
                    .table(Conversations::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Conversations::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Conversations::UserId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Conversations::Title)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Conversations::ModelProvider)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Conversations::ModelName)
                            .string()
                            .not_null(),
                    )
                    .col(
                        // Use timestamp_with_time_zone for Postgres; switch to .timestamp() for MySQL/SQLite
                        ColumnDef::new(Conversations::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Conversations::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Conversations::LastMessageAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Conversations::ArchivedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Conversations::MessageCount)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        // For portability: use BigInteger; if you’re on MySQL and want unsigned, use .big_unsigned()
                        ColumnDef::new(Conversations::TotalTokens)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        // Adjust precision/scale if your usage differs
                        ColumnDef::new(Conversations::TotalCost)
                            .decimal_len(18, 6)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Conversations::Metadata)
                            .json_binary()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // FK: conversations.userId -> users.id
        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk_conversations_user_id")
                    .from(Conversations::Table, Conversations::UserId)
                    .to(Users::Table, Users::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::NoAction)
                    .to_owned(),
            )
            .await?;

        // Helpful indexes
        manager
            .create_index(
                Index::create()
                    .name("idx_conversations_userId")
                    .table(Conversations::Table)
                    .col(Conversations::UserId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_conversations_updatedAt")
                    .table(Conversations::Table)
                    .col(Conversations::UpdatedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_conversations_lastMessageAt")
                    .table(Conversations::Table)
                    .col(Conversations::LastMessageAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_conversations_archivedAt")
                    .table(Conversations::Table)
                    .col(Conversations::ArchivedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Drop in reverse order (indexes & FKs are removed automatically with table drop in most DBs,
        // but doing explicit drops is fine if your engine requires it)
        manager
            .drop_table(Table::drop().table(Conversations::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum Conversations {
    Table,
    /// id
    #[iden = "id"]
    Id,
    /// userId
    #[iden = "userId"]
    UserId,
    /// title
    #[iden = "title"]
    Title,
    /// modelProvider
    #[iden = "modelProvider"]
    ModelProvider,
    /// modelName
    #[iden = "modelName"]
    ModelName,
    /// createdAt
    #[iden = "createdAt"]
    CreatedAt,
    /// updatedAt
    #[iden = "updatedAt"]
    UpdatedAt,
    /// lastMessageAt
    #[iden = "lastMessageAt"]
    LastMessageAt,
    /// archivedAt
    #[iden = "archivedAt"]
    ArchivedAt,
    /// messageCount
    #[iden = "messageCount"]
    MessageCount,
    /// totalTokens
    #[iden = "totalTokens"]
    TotalTokens,
    /// totalCost
    #[iden = "totalCost"]
    TotalCost,
    /// metadata
    #[iden = "metadata"]
    Metadata,
}

// Minimal `users` iden for FK target; adjust if your users table/column names differ.
#[derive(Iden)]
enum Users {
    #[iden = "users"]
    Table,
    #[iden = "id"]
    Id,
}
