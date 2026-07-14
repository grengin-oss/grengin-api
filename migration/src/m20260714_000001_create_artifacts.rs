use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Artifacts::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Artifacts::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Artifacts::FileId).uuid().not_null())
                    .col(ColumnDef::new(Artifacts::MessageId).uuid().not_null())
                    .col(ColumnDef::new(Artifacts::ConversationId).uuid().not_null())
                    .col(ColumnDef::new(Artifacts::Title).string().not_null())
                    .col(ColumnDef::new(Artifacts::ContentType).string().not_null())
                    .col(
                        ColumnDef::new(Artifacts::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(Artifacts::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_artifacts_file")
                            .from(Artifacts::Table, Artifacts::FileId)
                            .to(Files::Table, Files::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_artifacts_message")
                            .from(Artifacts::Table, Artifacts::MessageId)
                            .to(Messages::Table, Messages::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_artifacts_conversation")
                            .from(Artifacts::Table, Artifacts::ConversationId)
                            .to(Conversations::Table, Conversations::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_artifacts_message_id")
                    .table(Artifacts::Table)
                    .col(Artifacts::MessageId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_artifacts_conversation_id")
                    .table(Artifacts::Table)
                    .col(Artifacts::ConversationId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(Index::drop().name("idx_artifacts_conversation_id").to_owned())
            .await?;
        manager
            .drop_index(Index::drop().name("idx_artifacts_message_id").to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Artifacts::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum Artifacts {
    #[iden = "artifacts"]
    Table,
    #[iden = "id"]
    Id,
    #[iden = "fileId"]
    FileId,
    #[iden = "messageId"]
    MessageId,
    #[iden = "conversationId"]
    ConversationId,
    #[iden = "title"]
    Title,
    #[iden = "contentType"]
    ContentType,
    #[iden = "createdAt"]
    CreatedAt,
    #[iden = "updatedAt"]
    UpdatedAt,
}

#[derive(Iden)]
enum Files {
    #[iden = "files"]
    Table,
    #[iden = "id"]
    Id,
}

#[derive(Iden)]
enum Messages {
    #[iden = "messages"]
    Table,
    #[iden = "id"]
    Id,
}

#[derive(Iden)]
enum Conversations {
    #[iden = "conversations"]
    Table,
    #[iden = "id"]
    Id,
}
