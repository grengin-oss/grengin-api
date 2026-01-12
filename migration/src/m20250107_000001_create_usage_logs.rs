use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(UsageLogs::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(UsageLogs::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(UsageLogs::Identifier)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UsageLogs::UserId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UsageLogs::ModelProvider)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UsageLogs::ModelName)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UsageLogs::ConversationId)
                            .uuid()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(UsageLogs::RequestTokens)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(UsageLogs::ResponseTokens)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(UsageLogs::TotalTokens)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(UsageLogs::CostUsd)
                            .decimal_len(20, 10)
                            .not_null()
                            .default(0.0),
                    )
                    .col(
                        ColumnDef::new(UsageLogs::LatencyMs)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(UsageLogs::Timestamp)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(UsageLogs::Department)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(UsageLogs::Status)
                            .string()
                            .not_null()
                            .default("success"),
                    )
                    .col(
                        ColumnDef::new(UsageLogs::ErrorMessage)
                            .text()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(UsageLogs::Metadata)
                            .json_binary()
                            .null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_usage_logs_user")
                            .from(UsageLogs::Table, UsageLogs::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_usage_logs_conversation")
                            .from(UsageLogs::Table, UsageLogs::ConversationId)
                            .to(Conversations::Table, Conversations::Id)
                            .on_delete(ForeignKeyAction::SetNull)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_usage_logs_user_id")
                    .table(UsageLogs::Table)
                    .col(UsageLogs::UserId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_usage_logs_timestamp")
                    .table(UsageLogs::Table)
                    .col(UsageLogs::Timestamp)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_usage_logs_department")
                    .table(UsageLogs::Table)
                    .col(UsageLogs::Department)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_usage_logs_model_provider")
                    .table(UsageLogs::Table)
                    .col(UsageLogs::ModelProvider)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_usage_logs_model_name")
                    .table(UsageLogs::Table)
                    .col(UsageLogs::ModelName)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_usage_logs_status")
                    .table(UsageLogs::Table)
                    .col(UsageLogs::Status)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(Index::drop().name("idx_usage_logs_status").to_owned())
            .await?;

        manager
            .drop_index(Index::drop().name("idx_usage_logs_model_name").to_owned())
            .await?;

        manager
            .drop_index(Index::drop().name("idx_usage_logs_model_provider").to_owned())
            .await?;

        manager
            .drop_index(Index::drop().name("idx_usage_logs_department").to_owned())
            .await?;

        manager
            .drop_index(Index::drop().name("idx_usage_logs_timestamp").to_owned())
            .await?;

        manager
            .drop_index(Index::drop().name("idx_usage_logs_user_id").to_owned())
            .await?;

        manager
            .drop_table(Table::drop().table(UsageLogs::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum UsageLogs {
    #[iden = "usage_logs"]
    Table,

    #[iden = "id"]
    Id,

    #[iden = "identifier"]
    Identifier,

    #[iden = "userId"]
    UserId,

    #[iden = "modelProvider"]
    ModelProvider,

    #[iden = "modelName"]
    ModelName,

    #[iden = "conversationId"]
    ConversationId,

    #[iden = "requestTokens"]
    RequestTokens,

    #[iden = "responseTokens"]
    ResponseTokens,

    #[iden = "totalTokens"]
    TotalTokens,

    #[iden = "costUsd"]
    CostUsd,

    #[iden = "latencyMs"]
    LatencyMs,

    #[iden = "timestamp"]
    Timestamp,

    #[iden = "department"]
    Department,

    #[iden = "status"]
    Status,

    #[iden = "errorMessage"]
    ErrorMessage,

    #[iden = "metadata"]
    Metadata,
}

#[derive(Iden)]
enum Users {
    #[iden = "users"]
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
