// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;
use uuid::Uuid;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("chat_shares"))
                    .col(
                        ColumnDef::new(Alias::new("id"))
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("conversationId"))
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("ownerId")).uuid().not_null())
                    .col(
                        ColumnDef::new(Alias::new("tokenHash"))
                            .string_len(64)
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(Alias::new("token")).text().not_null())
                    .col(
                        ColumnDef::new(Alias::new("access"))
                            .string_len(16)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("snapshot"))
                            .json_binary()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("expiresAt")).timestamp_with_time_zone())
                    .col(ColumnDef::new(Alias::new("revokedAt")).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(Alias::new("createdAt"))
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("updatedAt"))
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_chat_shares_conversation")
                            .from(Alias::new("chat_shares"), Alias::new("conversationId"))
                            .to(Alias::new("conversations"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_chat_shares_owner")
                            .from(Alias::new("chat_shares"), Alias::new("ownerId"))
                            .to(Alias::new("users"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_chat_shares_conversation_owner")
                    .table(Alias::new("chat_shares"))
                    .col(Alias::new("conversationId"))
                    .col(Alias::new("ownerId"))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Alias::new("chat_share_departments"))
                    .col(ColumnDef::new(Alias::new("shareId")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("departmentId")).uuid().not_null())
                    .primary_key(
                        Index::create()
                            .col(Alias::new("shareId"))
                            .col(Alias::new("departmentId")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_chat_share_departments_share")
                            .from(Alias::new("chat_share_departments"), Alias::new("shareId"))
                            .to(Alias::new("chat_shares"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_chat_share_departments_department")
                            .from(
                                Alias::new("chat_share_departments"),
                                Alias::new("departmentId"),
                            )
                            .to(Alias::new("departments"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("notifications"))
                    .add_column(ColumnDef::new(Alias::new("shareId")).uuid())
                    .to_owned(),
            )
            .await?;
        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk_notifications_share_id")
                    .from(Alias::new("notifications"), Alias::new("shareId"))
                    .to(Alias::new("chat_shares"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_notifications_share_user")
                    .unique()
                    .table(Alias::new("notifications"))
                    .col(Alias::new("shareId"))
                    .col(Alias::new("userId"))
                    .to_owned(),
            )
            .await?;

        let backend = manager.get_database_backend();
        manager
            .exec_stmt(
                Query::insert()
                    .into_table(Alias::new("permissions"))
                    .columns(
                        [
                            "id",
                            "domain",
                            "action",
                            "isScopeable",
                            "descriptionKey",
                            "createdAt",
                            "updatedAt",
                        ]
                        .map(Alias::new),
                    )
                    .values_panic([
                        Uuid::new_v4().into(),
                        "chats".into(),
                        "share".into(),
                        false.into(),
                        "permissions.chats.share.description".into(),
                        Expr::current_timestamp().into(),
                        Expr::current_timestamp().into(),
                    ])
                    .on_conflict(
                        OnConflict::columns([Alias::new("domain"), Alias::new("action")])
                            .do_nothing()
                            .to_owned(),
                    )
                    .to_owned(),
            )
            .await?;
        let select = Query::select()
            .column(Alias::new("id"))
            .from(Alias::new("permissions"))
            .and_where(Expr::col(Alias::new("domain")).eq("chats"))
            .and_where(Expr::col(Alias::new("action")).eq("share"))
            .to_owned();
        let permission = manager
            .get_connection()
            .query_one(backend.build(&select))
            .await?
            .ok_or_else(|| DbErr::Custom("chat sharing permission missing".into()))?;
        let permission_id: Uuid = permission.try_get("", "id")?;
        let roles = Query::select()
            .column(Alias::new("id"))
            .from(Alias::new("roles"))
            .and_where(Expr::col(Alias::new("name")).eq("User"))
            .to_owned();
        for row in manager
            .get_connection()
            .query_all(backend.build(&roles))
            .await?
        {
            let role_id: Uuid = row.try_get("", "id")?;
            manager
                .exec_stmt(
                    Query::insert()
                        .into_table(Alias::new("role_permissions"))
                        .columns([Alias::new("roleId"), Alias::new("permissionId")])
                        .values_panic([role_id.into(), permission_id.into()])
                        .on_conflict(
                            OnConflict::columns([Alias::new("roleId"), Alias::new("permissionId")])
                                .do_nothing()
                                .to_owned(),
                        )
                        .to_owned(),
                )
                .await?;
        }
        // Existing cached permission displays must pick up the new User permission.
        manager
            .exec_stmt(
                Query::update()
                    .table(Alias::new("users"))
                    .value(Alias::new("effectivePermissions"), Value::Json(None))
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .exec_stmt(
                Query::delete()
                    .from_table(Alias::new("notifications"))
                    .and_where(Expr::col(Alias::new("shareId")).is_not_null())
                    .to_owned(),
            )
            .await?;
        manager
            .drop_foreign_key(
                ForeignKey::drop()
                    .name("fk_notifications_share_id")
                    .table(Alias::new("notifications"))
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx_notifications_share_user")
                    .table(Alias::new("notifications"))
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("notifications"))
                    .drop_column(Alias::new("shareId"))
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("chat_share_departments"))
                    .to_owned(),
            )
            .await?;
        let permissions = Query::select()
            .column(Alias::new("id"))
            .from(Alias::new("permissions"))
            .and_where(Expr::col(Alias::new("domain")).eq("chats"))
            .and_where(Expr::col(Alias::new("action")).eq("share"))
            .to_owned();
        manager
            .exec_stmt(
                Query::delete()
                    .from_table(Alias::new("role_permissions"))
                    .and_where(Expr::col(Alias::new("permissionId")).in_subquery(permissions))
                    .to_owned(),
            )
            .await?;
        manager
            .exec_stmt(
                Query::delete()
                    .from_table(Alias::new("permissions"))
                    .and_where(Expr::col(Alias::new("domain")).eq("chats"))
                    .and_where(Expr::col(Alias::new("action")).eq("share"))
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(Alias::new("chat_shares")).to_owned())
            .await?;
        manager
            .exec_stmt(
                Query::update()
                    .table(Alias::new("users"))
                    .value(Alias::new("effectivePermissions"), Value::Json(None))
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
