// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const HASH_INDEX: &str = "idx_files_user_sha256_uploaded";
const CREATE_HASH_INDEX_SQL: &str = r#"CREATE INDEX "idx_files_user_sha256_uploaded"
ON "files" ("userId", "sha256")
WHERE "sha256" IS NOT NULL AND "status" = 'uploaded'"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Files::Table)
                    .add_column(ColumnDef::new(Files::Sha256).string_len(64).null())
                    .to_owned(),
            )
            .await?;

        // SeaQuery cannot express a PostgreSQL partial index predicate. Null hashes preserve
        // legacy rows; the upload transaction's advisory lock serializes matching writes.
        manager
            .get_connection()
            .execute_unprepared(CREATE_HASH_INDEX_SQL)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(Index::drop().name(HASH_INDEX).to_owned())
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Files::Table)
                    .drop_column(Files::Sha256)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum Files {
    #[iden = "files"]
    Table,
    Sha256,
}

#[cfg(test)]
mod tests {
    use super::{CREATE_HASH_INDEX_SQL, HASH_INDEX};

    #[test]
    fn lookup_index_is_user_scoped_and_allows_independent_rows() {
        assert_eq!(HASH_INDEX, "idx_files_user_sha256_uploaded");
        assert!(!CREATE_HASH_INDEX_SQL.contains("UNIQUE"));
        for predicate in [
            r#""userId", "sha256""#,
            r#""sha256" IS NOT NULL"#,
            r#""status" = 'uploaded'"#,
        ] {
            assert!(CREATE_HASH_INDEX_SQL.contains(predicate));
        }
    }
}
