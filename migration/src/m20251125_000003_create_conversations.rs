use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // conversations
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
                    .col(ColumnDef::new(Conversations::UserId).uuid().not_null())
                    .col(ColumnDef::new(Conversations::Title).string().null())
                    .col(
                        ColumnDef::new(Conversations::ModelProvider)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Conversations::ModelName).string().not_null())
                    .col(
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
                    .col(ColumnDef::new(Conversations::MessageCount).integer().not_null())
                    .col(ColumnDef::new(Conversations::TotalTokens).integer().not_null())
                    .col(ColumnDef::new(Conversations::Metadata).json_binary().null())
                    .to_owned(),
            )
            .await?;

        // FK: conversations.userId -> users.id
        manager
            .create_foreign_key(
                ForeignKey::create()
                    .from(Conversations::Table, Conversations::UserId)
                    .to(Users::Table, Users::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        // Indexes
        manager
            .create_index(
                Index::create()
                    .name("uidx_conversations_id")
                    .table(Conversations::Table)
                    .col(Conversations::Id)
                    .unique()
                    .to_owned(),
            )
            .await?;
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
                    .name("idx_conversations_createdAt")
                    .table(Conversations::Table)
                    .col(Conversations::CreatedAt)
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

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Conversations::Table).to_owned())
            .await
    }
}


#[derive(DeriveIden)]
enum Conversations {
    #[sea_orm(iden = "conversations")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "userId")]
    UserId,
    #[sea_orm(iden = "title")]
    Title,
    #[sea_orm(iden = "modelProvider")]
    ModelProvider,
    #[sea_orm(iden = "modelName")]
    ModelName,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "updatedAt")]
    UpdatedAt,
    #[sea_orm(iden = "lastMessageAt")]
    LastMessageAt,
    #[sea_orm(iden = "archivedAt")]
    ArchivedAt,
    #[sea_orm(iden = "messageCount")]
    MessageCount,
    #[sea_orm(iden = "totalTokens")]
    TotalTokens,
    #[sea_orm(iden = "metadata")]
    Metadata,
}

#[derive(DeriveIden)]
enum Users {
    #[sea_orm(iden = "users")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
}
