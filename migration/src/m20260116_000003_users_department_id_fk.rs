// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 1) Add users.departmentId (uuid, nullable)
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(ColumnDef::new(Users::DepartmentId).uuid().null())
                    .to_owned(),
            )
            .await?;

        // 2) FK users.departmentId -> departments.id
        // Optional FK: ON DELETE SET NULL is usually what you want.
        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-users-department")
                    .from(Users::Table, Users::DepartmentId)
                    .to(Departments::Table, Departments::Id)
                    .on_delete(ForeignKeyAction::SetNull)
                    .on_update(ForeignKeyAction::Restrict)
                    .to_owned(),
            )
            .await?;

        // 3) Drop old users.department (string)
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .drop_column(Users::Department)
                    .to_owned(),
            )
            .await?;

        // Optional: index departmentId
        manager
            .create_index(
                Index::create()
                    .name("idx-users-department-id")
                    .table(Users::Table)
                    .col(Users::DepartmentId)
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
                    .name("fk-users-department")
                    .table(Users::Table)
                    .to_owned(),
            )
            .await?;

        // Drop optional index
        manager
            .drop_index(
                Index::drop()
                    .name("idx-users-department-id")
                    .table(Users::Table)
                    .to_owned(),
            )
            .await
            .ok(); // ignore if missing

        // Re-add old column and remove new one
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(ColumnDef::new(Users::Department).string().null())
                    .drop_column(Users::DepartmentId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum Users {
    #[sea_orm(iden = "users")]
    Table,

    #[sea_orm(iden = "department")]
    Department,
    #[sea_orm(iden = "departmentId")]
    DepartmentId,
}

#[derive(DeriveIden)]
enum Departments {
    #[sea_orm(iden = "departments")]
    Table,

    #[sea_orm(iden = "id")]
    Id,
}
