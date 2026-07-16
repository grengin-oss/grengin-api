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
                    .add_column(ColumnDef::new(Skills::UserId).uuid().null())
                    .add_foreign_key(
                        TableForeignKey::new()
                            .name("fk-skills-user-id")
                            .from_tbl(Skills::Table)
                            .from_col(Skills::UserId)
                            .to_tbl(Users::Table)
                            .to_col(Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-skills-userId")
                    .table(Skills::Table)
                    .col(Skills::UserId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(Index::drop().name("idx-skills-userId").table(Skills::Table).to_owned())
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Skills::Table)
                    .drop_foreign_key(Alias::new("fk-skills-user-id"))
                    .drop_column(Skills::UserId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum Skills {
    #[sea_orm(iden = "skills")]
    Table,
    #[sea_orm(iden = "userId")]
    UserId,
}

#[derive(DeriveIden)]
enum Users {
    #[sea_orm(iden = "users")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
}
