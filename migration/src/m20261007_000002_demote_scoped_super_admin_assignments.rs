// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

// Multi-table conditional updates keyed on role names read clearest as plain SQL.
// Order matters: affected users are flagged for a permission refresh while their scoped
// Super Admin rows can still be identified.
pub const UP_STATEMENTS: [&str; 4] = [
    r#"UPDATE "users" SET "effectivePermissions" = NULL
WHERE "id" IN (
    SELECT a."userId" FROM "user_role_assignments" a
    JOIN "roles" r ON r."id" = a."roleId"
    WHERE r."name" = 'Super Admin' AND a."scopeDepartmentId" IS NOT NULL
)"#,
    r#"DELETE FROM "user_role_assignments" a USING "roles" r
WHERE a."roleId" = r."id" AND r."name" = 'Super Admin' AND a."scopeDepartmentId" IS NOT NULL
AND EXISTS (
    SELECT 1 FROM "user_role_assignments" d
    JOIN "roles" dr ON dr."id" = d."roleId"
    WHERE dr."name" = 'Department Admin'
    AND d."userId" = a."userId"
    AND d."scopeDepartmentId" = a."scopeDepartmentId"
)"#,
    r#"UPDATE "user_role_assignments" a SET "roleId" = dr."id", "updatedAt" = now()
FROM "roles" r, "roles" dr
WHERE a."roleId" = r."id" AND r."name" = 'Super Admin' AND a."scopeDepartmentId" IS NOT NULL
AND dr."name" = 'Department Admin'"#,
    r#"DELETE FROM "user_role_assignments" a USING "roles" r
WHERE a."roleId" = r."id" AND r."name" = 'Super Admin' AND a."scopeDepartmentId" IS NOT NULL"#,
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for statement in UP_STATEMENTS {
            db.execute_unprepared(statement).await?;
        }
        Ok(())
    }

    // Demoted grants are indistinguishable from real Department Admin grants, and
    // restoring scoped Super Admin rows would reintroduce the escalation.
    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::UP_STATEMENTS;

    #[test]
    fn affected_users_are_flagged_before_their_rows_change() {
        assert!(UP_STATEMENTS[0].starts_with(r#"UPDATE "users""#));
        assert!(
            UP_STATEMENTS[1..]
                .iter()
                .all(|statement| statement.contains("user_role_assignments"))
        );
    }

    #[test]
    fn every_statement_only_touches_scoped_super_admin_rows() {
        for statement in UP_STATEMENTS {
            assert!(
                statement.contains(r#"r."name" = 'Super Admin'"#),
                "{statement}"
            );
            assert!(
                statement.contains(r#"a."scopeDepartmentId" IS NOT NULL"#),
                "{statement}"
            );
        }
    }

    #[test]
    fn duplicates_are_removed_before_conversion_to_avoid_unique_violations() {
        let delete_duplicates = UP_STATEMENTS[1];
        let convert = UP_STATEMENTS[2];
        assert!(delete_duplicates.starts_with("DELETE") && delete_duplicates.contains("EXISTS"));
        assert!(convert.contains(r#"SET "roleId" = dr."id""#));
        assert!(convert.contains(r#"dr."name" = 'Department Admin'"#));
    }
}
