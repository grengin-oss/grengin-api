use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Add new nullable string column: requestId
        manager
            .alter_table(
                TableAlterStatement::new()
                    .table(Messages::Table)
                    .add_column(
                        ColumnDef::new(Messages::RequestId)
                            .string()   // VARCHAR/TEXT
                            .null()     // Nullable
                    )
                    .to_owned()
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Remove the column on rollback
        manager
            .alter_table(
                TableAlterStatement::new()
                    .table(Messages::Table)
                    .drop_column(Messages::RequestId)
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
    #[iden = "requestId"]
    RequestId,
}
