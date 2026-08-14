// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::{prelude::*, sea_orm::ConnectionTrait};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in ["provider_credentials", "provider_plugins"] {
            if table_has_rows(manager, table).await? {
                return Err(DbErr::Custom(format!(
                    "legacy table '{table}' contains data; migrate it to ai_engines before removing the table"
                )));
            }
        }

        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("provider_credentials"))
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("provider_plugins"))
                    .if_exists()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        crate::m20260810_000001_create_provider_plugins::Migration
            .up(manager)
            .await
    }
}

async fn table_has_rows(manager: &SchemaManager<'_>, table: &str) -> Result<bool, DbErr> {
    if !manager.has_table(table).await? {
        return Ok(false);
    }

    let query = Query::select()
        .expr(Expr::value(1))
        .from(Alias::new(table))
        .limit(1)
        .to_owned();
    let db = manager.get_connection();

    Ok(db
        .query_one(db.get_database_backend().build(&query))
        .await?
        .is_some())
}
