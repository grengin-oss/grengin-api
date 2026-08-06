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
                    .table(ProjectMcpServers::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ProjectMcpServers::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ProjectMcpServers::ProjectId).uuid().not_null())
                    .col(ColumnDef::new(ProjectMcpServers::ServerId).uuid().not_null())
                    .col(
                        ColumnDef::new(ProjectMcpServers::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-project-mcp-servers-project-id")
                    .from(ProjectMcpServers::Table, ProjectMcpServers::ProjectId)
                    .to(Projects::Table, Projects::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-project-mcp-servers-server-id")
                    .from(ProjectMcpServers::Table, ProjectMcpServers::ServerId)
                    .to(McpServers::Table, McpServers::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq-project-mcp-servers-project-server")
                    .table(ProjectMcpServers::Table)
                    .col(ProjectMcpServers::ProjectId)
                    .col(ProjectMcpServers::ServerId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-project-mcp-servers-project-id")
                    .table(ProjectMcpServers::Table)
                    .col(ProjectMcpServers::ProjectId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx-project-mcp-servers-project-id")
                    .table(ProjectMcpServers::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("uq-project-mcp-servers-project-server")
                    .table(ProjectMcpServers::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_foreign_key(
                ForeignKey::drop()
                    .name("fk-project-mcp-servers-server-id")
                    .table(ProjectMcpServers::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_foreign_key(
                ForeignKey::drop()
                    .name("fk-project-mcp-servers-project-id")
                    .table(ProjectMcpServers::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(ProjectMcpServers::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ProjectMcpServers {
    #[sea_orm(iden = "project_mcp_servers")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "projectId")]
    ProjectId,
    #[sea_orm(iden = "serverId")]
    ServerId,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
}

#[derive(DeriveIden)]
enum Projects {
    #[sea_orm(iden = "projects")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
}

#[derive(DeriveIden)]
enum McpServers {
    #[sea_orm(iden = "mcp_servers")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
}
