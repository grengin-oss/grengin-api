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
                TableAlterStatement::new()
                    .table(Conversations::Table)
                    .add_column(
                        ColumnDef::new(Conversations::EditRequests)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .add_column(ColumnDef::new(Conversations::EditClaim).integer().null())
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                TableAlterStatement::new()
                    .table(Conversations::Table)
                    .drop_column(Conversations::EditClaim)
                    .drop_column(Conversations::EditRequests)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(Iden)]
enum Conversations {
    #[iden = "conversations"]
    Table,
    #[iden = "editRequests"]
    EditRequests,
    #[iden = "editClaim"]
    EditClaim,
}
