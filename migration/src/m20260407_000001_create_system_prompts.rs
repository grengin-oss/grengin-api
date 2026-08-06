// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;
use serde_json::json;
use uuid::Uuid;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum RolePrompts {
    #[sea_orm(iden = "role_prompts")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "name")]
    Name,
    #[sea_orm(iden = "role")]
    Role,
    #[sea_orm(iden = "promptText")]
    PromptText,
    #[sea_orm(iden = "variables")]
    Variables,
    #[sea_orm(iden = "isSystem")]
    IsSystem,
    #[sea_orm(iden = "createdBy")]
    CreatedBy,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "updatedAt")]
    UpdatedAt,
    #[sea_orm(iden = "usageCount")]
    UsageCount,
}

#[derive(DeriveIden)]
enum DepartmentPromptAssignments {
    #[sea_orm(iden = "department_prompt_assignments")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "departmentId")]
    DepartmentId,
    #[sea_orm(iden = "promptId")]
    PromptId,
    #[sea_orm(iden = "priority")]
    Priority,
    #[sea_orm(iden = "assignedBy")]
    AssignedBy,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "updatedAt")]
    UpdatedAt,
}

#[derive(DeriveIden)]
enum UserPromptPreferences {
    #[sea_orm(iden = "user_prompt_preferences")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "userId")]
    UserId,
    #[sea_orm(iden = "promptId")]
    PromptId,
    #[sea_orm(iden = "customPromptText")]
    CustomPromptText,
    #[sea_orm(iden = "isActive")]
    IsActive,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "updatedAt")]
    UpdatedAt,
}

#[derive(DeriveIden)]
enum PromptFeedback {
    #[sea_orm(iden = "prompt_feedback")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "userId")]
    UserId,
    #[sea_orm(iden = "promptId")]
    PromptId,
    #[sea_orm(iden = "rating")]
    Rating,
    #[sea_orm(iden = "comment")]
    Comment,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
}

#[derive(DeriveIden)]
enum Users {
    #[sea_orm(iden = "users")]
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

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(RolePrompts::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(RolePrompts::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(RolePrompts::Name).string().not_null())
                    .col(ColumnDef::new(RolePrompts::Role).string().not_null())
                    .col(ColumnDef::new(RolePrompts::PromptText).text().not_null())
                    .col(ColumnDef::new(RolePrompts::Variables).json_binary().null())
                    .col(ColumnDef::new(RolePrompts::IsSystem).boolean().not_null())
                    .col(ColumnDef::new(RolePrompts::CreatedBy).uuid().null())
                    .col(
                        ColumnDef::new(RolePrompts::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RolePrompts::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(RolePrompts::UsageCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .from(RolePrompts::Table, RolePrompts::CreatedBy)
                    .to(Users::Table, Users::Id)
                    .on_delete(ForeignKeyAction::SetNull)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_role_prompts_role")
                    .table(RolePrompts::Table)
                    .col(RolePrompts::Role)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_role_prompts_isSystem")
                    .table(RolePrompts::Table)
                    .col(RolePrompts::IsSystem)
                    .to_owned(),
            )
            .await?;

        let default_prompts = [
            (
                "Developer",
                "developer",
                "You are a senior software developer at {{company_name}}. Provide precise, practical guidance. Ask clarifying questions when needed. Use the user's context and avoid unnecessary verbosity. Address the user by name when appropriate ({{user_name}}).",
            ),
            (
                "Marketer",
                "marketer",
                "You are a performance marketer at {{company_name}}. Craft clear, persuasive messaging and campaigns. Tailor recommendations to the {{department}} team's goals. Ask for missing details when necessary.",
            ),
            (
                "Sales",
                "sales",
                "You are a sales specialist at {{company_name}}. Write concise, value-focused responses. Adapt tone to the prospect and include next-step suggestions.",
            ),
            (
                "Support",
                "support",
                "You are a customer support specialist at {{company_name}}. Be empathetic, clear, and solution-oriented. Ask for the minimum details required to resolve issues.",
            ),
            (
                "Analyst",
                "analyst",
                "You are a data analyst at {{company_name}}. Provide structured analysis, highlight assumptions, and suggest next actions. Use concise summaries and bullet points when helpful.",
            ),
        ];
        let variables = json!(["user_name", "department", "company_name"]);
        let now = Expr::current_timestamp();
        for (name, role, prompt_text) in default_prompts {
            let mut insert = Query::insert();
            insert
                .into_table(RolePrompts::Table)
                .columns([
                    RolePrompts::Id,
                    RolePrompts::Name,
                    RolePrompts::Role,
                    RolePrompts::PromptText,
                    RolePrompts::Variables,
                    RolePrompts::IsSystem,
                    RolePrompts::CreatedBy,
                    RolePrompts::CreatedAt,
                    RolePrompts::UpdatedAt,
                    RolePrompts::UsageCount,
                ])
                .values_panic([
                    Expr::val(Uuid::new_v4()).into(),
                    Expr::val(name).into(),
                    Expr::val(role).into(),
                    Expr::val(prompt_text).into(),
                    Expr::val(variables.clone()).into(),
                    Expr::val(true).into(),
                    Expr::val(Option::<Uuid>::None).into(),
                    now.clone().into(),
                    now.clone().into(),
                    Expr::val(0).into(),
                ]);
            manager.exec_stmt(insert).await?;
        }

        manager
            .create_table(
                Table::create()
                    .table(DepartmentPromptAssignments::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(DepartmentPromptAssignments::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(DepartmentPromptAssignments::DepartmentId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DepartmentPromptAssignments::PromptId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DepartmentPromptAssignments::Priority)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DepartmentPromptAssignments::AssignedBy)
                            .uuid()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(DepartmentPromptAssignments::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DepartmentPromptAssignments::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .from(
                        DepartmentPromptAssignments::Table,
                        DepartmentPromptAssignments::DepartmentId,
                    )
                    .to(Departments::Table, Departments::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;
        manager
            .create_foreign_key(
                ForeignKey::create()
                    .from(
                        DepartmentPromptAssignments::Table,
                        DepartmentPromptAssignments::PromptId,
                    )
                    .to(RolePrompts::Table, RolePrompts::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;
        manager
            .create_foreign_key(
                ForeignKey::create()
                    .from(
                        DepartmentPromptAssignments::Table,
                        DepartmentPromptAssignments::AssignedBy,
                    )
                    .to(Users::Table, Users::Id)
                    .on_delete(ForeignKeyAction::SetNull)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_department_prompts_department_priority")
                    .table(DepartmentPromptAssignments::Table)
                    .col(DepartmentPromptAssignments::DepartmentId)
                    .col(DepartmentPromptAssignments::Priority)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("uniq_department_prompt")
                    .table(DepartmentPromptAssignments::Table)
                    .col(DepartmentPromptAssignments::DepartmentId)
                    .col(DepartmentPromptAssignments::PromptId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(UserPromptPreferences::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(UserPromptPreferences::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(UserPromptPreferences::UserId)
                            .uuid()
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(UserPromptPreferences::PromptId)
                            .uuid()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(UserPromptPreferences::CustomPromptText)
                            .text()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(UserPromptPreferences::IsActive)
                            .boolean()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UserPromptPreferences::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UserPromptPreferences::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .from(UserPromptPreferences::Table, UserPromptPreferences::UserId)
                    .to(Users::Table, Users::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;
        manager
            .create_foreign_key(
                ForeignKey::create()
                    .from(
                        UserPromptPreferences::Table,
                        UserPromptPreferences::PromptId,
                    )
                    .to(RolePrompts::Table, RolePrompts::Id)
                    .on_delete(ForeignKeyAction::SetNull)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(PromptFeedback::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PromptFeedback::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(PromptFeedback::UserId).uuid().not_null())
                    .col(ColumnDef::new(PromptFeedback::PromptId).uuid().null())
                    .col(ColumnDef::new(PromptFeedback::Rating).integer().not_null())
                    .col(ColumnDef::new(PromptFeedback::Comment).text().null())
                    .col(
                        ColumnDef::new(PromptFeedback::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .from(PromptFeedback::Table, PromptFeedback::UserId)
                    .to(Users::Table, Users::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;
        manager
            .create_foreign_key(
                ForeignKey::create()
                    .from(PromptFeedback::Table, PromptFeedback::PromptId)
                    .to(RolePrompts::Table, RolePrompts::Id)
                    .on_delete(ForeignKeyAction::SetNull)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_prompt_feedback_prompt")
                    .table(PromptFeedback::Table)
                    .col(PromptFeedback::PromptId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PromptFeedback::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(UserPromptPreferences::Table).to_owned())
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(DepartmentPromptAssignments::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(RolePrompts::Table).to_owned())
            .await
    }
}
