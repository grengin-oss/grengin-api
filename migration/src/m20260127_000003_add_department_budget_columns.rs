// m20260127_000003_add_department_budget_columns.rs

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Departments::Table)
                    .add_column(
                        ColumnDef::new(Departments::BudgetAllocated)
                            .decimal_len(18, 2)
                            .not_null()
                            .default(0),
                    )
                    .add_column(
                        ColumnDef::new(Departments::BudgetPeriod)
                            .string_len(20)
                            .not_null()
                            .default("monthly"),
                    )
                    .add_column(
                        ColumnDef::new(Departments::ActionOnExceed)
                            .string_len(20)
                            .not_null()
                            .default("warn"),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Departments::Table)
                    .drop_column(Departments::ActionOnExceed)
                    .drop_column(Departments::BudgetPeriod)
                    .drop_column(Departments::BudgetAllocated)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum Departments {
    #[sea_orm(iden = "departments")]
    Table,

    // camelCase columns (matches your rename_all = "camelCase")
    #[sea_orm(iden = "budgetAllocated")]
    BudgetAllocated,

    #[sea_orm(iden = "budgetPeriod")]
    BudgetPeriod,

    #[sea_orm(iden = "actionOnExceed")]
    ActionOnExceed,
}
