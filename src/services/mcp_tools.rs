use std::collections::{HashMap, HashSet};

use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{
    dto::llm::openai::OpenaiTool,
    error::AppError,
    llm::tooling::mcp_openai_tool_name,
    models::{mcp_access_policies, mcp_servers, mcp_tools},
    services::mcp_access::{
        build_access_context, load_server_rules, load_tool_rules, resolve_server_access_with_rules,
        resolve_tool_access_with_rules,
    },
    state::SharedState,
};

#[derive(Debug, Clone)]
pub struct McpToolDescriptor {
    pub openai_name: String,
    pub server_id: Uuid,
    pub server_name: String,
    pub server_description: Option<String>,
    pub tool_id: Uuid,
    pub original_name: String,
    pub description: Option<String>,
    pub input_schema: Value,
    pub is_read_only: bool,
    pub permission: mcp_access_policies::McpPermission,
}

#[derive(Debug, Clone)]
pub struct McpServerSummary {
    pub server_id: Uuid,
    pub name: String,
    pub description: Option<String>,
}

pub async fn load_auto_mcp_server_ids(state: &SharedState) -> Result<Vec<Uuid>, AppError> {
    let tools = mcp_tools::Entity::find()
        .filter(mcp_tools::Column::Enabled.eq(true))
        .all(&state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp tools fetch error: {e}");
            AppError::DbTimeout
        })?;
    if tools.is_empty() {
        return Ok(Vec::new());
    }

    let mut server_ids: HashSet<Uuid> = HashSet::new();
    for tool in tools {
        server_ids.insert(tool.server_id);
    }
    let server_ids: Vec<Uuid> = server_ids.into_iter().collect();
    if server_ids.is_empty() {
        return Ok(Vec::new());
    }

    let servers = mcp_servers::Entity::find()
        .filter(mcp_servers::Column::Enabled.eq(true))
        .filter(mcp_servers::Column::Id.is_in(server_ids))
        .all(&state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp servers fetch error: {e}");
            AppError::DbTimeout
        })?;

    Ok(servers.into_iter().map(|server| server.id).collect())
}

fn normalize_openai_parameters(schema: &Value) -> Value {
    fn sanitize_schema(schema: &Value) -> Value {
        let Value::Object(map) = schema else {
            return json!({});
        };
        let mut out = Map::new();
        let allowed_keys = [
            "type",
            "properties",
            "required",
            "additionalProperties",
            "items",
            "oneOf",
            "anyOf",
            "allOf",
            "$defs",
            "definitions",
            "description",
            "title",
            "enum",
            "default",
            "nullable",
            "format",
            "minimum",
            "maximum",
            "minLength",
            "maxLength",
            "pattern",
        ];

        for key in allowed_keys {
            if let Some(value) = map.get(key) {
                let sanitized = match key {
                    "properties" => match value {
                        Value::Object(props) => {
                            let mut props_out = Map::new();
                            for (name, prop_schema) in props {
                                props_out.insert(name.clone(), sanitize_schema(prop_schema));
                            }
                            Value::Object(props_out)
                        }
                        _ => json!({}),
                    },
                    "items" => sanitize_items(value),
                    "oneOf" | "anyOf" | "allOf" => match value {
                        Value::Array(list) => Value::Array(
                            list.iter()
                                .map(|item| sanitize_schema(item))
                                .collect(),
                        ),
                        _ => Value::Array(Vec::new()),
                    },
                    "$defs" | "definitions" => match value {
                        Value::Object(defs) => {
                            let mut defs_out = Map::new();
                            for (name, def_schema) in defs {
                                defs_out.insert(name.clone(), sanitize_schema(def_schema));
                            }
                            Value::Object(defs_out)
                        }
                        _ => json!({}),
                    },
                    "additionalProperties" => match value {
                        Value::Bool(_) => value.clone(),
                        Value::Object(_) => sanitize_schema(value),
                        _ => Value::Bool(false),
                    },
                    "required" => match value {
                        Value::Array(reqs) => Value::Array(
                            reqs.iter()
                                .filter_map(|item| item.as_str().map(|s| Value::String(s.to_string())))
                                .collect(),
                        ),
                        _ => Value::Array(Vec::new()),
                    },
                    "type" => sanitize_type(value),
                    _ => value.clone(),
                };
                out.insert(key.to_string(), sanitized);
            }
        }

        let needs_items = out
            .get("type")
            .and_then(|value| match value {
                Value::String(s) => Some(s == "array"),
                Value::Array(list) => Some(list.iter().any(|item| item.as_str() == Some("array"))),
                _ => None,
            })
            .unwrap_or(false);
        if needs_items {
            let items = out.get("items").cloned().unwrap_or_else(|| json!({}));
            let items = sanitize_items(&items);
            out.insert("items".to_string(), items);
        }

        Value::Object(out)
    }

    fn sanitize_items(value: &Value) -> Value {
        match value {
            Value::Object(_) => sanitize_schema(value),
            Value::Array(list) => list
                .iter()
                .find(|item| item.is_object())
                .map(sanitize_schema)
                .unwrap_or_else(|| json!({})),
            _ => json!({}),
        }
    }

    fn sanitize_type(value: &Value) -> Value {
        match value {
            Value::String(_) => value.clone(),
            Value::Array(list) => {
                let mut out = Vec::new();
                for item in list {
                    if let Some(s) = item.as_str() {
                        out.push(Value::String(s.to_string()));
                    }
                }
                Value::Array(out)
            }
            _ => Value::String("object".to_string()),
        }
    }

    let mut normalized = sanitize_schema(schema);
    let Value::Object(ref mut map) = normalized else {
        return json!({"type":"object","properties":{}});
    };
    if !map.contains_key("type") {
        map.insert("type".to_string(), Value::String("object".to_string()));
    }
    if !map.contains_key("properties") {
        map.insert("properties".to_string(), json!({}));
    }

    normalized
}

pub async fn load_openai_mcp_tools(
    state: &SharedState,
    user_id: Uuid,
    selected_server_ids: &[Uuid],
    selected_tools: &[String],
) -> Result<(Vec<OpenaiTool>, HashMap<String, McpToolDescriptor>, Vec<McpServerSummary>), AppError> {
    let debug = std::env::var("MISTRAL_TOOL_DEBUG").as_deref() == Ok("1");
    if selected_server_ids.is_empty() {
        return Ok((Vec::new(), HashMap::new(), Vec::new()));
    }

    let access_context = build_access_context(&state.database, user_id).await?;

    let servers = mcp_servers::Entity::find()
        .filter(mcp_servers::Column::Id.is_in(selected_server_ids.to_vec()))
        .all(&state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp servers fetch error: {e}");
            AppError::DbTimeout
        })?;
    let mut server_map: HashMap<Uuid, (String, Option<String>, mcp_servers::Model)> = HashMap::new();
    for server in servers {
        server_map.insert(
            server.id,
            (server.name.clone(), server.description.clone(), server),
        );
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

    let server_ids: Vec<Uuid> = server_map.keys().copied().collect();
    let server_rules = load_server_rules(&state.database, &server_ids).await?;
    let mut server_rule_map: HashMap<Uuid, Vec<mcp_access_policies::Model>> = HashMap::new();
    for rule in server_rules {
        if let Some(server_id) = rule.server_id {
            server_rule_map.entry(server_id).or_default().push(rule);
        }
    }

    let tool_ids: Vec<Uuid> = tools.iter().map(|tool| tool.id).collect();
    let tool_rules = load_tool_rules(&state.database, &tool_ids).await?;
    let mut tool_rule_map: HashMap<Uuid, Vec<mcp_access_policies::Model>> = HashMap::new();
    for rule in tool_rules {
        if let Some(tool_id) = rule.tool_id {
            tool_rule_map.entry(tool_id).or_default().push(rule);
        }
    }

    let selected_set: HashSet<String> = selected_tools.iter().cloned().collect();
    let filter_by_selected = !selected_set.is_empty();

    let mut openai_tools = Vec::new();
    let mut lookup = HashMap::new();
    let mut allowed_servers: HashSet<Uuid> = HashSet::new();

    for tool in tools {
        let Some((server_name, server_description, server_model)) = server_map.get(&tool.server_id) else {
            continue;
        };
        let openai_name = mcp_openai_tool_name(&tool.server_id, &tool.name);
        if filter_by_selected
            && !selected_set.contains(&openai_name)
            && !selected_set.contains(&tool.name)
            && !selected_set.contains(&tool.original_name)
        {
            if debug {
                println!(
                    "mcp tools debug: skip tool '{}' (openai='{}') not in selected {:?}",
                    tool.name, openai_name, selected_set
                );
            }
            continue;
        }

        let server_rules = server_rule_map
            .get(&tool.server_id)
            .map(|rules| rules.as_slice())
            .unwrap_or(&[]);
        let server_access = resolve_server_access_with_rules(
            &state.database,
            &access_context,
            server_model,
            server_rules,
        )
        .await?;

        let tool_rules = tool_rule_map
            .get(&tool.id)
            .map(|rules| rules.as_slice())
            .unwrap_or(&[]);
        let tool_access = resolve_tool_access_with_rules(
            &state.database,
            &access_context,
            &tool,
            &server_access,
            tool_rules,
        )
        .await?;

        if tool_access.permission == mcp_access_policies::McpPermission::Denied {
            if debug {
                println!(
                    "mcp tools debug: deny tool '{}' (openai='{}') via {:?}",
                    tool.name, openai_name, tool_access.resolved_via
                );
            }
            continue;
        }
        if tool_access.permission == mcp_access_policies::McpPermission::ReadOnly && !tool.is_read_only {
            if debug {
                println!(
                    "mcp tools debug: read-only deny tool '{}' (openai='{}') is_read_only=false",
                    tool.name, openai_name
                );
            }
            continue;
        }

        let descriptor = McpToolDescriptor {
            openai_name: openai_name.clone(),
            server_id: tool.server_id,
            server_name: server_name.clone(),
            server_description: server_description.clone(),
            tool_id: tool.id,
            original_name: tool.original_name.clone(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
            is_read_only: tool.is_read_only,
            permission: tool_access.permission,
        };

        openai_tools.push(OpenaiTool::Function {
            name: openai_name.clone(),
            description: tool.description.clone(),
            parameters: normalize_openai_parameters(&tool.input_schema),
            strict: None,
        });
        lookup.insert(openai_name, descriptor);
        allowed_servers.insert(tool.server_id);
    }

    let mut server_summaries = Vec::new();
    for server_id in allowed_servers {
        if let Some((name, description, _)) = server_map.get(&server_id) {
            server_summaries.push(McpServerSummary {
                server_id,
                name: name.clone(),
                description: description.clone(),
            });
        }
    }

    Ok((openai_tools, lookup, server_summaries))
}
