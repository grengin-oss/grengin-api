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
                    .table(GrenginServiceBindings::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(GrenginServiceBindings::InstanceUuid)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(GrenginServiceBindings::ServiceOrigin)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(GrenginServiceBindings::InstallationId)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(GrenginServiceBindings::OrganizationId)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(GrenginServiceBindings::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(GrenginServiceBindings::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum GrenginServiceBindings {
    #[sea_orm(iden = "grengin_service_bindings")]
    Table,
    #[sea_orm(iden = "instanceUuid")]
    InstanceUuid,
    #[sea_orm(iden = "serviceOrigin")]
    ServiceOrigin,
    #[sea_orm(iden = "installationId")]
    InstallationId,
    #[sea_orm(iden = "organizationId")]
    OrganizationId,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
}
