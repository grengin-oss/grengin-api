// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;
use uuid::Uuid;

#[derive(DeriveMigrationName)]
pub struct Migration;

// GET /admin/ai-engines requires ai_platform:view, and manage never implies view.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let permission_ids = self
            .lookup_permission_ids(manager, "ai_platform", &["view"])
            .await?;
        let role_ids = self.lookup_role_ids(manager, &["IT Admin"]).await?;
        if permission_ids.is_empty() || role_ids.is_empty() {
            return Ok(());
        }

        let mut insert = Query::insert();
        insert
            .into_table(RolePermissions::Table)
            .columns([RolePermissions::RoleId, RolePermissions::PermissionId])
            .on_conflict(
                OnConflict::columns([RolePermissions::RoleId, RolePermissions::PermissionId])
                    .do_nothing()
                    .to_owned(),
            );
        for role_id in &role_ids {
            for permission_id in &permission_ids {
                insert.values_panic([(*role_id).into(), (*permission_id).into()]);
            }
        }
        manager.exec_stmt(insert).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let permission_ids = self
            .lookup_permission_ids(manager, "ai_platform", &["view"])
            .await?;
        let role_ids = self.lookup_role_ids(manager, &["IT Admin"]).await?;
        if permission_ids.is_empty() || role_ids.is_empty() {
            return Ok(());
        }
        manager
            .exec_stmt(
                Query::delete()
                    .from_table(RolePermissions::Table)
                    .and_where(Expr::col(RolePermissions::RoleId).is_in(role_ids))
                    .and_where(Expr::col(RolePermissions::PermissionId).is_in(permission_ids))
                    .to_owned(),
            )
            .await
    }
}

impl Migration {
    async fn lookup_permission_ids(
        &self,
        manager: &SchemaManager<'_>,
        domain: &str,
        actions: &[&str],
    ) -> Result<Vec<Uuid>, DbErr> {
        let builder = manager.get_database_backend();
        let mut ids = Vec::new();
        for action in actions {
            let stmt = Query::select()
                .column(Permissions::Id)
                .from(Permissions::Table)
                .and_where(Expr::col(Permissions::Domain).eq(domain))
                .and_where(Expr::col(Permissions::Action).eq(*action))
                .to_owned();
            let rows = manager
                .get_connection()
                .query_all(builder.build(&stmt))
                .await?;
            for row in rows {
                let id: Uuid = row.try_get("", "id")?;
                ids.push(id);
            }
        }
        Ok(ids)
    }

    async fn lookup_role_ids(
        &self,
        manager: &SchemaManager<'_>,
        names: &[&str],
    ) -> Result<Vec<Uuid>, DbErr> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let builder = manager.get_database_backend();
        let stmt = Query::select()
            .column(Roles::Id)
            .from(Roles::Table)
            .and_where(Expr::col(Roles::Name).is_in(names.iter().copied()))
            .to_owned();
        let rows = manager
            .get_connection()
            .query_all(builder.build(&stmt))
            .await?;
        let mut ids = Vec::new();
        for row in rows {
            let id: Uuid = row.try_get("", "id")?;
            ids.push(id);
        }
        Ok(ids)
    }
}

#[derive(DeriveIden)]
enum Permissions {
    #[sea_orm(iden = "permissions")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "domain")]
    Domain,
    #[sea_orm(iden = "action")]
    Action,
}

#[derive(DeriveIden)]
enum Roles {
    #[sea_orm(iden = "roles")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "name")]
    Name,
}

#[derive(DeriveIden)]
enum RolePermissions {
    #[sea_orm(iden = "role_permissions")]
    Table,
    #[sea_orm(iden = "roleId")]
    RoleId,
    #[sea_orm(iden = "permissionId")]
    PermissionId,
}
