use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Skills::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Skills::Id).uuid().not_null().primary_key())
                    .col(
                        ColumnDef::new(Skills::Identifier)
                            .string_len(100)
                            .not_null(),
                    )
                    .col(ColumnDef::new(Skills::Name).string_len(100).not_null())
                    .col(ColumnDef::new(Skills::Description).string_len(500).null())
                    .col(ColumnDef::new(Skills::Avatar).string_len(255).null())
                    .col(ColumnDef::new(Skills::SystemRole).text().null())
                    .col(ColumnDef::new(Skills::ToolsConfig).json_binary().null())
                    .col(
                        ColumnDef::new(Skills::IsBuiltin)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(Skills::IsActive)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(ColumnDef::new(Skills::DepartmentId).uuid().null())
                    .col(
                        ColumnDef::new(Skills::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Skills::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq-skills-identifier")
                    .table(Skills::Table)
                    .col(Skills::Identifier)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-skills-department-id")
                    .table(Skills::Table)
                    .col(Skills::DepartmentId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-skills-is-active")
                    .table(Skills::Table)
                    .col(Skills::IsActive)
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-skills-department-id")
                    .from(Skills::Table, Skills::DepartmentId)
                    .to(Departments::Table, Departments::Id)
                    .on_delete(ForeignKeyAction::SetNull)
                    .on_update(ForeignKeyAction::Restrict)
                    .to_owned(),
            )
            .await?;

        // conversation_skills: skills attached to a conversation
        manager
            .create_table(
                Table::create()
                    .table(ConversationSkills::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ConversationSkills::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConversationSkills::ConversationId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationSkills::SkillId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationSkills::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-conversation-skills-conversation-id")
                    .from(ConversationSkills::Table, ConversationSkills::ConversationId)
                    .to(Conversations::Table, Conversations::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::Restrict)
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-conversation-skills-skill-id")
                    .from(ConversationSkills::Table, ConversationSkills::SkillId)
                    .to(Skills::Table, Skills::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::Restrict)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq-conversation-skills-conv-skill")
                    .table(ConversationSkills::Table)
                    .col(ConversationSkills::ConversationId)
                    .col(ConversationSkills::SkillId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-conversation-skills-conversation-id")
                    .table(ConversationSkills::Table)
                    .col(ConversationSkills::ConversationId)
                    .to_owned(),
            )
            .await?;

        // Seed built-in artifact-create skill using typed query builder
        let now = chrono::Utc::now();
        let tools_config = serde_json::json!({"webSearch": false, "mcpServerIds": []});
        manager
            .exec_stmt(
                Query::insert()
                    .into_table(Skills::Table)
                    .columns([
                        Skills::Id,
                        Skills::Identifier,
                        Skills::Name,
                        Skills::Description,
                        Skills::Avatar,
                        Skills::SystemRole,
                        Skills::ToolsConfig,
                        Skills::IsBuiltin,
                        Skills::IsActive,
                        Skills::DepartmentId,
                        Skills::CreatedAt,
                        Skills::UpdatedAt,
                    ])
                    .values_panic([
                        uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000001")
                            .unwrap()
                            .into(),
                        "artifact-create".into(),
                        "Artifact Creator".into(),
                        "Enables the model to produce structured artifacts such as code, documents, and diagrams.".into(),
                        "🎨".into(),
                        Option::<String>::None.into(),
                        tools_config.into(),
                        true.into(),
                        true.into(),
                        Option::<uuid::Uuid>::None.into(),
                        now.into(),
                        now.into(),
                    ])
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // conversation_skills
        manager
            .drop_index(
                Index::drop()
                    .name("idx-conversation-skills-conversation-id")
                    .table(ConversationSkills::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("uq-conversation-skills-conv-skill")
                    .table(ConversationSkills::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_foreign_key(
                ForeignKey::drop()
                    .name("fk-conversation-skills-skill-id")
                    .table(ConversationSkills::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_foreign_key(
                ForeignKey::drop()
                    .name("fk-conversation-skills-conversation-id")
                    .table(ConversationSkills::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(ConversationSkills::Table).to_owned())
            .await?;

        // skills
        manager
            .drop_foreign_key(
                ForeignKey::drop()
                    .name("fk-skills-department-id")
                    .table(Skills::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx-skills-is-active")
                    .table(Skills::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx-skills-department-id")
                    .table(Skills::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("uq-skills-identifier")
                    .table(Skills::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(Skills::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Skills {
    #[sea_orm(iden = "skills")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "identifier")]
    Identifier,
    #[sea_orm(iden = "name")]
    Name,
    #[sea_orm(iden = "description")]
    Description,
    #[sea_orm(iden = "avatar")]
    Avatar,
    #[sea_orm(iden = "systemRole")]
    SystemRole,
    #[sea_orm(iden = "toolsConfig")]
    ToolsConfig,
    #[sea_orm(iden = "isBuiltin")]
    IsBuiltin,
    #[sea_orm(iden = "isActive")]
    IsActive,
    #[sea_orm(iden = "departmentId")]
    DepartmentId,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "updatedAt")]
    UpdatedAt,
}

#[derive(DeriveIden)]
enum ConversationSkills {
    #[sea_orm(iden = "conversation_skills")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "conversationId")]
    ConversationId,
    #[sea_orm(iden = "skillId")]
    SkillId,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
}

#[derive(DeriveIden)]
enum Conversations {
    #[sea_orm(iden = "conversations")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
}

#[derive(DeriveIden)]
enum Departments {
    #[sea_orm(iden = "departments")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
}
