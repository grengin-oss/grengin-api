use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(UsageSummaryDaily::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(UsageSummaryDaily::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(UsageSummaryDaily::Date)
                            .date()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UsageSummaryDaily::UserId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UsageSummaryDaily::Department)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(UsageSummaryDaily::ModelProvider)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UsageSummaryDaily::ModelName)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UsageSummaryDaily::TotalRequests)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(UsageSummaryDaily::TotalTokens)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(UsageSummaryDaily::TotalCost)
                            .decimal_len(20, 10)
                            .not_null()
                            .default(0.0),
                    )
                    .col(
                        ColumnDef::new(UsageSummaryDaily::AverageLatency)
                            .decimal_len(20, 2)
                            .not_null()
                            .default(0.0),
                    )
                    .col(
                        ColumnDef::new(UsageSummaryDaily::SuccessCount)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(UsageSummaryDaily::ErrorCount)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(UsageSummaryDaily::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(UsageSummaryDaily::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_usage_summary_daily_user")
                            .from(UsageSummaryDaily::Table, UsageSummaryDaily::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq_usage_summary_daily_date_user_model")
                    .table(UsageSummaryDaily::Table)
                    .col(UsageSummaryDaily::Date)
                    .col(UsageSummaryDaily::UserId)
                    .col(UsageSummaryDaily::ModelProvider)
                    .col(UsageSummaryDaily::ModelName)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_usage_summary_daily_date")
                    .table(UsageSummaryDaily::Table)
                    .col(UsageSummaryDaily::Date)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_usage_summary_daily_user_id")
                    .table(UsageSummaryDaily::Table)
                    .col(UsageSummaryDaily::UserId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_usage_summary_daily_department")
                    .table(UsageSummaryDaily::Table)
                    .col(UsageSummaryDaily::Department)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(Index::drop().name("idx_usage_summary_daily_department").to_owned())
            .await?;

        manager
            .drop_index(Index::drop().name("idx_usage_summary_daily_user_id").to_owned())
            .await?;

        manager
            .drop_index(Index::drop().name("idx_usage_summary_daily_date").to_owned())
            .await?;

        manager
            .drop_index(Index::drop().name("uq_usage_summary_daily_date_user_model").to_owned())
            .await?;

        manager
            .drop_table(Table::drop().table(UsageSummaryDaily::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum UsageSummaryDaily {
    #[iden = "usage_summary_daily"]
    Table,

    #[iden = "id"]
    Id,

    #[iden = "date"]
    Date,

    #[iden = "userId"]
    UserId,

    #[iden = "department"]
    Department,

    #[iden = "modelProvider"]
    ModelProvider,

    #[iden = "modelName"]
    ModelName,

    #[iden = "totalRequests"]
    TotalRequests,

    #[iden = "totalTokens"]
    TotalTokens,

    #[iden = "totalCost"]
    TotalCost,

    #[iden = "averageLatency"]
    AverageLatency,

    #[iden = "successCount"]
    SuccessCount,

    #[iden = "errorCount"]
    ErrorCount,

    #[iden = "createdAt"]
    CreatedAt,

    #[iden = "updatedAt"]
    UpdatedAt,
}

#[derive(Iden)]
enum Users {
    #[iden = "users"]
    Table,
    #[iden = "id"]
    Id,
}
