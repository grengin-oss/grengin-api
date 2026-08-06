// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Notifications::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Notifications::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Notifications::UserId).uuid().not_null())
                    .col(ColumnDef::new(Notifications::DepartmentId).uuid().null())
                    .col(ColumnDef::new(Notifications::Kind).string().not_null())
                    .col(ColumnDef::new(Notifications::Title).string().not_null())
                    .col(ColumnDef::new(Notifications::Body).text().not_null())
                    .col(
                        ColumnDef::new(Notifications::Payload)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Notifications::PeriodStart)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Notifications::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Notifications::ReadAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_notifications_user_created_at")
                    .table(Notifications::Table)
                    .col(Notifications::UserId)
                    .col(Notifications::CreatedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_notifications_department_kind_period")
                    .table(Notifications::Table)
                    .col(Notifications::DepartmentId)
                    .col(Notifications::Kind)
                    .col(Notifications::PeriodStart)
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk_notifications_user_id")
                    .from(Notifications::Table, Notifications::UserId)
                    .to(Users::Table, Users::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::NoAction)
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk_notifications_department_id")
                    .from(Notifications::Table, Notifications::DepartmentId)
                    .to(Departments::Table, Departments::Id)
                    .on_delete(ForeignKeyAction::SetNull)
                    .on_update(ForeignKeyAction::NoAction)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Notifications::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Notifications {
    #[sea_orm(iden = "notifications")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "userId")]
    UserId,
    #[sea_orm(iden = "departmentId")]
    DepartmentId,
    #[sea_orm(iden = "kind")]
    Kind,
    #[sea_orm(iden = "title")]
    Title,
    #[sea_orm(iden = "body")]
    Body,
    #[sea_orm(iden = "payload")]
    Payload,
    #[sea_orm(iden = "periodStart")]
    PeriodStart,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "readAt")]
    ReadAt,
}

#[derive(DeriveIden)]
enum Users {
    #[sea_orm(iden = "users")]
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Departments {
    #[sea_orm(iden = "departments")]
    Table,
    Id,
}
