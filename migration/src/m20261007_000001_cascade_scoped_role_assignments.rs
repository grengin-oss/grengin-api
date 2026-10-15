// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const SCOPE_DEPARTMENT_FK: &str = "fk-user-role-assignments-scope-dept";

fn drop_scope_department_fk() -> ForeignKeyDropStatement {
    ForeignKey::drop()
        .name(SCOPE_DEPARTMENT_FK)
        .table(UserRoleAssignments::Table)
        .to_owned()
}

fn create_scope_department_fk(on_delete: ForeignKeyAction) -> ForeignKeyCreateStatement {
    ForeignKey::create()
        .name(SCOPE_DEPARTMENT_FK)
        .from(
            UserRoleAssignments::Table,
            UserRoleAssignments::ScopeDepartmentId,
        )
        .to(Departments::Table, Departments::Id)
        .on_delete(on_delete)
        .on_update(ForeignKeyAction::Restrict)
        .to_owned()
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.drop_foreign_key(drop_scope_department_fk()).await?;
        manager
            .create_foreign_key(create_scope_department_fk(ForeignKeyAction::Cascade))
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.drop_foreign_key(drop_scope_department_fk()).await?;
        manager
            .create_foreign_key(create_scope_department_fk(ForeignKeyAction::SetNull))
            .await
    }
}

#[derive(DeriveIden)]
enum UserRoleAssignments {
    #[sea_orm(iden = "user_role_assignments")]
    Table,
    #[sea_orm(iden = "scopeDepartmentId")]
    ScopeDepartmentId,
}

#[derive(DeriveIden)]
enum Departments {
    #[sea_orm(iden = "departments")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up_makes_scoped_assignments_disappear_with_their_department() {
        let sql =
            create_scope_department_fk(ForeignKeyAction::Cascade).to_string(PostgresQueryBuilder);
        assert_eq!(
            sql,
            r#"ALTER TABLE "user_role_assignments" ADD CONSTRAINT "fk-user-role-assignments-scope-dept" FOREIGN KEY ("scopeDepartmentId") REFERENCES "departments" ("id") ON DELETE CASCADE ON UPDATE RESTRICT"#
        );
    }

    #[test]
    fn down_restores_set_null_on_the_same_constraint() {
        let sql =
            create_scope_department_fk(ForeignKeyAction::SetNull).to_string(PostgresQueryBuilder);
        assert!(sql.contains(r#""fk-user-role-assignments-scope-dept""#));
        assert!(sql.contains("ON DELETE SET NULL"));
        assert_eq!(
            drop_scope_department_fk().to_string(PostgresQueryBuilder),
            r#"ALTER TABLE "user_role_assignments" DROP CONSTRAINT "fk-user-role-assignments-scope-dept""#
        );
    }
}
