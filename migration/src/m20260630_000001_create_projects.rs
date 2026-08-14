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
                    .table(Projects::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Projects::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Projects::Name).string_len(100).not_null())
                    .col(ColumnDef::new(Projects::Description).string_len(500).null())
                    .col(
                        ColumnDef::new(Projects::Category)
                            .string_len(20)
                            .not_null()
                            .default("research"),
                    )
                    .col(
                        ColumnDef::new(Projects::Visibility)
                            .string_len(10)
                            .not_null()
                            .default("private"),
                    )
                    .col(ColumnDef::new(Projects::OwnerId).uuid().not_null())
                    .col(ColumnDef::new(Projects::Instructions).text().null())
                    .col(
                        ColumnDef::new(Projects::LastActivityAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Projects::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Projects::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-projects-owner-id")
                    .from(Projects::Table, Projects::OwnerId)
                    .to(Users::Table, Users::Id)
                    .on_delete(ForeignKeyAction::Restrict)
                    .on_update(ForeignKeyAction::Restrict)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-projects-owner-id")
                    .table(Projects::Table)
                    .col(Projects::OwnerId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-projects-visibility")
                    .table(Projects::Table)
                    .col(Projects::Visibility)
                    .to_owned(),
            )
            .await?;

        // project_members: explicit cross-department sharing
        manager
            .create_table(
                Table::create()
                    .table(ProjectMembers::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ProjectMembers::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ProjectMembers::ProjectId).uuid().not_null())
                    .col(ColumnDef::new(ProjectMembers::UserId).uuid().not_null())
                    .col(
                        ColumnDef::new(ProjectMembers::Role)
                            .string_len(10)
                            .not_null()
                            .default("member"),
                    )
                    .col(
                        ColumnDef::new(ProjectMembers::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-project-members-project-id")
                    .from(ProjectMembers::Table, ProjectMembers::ProjectId)
                    .to(Projects::Table, Projects::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-project-members-user-id")
                    .from(ProjectMembers::Table, ProjectMembers::UserId)
                    .to(Users::Table, Users::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq-project-members-project-user")
                    .table(ProjectMembers::Table)
                    .col(ProjectMembers::ProjectId)
                    .col(ProjectMembers::UserId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-project-members-user-id")
                    .table(ProjectMembers::Table)
                    .col(ProjectMembers::UserId)
                    .to_owned(),
            )
            .await?;

        // conversation_projects: a chat can reference many projects
        manager
            .create_table(
                Table::create()
                    .table(ConversationProjects::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ConversationProjects::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConversationProjects::ConversationId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationProjects::ProjectId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationProjects::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-conversation-projects-conversation-id")
                    .from(
                        ConversationProjects::Table,
                        ConversationProjects::ConversationId,
                    )
                    .to(Conversations::Table, Conversations::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-conversation-projects-project-id")
                    .from(ConversationProjects::Table, ConversationProjects::ProjectId)
                    .to(Projects::Table, Projects::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq-conversation-projects-conv-proj")
                    .table(ConversationProjects::Table)
                    .col(ConversationProjects::ConversationId)
                    .col(ConversationProjects::ProjectId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-conversation-projects-project-id")
                    .table(ConversationProjects::Table)
                    .col(ConversationProjects::ProjectId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-conversation-projects-conversation-id")
                    .table(ConversationProjects::Table)
                    .col(ConversationProjects::ConversationId)
                    .to_owned(),
            )
            .await?;

        // project_sources: uploaded files and contributed artifacts
        manager
            .create_table(
                Table::create()
                    .table(ProjectSources::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ProjectSources::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ProjectSources::ProjectId).uuid().not_null())
                    .col(ColumnDef::new(ProjectSources::FileName).string().not_null())
                    .col(ColumnDef::new(ProjectSources::FileType).string().not_null())
                    .col(
                        ColumnDef::new(ProjectSources::FileSize)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ProjectSources::Origin)
                            .string_len(10)
                            .not_null()
                            .default("uploaded"),
                    )
                    .col(
                        ColumnDef::new(ProjectSources::UploadedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-project-sources-project-id")
                    .from(ProjectSources::Table, ProjectSources::ProjectId)
                    .to(Projects::Table, Projects::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-project-sources-project-id")
                    .table(ProjectSources::Table)
                    .col(ProjectSources::ProjectId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // project_sources
        manager
            .drop_index(
                Index::drop()
                    .name("idx-project-sources-project-id")
                    .table(ProjectSources::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_foreign_key(
                ForeignKey::drop()
                    .name("fk-project-sources-project-id")
                    .table(ProjectSources::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(ProjectSources::Table).to_owned())
            .await?;

        // conversation_projects
        manager
            .drop_index(
                Index::drop()
                    .name("idx-conversation-projects-conversation-id")
                    .table(ConversationProjects::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx-conversation-projects-project-id")
                    .table(ConversationProjects::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("uq-conversation-projects-conv-proj")
                    .table(ConversationProjects::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_foreign_key(
                ForeignKey::drop()
                    .name("fk-conversation-projects-project-id")
                    .table(ConversationProjects::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_foreign_key(
                ForeignKey::drop()
                    .name("fk-conversation-projects-conversation-id")
                    .table(ConversationProjects::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(ConversationProjects::Table).to_owned())
            .await?;

        // project_members
        manager
            .drop_index(
                Index::drop()
                    .name("idx-project-members-user-id")
                    .table(ProjectMembers::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("uq-project-members-project-user")
                    .table(ProjectMembers::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_foreign_key(
                ForeignKey::drop()
                    .name("fk-project-members-user-id")
                    .table(ProjectMembers::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_foreign_key(
                ForeignKey::drop()
                    .name("fk-project-members-project-id")
                    .table(ProjectMembers::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(ProjectMembers::Table).to_owned())
            .await?;

        // projects
        manager
            .drop_index(
                Index::drop()
                    .name("idx-projects-visibility")
                    .table(Projects::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx-projects-owner-id")
                    .table(Projects::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_foreign_key(
                ForeignKey::drop()
                    .name("fk-projects-owner-id")
                    .table(Projects::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(Projects::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Projects {
    #[sea_orm(iden = "projects")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "name")]
    Name,
    #[sea_orm(iden = "description")]
    Description,
    #[sea_orm(iden = "category")]
    Category,
    #[sea_orm(iden = "visibility")]
    Visibility,
    #[sea_orm(iden = "ownerId")]
    OwnerId,
    #[sea_orm(iden = "instructions")]
    Instructions,
    #[sea_orm(iden = "lastActivityAt")]
    LastActivityAt,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "updatedAt")]
    UpdatedAt,
}

#[derive(DeriveIden)]
enum ProjectMembers {
    #[sea_orm(iden = "project_members")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "projectId")]
    ProjectId,
    #[sea_orm(iden = "userId")]
    UserId,
    #[sea_orm(iden = "role")]
    Role,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
}

#[derive(DeriveIden)]
enum ConversationProjects {
    #[sea_orm(iden = "conversation_projects")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "conversationId")]
    ConversationId,
    #[sea_orm(iden = "projectId")]
    ProjectId,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
}

#[derive(DeriveIden)]
enum ProjectSources {
    #[sea_orm(iden = "project_sources")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "projectId")]
    ProjectId,
    #[sea_orm(iden = "fileName")]
    FileName,
    #[sea_orm(iden = "fileType")]
    FileType,
    #[sea_orm(iden = "fileSize")]
    FileSize,
    #[sea_orm(iden = "origin")]
    Origin,
    #[sea_orm(iden = "uploadedAt")]
    UploadedAt,
}

// References to existing tables
#[derive(DeriveIden)]
enum Users {
    #[sea_orm(iden = "users")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
}

#[derive(DeriveIden)]
enum Conversations {
    #[sea_orm(iden = "conversations")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
}
