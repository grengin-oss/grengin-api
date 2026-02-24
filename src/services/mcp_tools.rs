use std::collections::{HashMap, HashSet};

use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    dto::llm::openai::OpenaiTool,
    error::AppError,
    llm::tooling::mcp_openai_tool_name,
    models::mcp_tools,
    state::SharedState,
};

#[derive(Debug, Clone)]
pub struct McpToolDescriptor {
    pub openai_name: String,
    pub server_id: Uuid,
    pub server_name: String,
    pub tool_id: Uuid,
    pub original_name: String,
    pub description: Option<String>,
    pub input_schema: Value,
    pub is_read_only: bool,
}

pub async fn load_openai_mcp_tools(
    state: &SharedState,
    selected_server_ids: &[Uuid],
    selected_tools: &[String],
) -> Result<(Vec<OpenaiTool>, HashMap<String, McpToolDescriptor>), AppError> {
    if selected_server_ids.is_empty() {
        return Ok((Vec::new(), HashMap::new()));
    }

    let tools = mcp_tools::Entity::find()
        .filter(mcp_tools::Column::Enabled.eq(true))
        .filter(mcp_tools::Column::ServerId.is_in(selected_server_ids.to_vec()))
        .all(&state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp tools fetch error: {e}");
            AppError::DbTimeout
        })?;

    let selected_set: HashSet<String> = selected_tools.iter().cloned().collect();
    let filter_by_selected = !selected_set.is_empty();

    let mut openai_tools = Vec::new();
    let mut lookup = HashMap::new();

    for tool in tools {
        let openai_name = mcp_openai_tool_name(&tool.server_id, &tool.name);
        if filter_by_selected
            && !selected_set.contains(&openai_name)
            && !selected_set.contains(&tool.name)
            && !selected_set.contains(&tool.original_name)
        {
            continue;
        }

        let descriptor = McpToolDescriptor {
            openai_name: openai_name.clone(),
            server_id: tool.server_id,
            server_name: tool.server_name.clone(),
            tool_id: tool.id,
            original_name: tool.original_name.clone(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
            is_read_only: tool.is_read_only,
        };

        openai_tools.push(OpenaiTool::Function {
            name: openai_name.clone(),
            description: tool.description.clone(),
            parameters: tool.input_schema.clone(),
            strict: None,
        });
        lookup.insert(openai_name, descriptor);
    }

    Ok((openai_tools, lookup))
}
