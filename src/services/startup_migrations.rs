// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, TransactionTrait};

const MIGRATION_LOCK_SQL: &str =
    "SELECT pg_advisory_xact_lock(hashtextextended('grengin-api:start-migrations', 0))";

pub async fn run_startup_migrations(database: &DatabaseConnection) -> Result<(), DbErr> {
    let transaction = database.begin().await?;
    // SeaORM has no typed API for PostgreSQL advisory locks.
    transaction.execute_unprepared(MIGRATION_LOCK_SQL).await?;
    Migrator::up(&transaction, None).await?;
    transaction.commit().await
}

#[cfg(test)]
mod tests {
    use super::MIGRATION_LOCK_SQL;

    #[test]
    fn startup_migrations_use_a_transaction_scoped_named_lock() {
        assert!(MIGRATION_LOCK_SQL.contains("pg_advisory_xact_lock"));
        assert!(MIGRATION_LOCK_SQL.contains("grengin-api:start-migrations"));
    }
}
