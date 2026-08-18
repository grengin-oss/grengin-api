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
                    .table(SkillKnowledge::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SkillKnowledge::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(SkillKnowledge::SkillId).uuid().not_null())
                    .col(ColumnDef::new(SkillKnowledge::FileId).uuid().null())
                    .col(ColumnDef::new(SkillKnowledge::FileName).text().not_null())
                    .col(ColumnDef::new(SkillKnowledge::Content).text().not_null())
                    .col(
                        ColumnDef::new(SkillKnowledge::CharCount)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(SkillKnowledge::StorageMode)
                            .string_len(20)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(SkillKnowledge::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-skill-knowledge-skill")
                            .from(SkillKnowledge::Table, SkillKnowledge::SkillId)
                            .to(Skills::Table, Skills::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-skill-knowledge-file")
                            .from(SkillKnowledge::Table, SkillKnowledge::FileId)
                            .to(Files::Table, Files::Id)
                            .on_delete(ForeignKeyAction::SetNull)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-skill-knowledge-skill")
                    .table(SkillKnowledge::Table)
                    .col(SkillKnowledge::SkillId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(SkillKnowledge::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum SkillKnowledge {
    #[sea_orm(iden = "skill_knowledge")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "skillId")]
    SkillId,
    #[sea_orm(iden = "fileId")]
    FileId,
    #[sea_orm(iden = "fileName")]
    FileName,
    #[sea_orm(iden = "content")]
    Content,
    #[sea_orm(iden = "charCount")]
    CharCount,
    #[sea_orm(iden = "storageMode")]
    StorageMode,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
}

#[derive(DeriveIden)]
enum Skills {
    #[sea_orm(iden = "skills")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
}

#[derive(DeriveIden)]
enum Files {
    #[sea_orm(iden = "files")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
}
