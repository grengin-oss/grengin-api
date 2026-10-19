// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum RolePrompts {
    #[sea_orm(iden = "role_prompts")]
    Table,
    #[sea_orm(iden = "name")]
    Name,
    #[sea_orm(iden = "promptText")]
    PromptText,
    #[sea_orm(iden = "isSystem")]
    IsSystem,
    #[sea_orm(iden = "createdBy")]
    CreatedBy,
}

// The persona templates seeded by m20260407_000001_create_system_prompts, as seeded. Each was a
// default for its own role, but m20260415_000001_role_prompts_role_id moved all five onto the User
// role, so every user without a prompt of their own got whichever one Postgres returned first.
// They stay in the prompt library and apply only when an admin or the user turns one on.
const SEEDED_PERSONAS: [(&str, &str); 5] = [
    (
        "Developer",
        "You are a senior software developer at {{company_name}}. Provide precise, practical guidance. Ask clarifying questions when needed. Use the user's context and avoid unnecessary verbosity. Address the user by name when appropriate ({{user_name}}).",
    ),
    (
        "Marketer",
        "You are a performance marketer at {{company_name}}. Craft clear, persuasive messaging and campaigns. Tailor recommendations to the {{department}} team's goals. Ask for missing details when necessary.",
    ),
    (
        "Sales",
        "You are a sales specialist at {{company_name}}. Write concise, value-focused responses. Adapt tone to the prospect and include next-step suggestions.",
    ),
    (
        "Support",
        "You are a customer support specialist at {{company_name}}. Be empathetic, clear, and solution-oriented. Ask for the minimum details required to resolve issues.",
    ),
    (
        "Analyst",
        "You are a data analyst at {{company_name}}. Provide structured analysis, highlight assumptions, and suggest next actions. Use concise summaries and bullet points when helpful.",
    ),
];

// Only the untouched seed rows: an admin-created prompt always has a creator, and an edited one no
// longer has the seeded text.
fn mark_seeded_personas(name: &str, prompt_text: &str, is_system: bool) -> UpdateStatement {
    Query::update()
        .table(RolePrompts::Table)
        .value(RolePrompts::IsSystem, is_system)
        .and_where(Expr::col(RolePrompts::Name).eq(name))
        .and_where(Expr::col(RolePrompts::PromptText).eq(prompt_text))
        .and_where(Expr::col(RolePrompts::CreatedBy).is_null())
        .to_owned()
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (name, prompt_text) in SEEDED_PERSONAS {
            manager
                .exec_stmt(mark_seeded_personas(name, prompt_text, false))
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (name, prompt_text) in SEEDED_PERSONAS {
            manager
                .exec_stmt(mark_seeded_personas(name, prompt_text, true))
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_untouched_seed_rows_are_changed() {
        let sql =
            mark_seeded_personas("Support", "seeded text", false).to_string(PostgresQueryBuilder);
        assert_eq!(
            sql,
            r#"UPDATE "role_prompts" SET "isSystem" = FALSE WHERE "name" = 'Support' AND "promptText" = 'seeded text' AND "createdBy" IS NULL"#
        );
    }
}
