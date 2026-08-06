// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(SsoProviders::Table)
                    .add_column(
                        ColumnDef::new(SsoProviders::UseGrenginProxy)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(SsoProviders::Table)
                    .drop_column(SsoProviders::UseGrenginProxy)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum SsoProviders {
    #[iden = "sso_providers"]
    Table,
    #[iden = "useGrenginProxy"]
    UseGrenginProxy,
}
