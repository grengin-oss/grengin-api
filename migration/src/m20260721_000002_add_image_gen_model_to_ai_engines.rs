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
                    .table(AiEngines::Table)
                    .add_column(
                        ColumnDef::new(AiEngines::DefaultImageGenModel)
                            .text()
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AiEngines::Table)
                    .drop_column(AiEngines::DefaultImageGenModel)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum AiEngines {
    #[sea_orm(iden = "ai_engines")]
    Table,
    #[sea_orm(iden = "defaultImageGenModel")]
    DefaultImageGenModel,
}
