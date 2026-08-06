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
                    .table(AuditLogs::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AuditLogs::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(AuditLogs::UserId).uuid().null())
                    .col(ColumnDef::new(AuditLogs::Action).string().not_null())
                    .col(ColumnDef::new(AuditLogs::ResourceType).string().null())
                    .col(ColumnDef::new(AuditLogs::ResourceId).string().null())
                    .col(ColumnDef::new(AuditLogs::Details).json_binary().null())
                    .col(ColumnDef::new(AuditLogs::IpAddress).string().null())
                    .col(ColumnDef::new(AuditLogs::UserAgent).text().null())
                    .col(
                        ColumnDef::new(AuditLogs::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-audit-logs-user-id")
                    .table(AuditLogs::Table)
                    .col(AuditLogs::UserId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-audit-logs-action")
                    .table(AuditLogs::Table)
                    .col(AuditLogs::Action)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-audit-logs-created-at")
                    .table(AuditLogs::Table)
                    .col(AuditLogs::CreatedAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx-audit-logs-created-at")
                    .table(AuditLogs::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx-audit-logs-action")
                    .table(AuditLogs::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx-audit-logs-user-id")
                    .table(AuditLogs::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(AuditLogs::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum AuditLogs {
    #[sea_orm(iden = "audit_logs")]
    Table,
    Id,
    #[sea_orm(iden = "userId")]
    UserId,
    Action,
    #[sea_orm(iden = "resourceType")]
    ResourceType,
    #[sea_orm(iden = "resourceId")]
    ResourceId,
    Details,
    #[sea_orm(iden = "ipAddress")]
    IpAddress,
    #[sea_orm(iden = "userAgent")]
    UserAgent,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
}
