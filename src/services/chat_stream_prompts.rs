use std::collections::HashMap;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect};
use uuid::Uuid;

use crate::{
    llm::prompt::Prompt,
    models::{conversation_projects, messages::ChatRole, projects},
    services::{
        artifacts::ARTIFACT_SYSTEM_HINT,
        mcp_helpers::build_mcp_server_context,
        mcp_tools::{McpServerSummary, McpToolDescriptor},
    },
};

pub async fn inject_project_instructions(
    prompts: &mut Vec<Prompt>,
    db: &DatabaseConnection,
    conversation_id: Uuid,
) {
    let project_ids: Vec<Uuid> = conversation_projects::Entity::find()
        .select_only()
        .column(conversation_projects::Column::ProjectId)
        .filter(conversation_projects::Column::ConversationId.eq(conversation_id))
        .into_tuple::<Uuid>()
        .all(db)
        .await
        .unwrap_or_default();

    if project_ids.is_empty() {
        return;
    }

    let linked_projects = projects::Entity::find()
        .filter(projects::Column::Id.is_in(project_ids))
        .all(db)
        .await
        .unwrap_or_default();

    let mut project_blocks: Vec<String> = linked_projects
        .into_iter()
        .filter_map(|p| {
            let instr = p.instructions?;
            let trimmed = instr.trim().to_string();
            if trimmed.is_empty() {
                return None;
            }
            Some(format!("## Project: {}\n{}", p.name, trimmed))
        })
        .collect();

    if project_blocks.is_empty() {
        return;
    }

    project_blocks.sort();
    let project_text = format!(
        "You are working within the context of the following project(s). Follow their instructions carefully.\n\n{}",
        project_blocks.join("\n\n---\n\n")
    );

    if let Some(existing) = prompts.iter_mut().find(|p| p.role == ChatRole::System) {
        existing.text = format!("{}\n\n---\n\n{}", project_text, existing.text);
    } else {
        prompts.insert(0, Prompt {
            role: ChatRole::System,
            text: project_text,
            files: Vec::new(),
        });
    }
}

pub fn inject_artifact_hint(prompts: &mut Vec<Prompt>) {
    if let Some(sys) = prompts.iter_mut().find(|p| p.role == ChatRole::System) {
        sys.text = format!("{}\n\n{}", sys.text, ARTIFACT_SYSTEM_HINT);
    } else {
        prompts.insert(0, Prompt {
            role: ChatRole::System,
            text: ARTIFACT_SYSTEM_HINT.to_string(),
            files: Vec::new(),
        });
    }
}

pub fn inject_mcp_context(
    prompts: &mut Vec<Prompt>,
    summaries: &[McpServerSummary],
    tool_lookup: &HashMap<String, McpToolDescriptor>,
) {
    if let Some(context) = build_mcp_server_context(summaries, tool_lookup) {
        let insert_at = prompts
            .iter()
            .position(|p| p.role != ChatRole::System)
            .unwrap_or(prompts.len());
        prompts.insert(insert_at, Prompt {
            role: ChatRole::System,
            text: context,
            files: Vec::new(),
        });
    }
}
