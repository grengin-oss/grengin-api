// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;
use uuid::Uuid;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let mut insert_permissions = Query::insert();
        insert_permissions
            .into_table(Permissions::Table)
            .columns([
                Permissions::Id,
                Permissions::Domain,
                Permissions::Action,
                Permissions::IsScopeable,
                Permissions::DescriptionKey,
                Permissions::CreatedAt,
                Permissions::UpdatedAt,
            ])
            .values_panic([
                Uuid::new_v4().into(),
                "skills".into(),
                "view".into(),
                false.into(),
                "permissions.skills.view.description".into(),
                Expr::current_timestamp().into(),
                Expr::current_timestamp().into(),
            ])
            .values_panic([
                Uuid::new_v4().into(),
                "skills".into(),
                "manage".into(),
                false.into(),
                "permissions.skills.manage.description".into(),
                Expr::current_timestamp().into(),
                Expr::current_timestamp().into(),
            ])
            .on_conflict(
                OnConflict::columns([Permissions::Domain, Permissions::Action])
                    .do_nothing()
                    .to_owned(),
            );
        manager.exec_stmt(insert_permissions).await?;

        let permission_ids = self
            .lookup_permission_ids(manager, "skills", &["view", "manage"])
            .await?;
        if permission_ids.is_empty() {
            return Ok(());
        }

        let role_ids = self
            .lookup_role_ids(manager, &["Super Admin", "IT Admin"])
            .await?;
        if role_ids.is_empty() {
            return Ok(());
        }

        let mut insert_role_permissions = Query::insert();
        insert_role_permissions
            .into_table(RolePermissions::Table)
            .columns([RolePermissions::RoleId, RolePermissions::PermissionId])
            .on_conflict(
                OnConflict::columns([RolePermissions::RoleId, RolePermissions::PermissionId])
                    .do_nothing()
                    .to_owned(),
            );
        for role_id in role_ids {
            for permission_id in &permission_ids {
                insert_role_permissions.values_panic([role_id.into(), (*permission_id).into()]);
            }
        }
        manager.exec_stmt(insert_role_permissions).await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let permission_ids = self
            .lookup_permission_ids(manager, "skills", &["view", "manage"])
            .await?;

        if !permission_ids.is_empty() {
            manager
                .exec_stmt(
                    Query::delete()
                        .from_table(RolePermissions::Table)
                        .and_where(
                            Expr::col(RolePermissions::PermissionId).is_in(permission_ids.clone()),
                        )
                        .to_owned(),
                )
                .await?;
        }

        manager
            .exec_stmt(
                Query::delete()
                    .from_table(Permissions::Table)
                    .and_where(Expr::col(Permissions::Domain).eq("skills"))
                    .to_owned(),
            )
            .await?;

        Ok(())
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
    #[sea_orm(iden = "isScopeable")]
    IsScopeable,
    #[sea_orm(iden = "descriptionKey")]
    DescriptionKey,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "updatedAt")]
    UpdatedAt,
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
