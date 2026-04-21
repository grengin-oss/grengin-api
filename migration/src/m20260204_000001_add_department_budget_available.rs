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
                        ColumnDef::new(Departments::BudgetAvailable)
                            .decimal_len(18, 2)
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(r#"UPDATE departments SET "budgetAvailable" = "budgetAllocated";"#)
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Departments::Table)
                    .drop_column(Departments::BudgetAvailable)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Departments {
    #[sea_orm(iden = "departments")]
    Table,
    #[sea_orm(iden = "budgetAvailable")]
    BudgetAvailable,
}
