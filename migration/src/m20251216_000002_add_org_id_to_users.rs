use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 1) Add nullable orgId column to users
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(ColumnDef::new(Users::OrgId).uuid().null())
                    .to_owned(),
            )
            .await?;

        // 2) Create index on orgId
        manager
            .create_index(
                Index::create()
                    .name("idx-users-orgId")
                    .table(Users::Table)
                    .col(Users::OrgId)
                    .to_owned(),
            )
            .await?;

        // 3) FK users.orgId -> organizations.id
        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-users-orgId-organizations-id")
                    .from(Users::Table, Users::OrgId)
                    .to(Organizations::Table, Organizations::Id)
                    .on_delete(ForeignKeyAction::SetNull)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Drop FK first
        manager
            .drop_foreign_key(
                ForeignKey::drop()
                    .name("fk-users-orgId-organizations-id")
                    .table(Users::Table)
                    .to_owned(),
            )
            .await?;

        // Drop index
        manager
            .drop_index(
                Index::drop()
                    .name("idx-users-orgId")
                    .table(Users::Table)
                    .to_owned(),
            )
            .await?;

        // Drop column
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .drop_column(Users::OrgId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(Iden)]
enum Users {
    #[iden = "users"]
    Table,
    // camelCase column name to match your rename_all="camelCase"
    #[iden = "orgId"]
    OrgId,
}

#[derive(Iden)]
enum Organizations {
    #[iden = "organizations"]
    Table,
    #[iden = "id"]
    Id,
}
