// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::{prelude::*, sea_orm::ConnectionTrait};

#[derive(DeriveMigrationName)]
pub struct Migration;

// Partial unique indexes have no SeaORM builder; the reserved key is a literal.
const UNIQUE_GRENGIN: &str = r#"CREATE UNIQUE INDEX IF NOT EXISTS ai_engines_grengin_unique
    ON ai_engines ("engineKey") WHERE "engineKey" = 'grengin'"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        let duplicates = db
            .query_one(sea_orm::Statement::from_string(
                manager.get_database_backend(),
                r#"SELECT COUNT(*) AS n FROM ai_engines WHERE "engineKey" = 'grengin'"#,
            ))
            .await?
            .map(|row| row.try_get::<i64>("", "n"))
            .transpose()?
            .unwrap_or(0);
        if duplicates > 1 {
            return Err(DbErr::Migration(
                "several ai_engines rows use the reserved engine key grengin; keep one (custom \
                 engines need another key) and run the migration again"
                    .into(),
            ));
        }
        db.execute_unprepared(UNIQUE_GRENGIN).await?;
        manager
            .create_table(
                Table::create()
                    .table(Lifecycle::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Lifecycle::Id)
                            .small_integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Lifecycle::Generation)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Lifecycle::Revision)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Lifecycle::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(Operations::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Operations::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Operations::Generation)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Operations::Revision)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Operations::DesiredEnabled)
                            .boolean()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Operations::State).text().not_null())
                    .col(ColumnDef::new(Operations::ErrorCode).text().null())
                    .col(
                        ColumnDef::new(Operations::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(Operations::UpdatedAt)
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
                    .table(Operations::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(Lifecycle::Table).if_exists().to_owned())
            .await?;
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS ai_engines_grengin_unique")
            .await
            .map(|_| ())
    }
}

#[derive(DeriveIden)]
enum Lifecycle {
    #[sea_orm(iden = "grengin_engine_lifecycle")]
    Table,
    Id,
    Generation,
    Revision,
    #[sea_orm(iden = "updatedAt")]
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Operations {
    #[sea_orm(iden = "grengin_engine_operations")]
    Table,
    Id,
    Generation,
    Revision,
    #[sea_orm(iden = "desiredEnabled")]
    DesiredEnabled,
    State,
    #[sea_orm(iden = "errorCode")]
    ErrorCode,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "updatedAt")]
    UpdatedAt,
}
