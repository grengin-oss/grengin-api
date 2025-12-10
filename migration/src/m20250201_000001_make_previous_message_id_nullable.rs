use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {

        // 1. Drop existing foreign key
        manager
            .drop_foreign_key(
                ForeignKey::drop()
                    .name("fk_messages_previous_message_id")
                    .table(Messages::Table)
                    .to_owned()
            )
            .await?;

        // 2. Alter the column to be NULLABLE
        manager
            .alter_table(
                TableAlterStatement::new()
                    .table(Messages::Table)
                    .modify_column(
                        ColumnDef::new(Messages::PreviousMessageId)
                            .uuid()
                            .null()     // <-- Make nullable
                    )
                    .to_owned()
            )
            .await?;

        // 3. Recreate foreign key but allow NULLs
        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk_messages_previous_message_id")
                    .from(Messages::Table, Messages::PreviousMessageId)
                    .to(Messages::Table, Messages::Id)
                    .on_delete(ForeignKeyAction::Restrict)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned()
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {

        // Reverse order

        manager
            .drop_foreign_key(
                ForeignKey::drop()
                    .name("fk_messages_previous_message_id")
                    .table(Messages::Table)
                    .to_owned()
            )
            .await?;

        manager
            .alter_table(
                TableAlterStatement::new()
                    .table(Messages::Table)
                    .modify_column(
                        ColumnDef::new(Messages::PreviousMessageId)
                            .uuid()
                            .not_null()   // rollback to NOT NULL
                    )
                    .to_owned()
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk_messages_previous_message_id")
                    .from(Messages::Table, Messages::PreviousMessageId)
                    .to(Messages::Table, Messages::Id)
                    .on_delete(ForeignKeyAction::Restrict)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned()
            )
            .await?;

        Ok(())
    }
}

#[derive(Iden)]
enum Messages {
    #[iden = "messages"]
    Table,
    #[iden = "previousMessageId"]
    PreviousMessageId,
    #[iden = "id"]
    Id,
}
