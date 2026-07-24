use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Skills::Table)
                    .rename_column(Alias::new("systemRole"), Alias::new("instructions"))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Skills::Table)
                    .rename_column(Alias::new("instructions"), Alias::new("systemRole"))
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Skills {
    #[sea_orm(iden = "skills")]
    Table,
}
