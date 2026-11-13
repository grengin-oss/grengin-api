// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("connectors"))
                    .col(
                        ColumnDef::new(Alias::new("id"))
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("userId")).uuid().not_null())
                    .col(
                        ColumnDef::new(Alias::new("connectorKey"))
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("catalogVersion"))
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("serverId")).uuid())
                    .col(ColumnDef::new(Alias::new("status")).string().not_null())
                    .col(ColumnDef::new(Alias::new("encryptedGrant")).text())
                    .col(ColumnDef::new(Alias::new("authorizationUrl")).text())
                    .col(ColumnDef::new(Alias::new("mcpUrl")).text())
                    .col(
                        ColumnDef::new(Alias::new("scopes"))
                            .json_binary()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("expiresAt")).timestamp_with_time_zone())
                    .col(ColumnDef::new(Alias::new("connectedAt")).timestamp_with_time_zone())
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
                            .name("fk_connectors_user")
                            .from(Alias::new("connectors"), Alias::new("userId"))
                            .to(Alias::new("users"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Restrict),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_connectors_server")
                            .from(Alias::new("connectors"), Alias::new("serverId"))
                            .to(Alias::new("mcp_servers"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;
        for (name, column) in [
            ("uq_connectors_user_key", "connectorKey"),
            ("uq_connectors_user_server", "serverId"),
        ] {
            manager
                .create_index(
                    Index::create()
                        .name(name)
                        .table(Alias::new("connectors"))
                        .col(Alias::new("userId"))
                        .col(Alias::new(column))
                        .unique()
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Alias::new("connectors")).to_owned())
            .await
    }
}
