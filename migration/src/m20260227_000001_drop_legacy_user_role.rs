// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{DatabaseBackend, Statement};
use uuid::Uuid;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let role_ids = load_role_ids(manager).await?;
        let has_role_column = legacy_role_column_exists(manager).await?;

        if has_role_column {
            if let Some(role_id) = role_ids.get("Super Admin") {
                assign_legacy_users(manager, *role_id, &["admin", "superadmin"]).await?;
            }

            if let Some(role_id) = role_ids.get("Observer") {
                assign_legacy_users(manager, *role_id, &["observer"]).await?;
            }

            if let Some(role_id) = role_ids.get("User") {
                assign_legacy_users(manager, *role_id, &["user"]).await?;
            }
        }

        if let Some(role_id) = role_ids.get("User") {
            assign_unassigned_users(manager, *role_id).await?;
        }

        if has_role_column {
            manager
                .alter_table(
                    Table::alter()
                        .table(Users::Table)
                        .drop_column(Users::Role)
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::Role)
                            .string()
                            .not_null()
                            .default("user"),
                    )
                    .to_owned(),
            )
            .await?;

        let role_ids = load_role_ids(manager).await?;
        if let Some(role_id) = role_ids.get("Super Admin") {
            let update = format!(
                "UPDATE users SET \"role\" = 'superadmin' FROM user_role_assignments ura WHERE ura.\"userId\" = users.id AND ura.\"roleId\" = '{}' AND ura.\"scopeDepartmentId\" IS NULL",
                role_id
            );
            manager.get_connection().execute_unprepared(&update).await?;
        }

        if let Some(role_id) = role_ids.get("Observer") {
            let update = format!(
                "UPDATE users SET \"role\" = 'observer' FROM user_role_assignments ura WHERE ura.\"userId\" = users.id AND ura.\"roleId\" = '{}' AND ura.\"scopeDepartmentId\" IS NULL AND users.\"role\" != 'superadmin'",
                role_id
            );
            manager.get_connection().execute_unprepared(&update).await?;
        }

        Ok(())
    }
}

async fn load_role_ids(
    manager: &SchemaManager<'_>,
) -> Result<std::collections::HashMap<String, Uuid>, DbErr> {
    let mut role_ids = std::collections::HashMap::new();
    let role_rows = manager
        .get_connection()
        .query_all(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT \"id\", \"name\" FROM roles",
        ))
        .await?;

    for row in role_rows {
        let id: Uuid = row.try_get("", "id")?;
        let name: String = row.try_get("", "name")?;
        role_ids.insert(name, id);
    }

    Ok(role_ids)
}

async fn legacy_role_column_exists(manager: &SchemaManager<'_>) -> Result<bool, DbErr> {
    let rows = manager
        .get_connection()
        .query_all(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT 1 FROM information_schema.columns WHERE table_schema = 'public' AND table_name = 'users' AND column_name = 'role'",
        ))
        .await?;
    Ok(!rows.is_empty())
}

async fn assign_legacy_users(
    manager: &SchemaManager<'_>,
    role_id: Uuid,
    legacy_roles: &[&str],
) -> Result<(), DbErr> {
    if legacy_roles.is_empty() {
        return Ok(());
    }

    let roles_list = legacy_roles
        .iter()
        .map(|role| format!("'{}'", role.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(",");

    let query = format!(
        "SELECT u.\"id\" FROM users u LEFT JOIN user_role_assignments ura ON ura.\"userId\" = u.\"id\" AND ura.\"roleId\" = '{}' AND ura.\"scopeDepartmentId\" IS NULL WHERE u.\"role\" IN ({}) AND ura.\"id\" IS NULL",
        role_id, roles_list
    );

    let rows = manager
        .get_connection()
        .query_all(Statement::from_string(DatabaseBackend::Postgres, query))
        .await?;

    let mut user_ids = Vec::with_capacity(rows.len());
    for row in rows {
        let user_id: Uuid = row.try_get("", "id")?;
        user_ids.push(user_id);
    }

    insert_role_assignments(manager, role_id, &user_ids).await
}

async fn assign_unassigned_users(manager: &SchemaManager<'_>, role_id: Uuid) -> Result<(), DbErr> {
    let query = "SELECT u.\"id\" FROM users u LEFT JOIN user_role_assignments ura ON ura.\"userId\" = u.\"id\" WHERE ura.\"id\" IS NULL";
    let rows = manager
        .get_connection()
        .query_all(Statement::from_string(DatabaseBackend::Postgres, query))
        .await?;

    let mut user_ids = Vec::with_capacity(rows.len());
    for row in rows {
        let user_id: Uuid = row.try_get("", "id")?;
        user_ids.push(user_id);
    }

    insert_role_assignments(manager, role_id, &user_ids).await
}

async fn insert_role_assignments(
    manager: &SchemaManager<'_>,
    role_id: Uuid,
    user_ids: &[Uuid],
) -> Result<(), DbErr> {
    if user_ids.is_empty() {
        return Ok(());
    }

    let values = user_ids
        .iter()
        .map(|user_id| {
            format!(
                "('{}','{}','{}',NULL,'{}', NOW(), NOW())",
                Uuid::new_v4(),
                user_id,
                role_id,
                user_id
            )
        })
        .collect::<Vec<_>>();

    let insert = format!(
        "INSERT INTO user_role_assignments (\"id\", \"userId\", \"roleId\", \"scopeDepartmentId\", \"assignedBy\", \"createdAt\", \"updatedAt\") VALUES {}",
        values.join(",")
    );

    manager.get_connection().execute_unprepared(&insert).await?;

    Ok(())
}

#[derive(DeriveIden)]
enum Users {
    #[sea_orm(iden = "users")]
    Table,
    #[sea_orm(iden = "role")]
    Role,
}
