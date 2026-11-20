// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::collections::{HashMap, HashSet};

use openssl::sha::sha256;
use rmcp::model::CallToolResult;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{
    error::AppError,
    models::{mcp_access_policies, mcp_servers, mcp_tools, users},
    services::{
        mcp_access::{
            McpAccessResolver, ResolvedAccess, build_access_context, load_server_rules,
            load_tool_rules, resolve_server_access_with_rules, resolve_tool_access_with_rules,
        },
        mcp_client::{McpClientError, McpServerClient},
    },
    state::SharedState,
};

pub fn sanitize_tool_name(name: &str) -> String {
    let mut sanitized = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            sanitized.push(ch.to_ascii_lowercase());
        } else {
            sanitized.push('_');
        }
    }
    sanitized
}

pub fn mcp_server_short_id(server_id: &Uuid) -> String {
    let compact = server_id.to_string().replace('-', "");
    compact.chars().take(8).collect()
}

pub fn mcp_tool_name(server_id: &Uuid, tool_name: &str) -> String {
    let server_part = mcp_server_short_id(server_id);
    let mut tool_part = sanitize_tool_name(tool_name);
    if tool_part.is_empty() {
        tool_part = "tool".to_string();
    }
    let digest = sha256(format!("{server_id}:{tool_name}").as_bytes());
    let hash = digest
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let prefix = format!("mcp__{server_part}__");
    let suffix = format!("__{hash}");
    let max_tool_len = 64usize.saturating_sub(prefix.len() + suffix.len()).max(1);
    if tool_part.len() > max_tool_len {
        tool_part.truncate(max_tool_len);
    }
    format!("{prefix}{tool_part}{suffix}")
}

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
                        Value::Array(list) => {
                            Value::Array(list.iter().map(|item| sanitize_schema(item)).collect())
                        }
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
                                .filter_map(|item| {
                                    item.as_str().map(|s| Value::String(s.to_string()))
                                })
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

pub fn tool_is_usable(
    permission: mcp_access_policies::McpPermission,
    tool_is_read_only: bool,
) -> bool {
    match permission {
        mcp_access_policies::McpPermission::Full => true,
        mcp_access_policies::McpPermission::ReadOnly => tool_is_read_only,
        mcp_access_policies::McpPermission::Denied => false,
    }
}

pub async fn filter_tools_for_user(
    db: &DatabaseConnection,
    user_id: Uuid,
    tools: Vec<mcp_tools::Model>,
) -> Result<Vec<mcp_tools::Model>, AppError> {
    if tools.is_empty() {
        return Ok(tools);
    }
    let server_ids: Vec<Uuid> = tools
        .iter()
        .map(|tool| tool.server_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let servers = mcp_servers::Entity::find()
        .filter(mcp_servers::Column::Id.is_in(server_ids.clone()))
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("mcp servers fetch error: {e}");
            AppError::DbTimeout
        })?;
    Ok(resolve_listing_access(db, user_id, &servers, tools)
        .await?
        .tools)
}

pub async fn call_with_current_access(
    db: &DatabaseConnection,
    client: &McpServerClient,
    tool_name: &str,
    arguments: Value,
    token: Option<String>,
    user: Uuid,
) -> Result<CallToolResult, McpClientError> {
    ensure_current_access(db, client, tool_name, user).await?;
    client
        .call_tool(tool_name, arguments, token, Some(user))
        .await
}

// Tool access is resolved once when a chat request starts; a user, server, tool or rule can be
// revoked while that request is still calling tools, so every dispatch re-reads it, whatever the transport.
async fn ensure_current_access(
    db: &DatabaseConnection,
    client: &McpServerClient,
    tool_name: &str,
    user: Uuid,
) -> Result<(), McpClientError> {
    let unavailable = |_| McpClientError::Config("mcp authorization unavailable".into());
    let denied = || McpClientError::Config("mcp access denied or configuration changed".into());
    let owner = users::Entity::find_by_id(user)
        .one(db)
        .await
        .map_err(unavailable)?
        .ok_or_else(denied)?;
    if owner.status != users::UserStatus::Active {
        return Err(denied());
    }
    let server = mcp_servers::Entity::find_by_id(client.server_id)
        .one(db)
        .await
        .map_err(unavailable)?
        .ok_or_else(denied)?;
    if !server.enabled
        || server.transport_type != client.transport_type
        || server.url != client.url
        || server.connection_config != client.connection_config
    {
        return Err(denied());
    }
    let tool = mcp_tools::Entity::find()
        .filter(mcp_tools::Column::ServerId.eq(client.server_id))
        .filter(mcp_tools::Column::OriginalName.eq(tool_name))
        .filter(mcp_tools::Column::Enabled.eq(true))
        .one(db)
        .await
        .map_err(unavailable)?
        .ok_or_else(denied)?;
    let usable = resolve_listing_access(db, user, &[server], vec![tool])
        .await
        .map_err(|_| denied())?;
    if usable.tools.is_empty() {
        return Err(denied());
    }
    Ok(())
}

pub struct McpListingAccess {
    pub server_access: HashMap<Uuid, ResolvedAccess>,
    pub tools: Vec<mcp_tools::Model>,
}

pub async fn resolve_listing_access(
    db: &DatabaseConnection,
    user_id: Uuid,
    servers: &[mcp_servers::Model],
    tools: Vec<mcp_tools::Model>,
) -> Result<McpListingAccess, AppError> {
    if servers.is_empty() {
        return Ok(McpListingAccess {
            server_access: HashMap::new(),
            tools: Vec::new(),
        });
    }
    let access_context = build_access_context(db, user_id).await?;
    let server_ids: Vec<_> = servers.iter().map(|server| server.id).collect();
    let tool_ids: Vec<_> = tools.iter().map(|tool| tool.id).collect();
    let mut rules = load_server_rules(db, &server_ids).await?;
    rules.extend(load_tool_rules(db, &tool_ids).await?);
    let resolver = McpAccessResolver::load(db, &access_context, &rules).await?;
    let mut server_rule_map: HashMap<Uuid, Vec<mcp_access_policies::Model>> = HashMap::new();
    let mut tool_rule_map: HashMap<Uuid, Vec<mcp_access_policies::Model>> = HashMap::new();
    for rule in rules {
        match rule.target_type {
            mcp_access_policies::McpAccessTarget::Server => {
                if let Some(id) = rule.server_id {
                    server_rule_map.entry(id).or_default().push(rule);
                }
            }
            mcp_access_policies::McpAccessTarget::Tool => {
                if let Some(id) = rule.tool_id {
                    tool_rule_map.entry(id).or_default().push(rule);
                }
            }
        }
    }
    let mut server_access = HashMap::with_capacity(servers.len());
    for server in servers {
        if !server.enabled {
            continue;
        }
        let rules = server_rule_map
            .get(&server.id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        server_access.insert(server.id, resolver.server(server, rules));
    }
    let mut allowed = Vec::with_capacity(tools.len());
    for tool in tools {
        let Some(access) = server_access.get(&tool.server_id) else {
            continue;
        };
        let tool_rules = tool_rule_map
            .get(&tool.id)
            .map(|rules| rules.as_slice())
            .unwrap_or(&[]);
        let tool_access = resolver.tool(&tool, access, tool_rules);
        if tool_is_usable(tool_access.permission, tool.is_read_only) {
            allowed.push(tool);
        }
    }
    Ok(McpListingAccess {
        server_access,
        tools: allowed,
    })
}

pub async fn load_mcp_tools(
    state: &SharedState,
    user_id: Uuid,
    selected_server_ids: &[Uuid],
    selected_tools: &[String],
) -> Result<(HashMap<String, McpToolDescriptor>, Vec<McpServerSummary>), AppError> {
    let debug = std::env::var("MISTRAL_TOOL_DEBUG").as_deref() == Ok("1");
    if selected_server_ids.is_empty() {
        return Ok((HashMap::new(), Vec::new()));
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
    let mut server_map: HashMap<Uuid, (String, Option<String>, mcp_servers::Model)> =
        HashMap::new();
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

    let mut lookup = HashMap::new();
    let mut allowed_servers: HashSet<Uuid> = HashSet::new();

    for tool in tools {
        let Some((server_name, server_description, server_model)) = server_map.get(&tool.server_id)
        else {
            continue;
        };
        let openai_name = mcp_tool_name(&tool.server_id, &tool.name);
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

        if !tool_is_usable(tool_access.permission, tool.is_read_only) {
            if debug {
                println!(
                    "mcp tools debug: deny tool '{}' (openai='{}') via {:?} permission={:?} is_read_only={}",
                    tool.name,
                    openai_name,
                    tool_access.resolved_via,
                    tool_access.permission,
                    tool.is_read_only
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
            input_schema: normalize_openai_parameters(&tool.input_schema),
            is_read_only: tool.is_read_only,
            permission: tool_access.permission,
        };

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

    Ok((lookup, server_summaries))
}

#[cfg(test)]
mod tests {
    use super::tool_is_usable;
    use crate::models::mcp_access_policies::McpPermission;

    #[test]
    fn full_access_tools_are_usable_whether_or_not_they_are_read_only() {
        assert!(tool_is_usable(McpPermission::Full, false));
        assert!(tool_is_usable(McpPermission::Full, true));
    }

    #[test]
    fn read_only_access_allows_only_read_only_tools() {
        assert!(tool_is_usable(McpPermission::ReadOnly, true));
        assert!(!tool_is_usable(McpPermission::ReadOnly, false));
    }

    #[test]
    fn denied_tools_are_never_usable() {
        assert!(!tool_is_usable(McpPermission::Denied, true));
        assert!(!tool_is_usable(McpPermission::Denied, false));
    }

    // Runs on the disposable security-test database against a local remote-MCP fixture, so a
    // refused call is proven by the fixture never seeing tools/call.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
    async fn revocation_blocks_the_next_remote_mcp_call_before_it_reaches_the_server() {
        use super::call_with_current_access;
        use crate::{models::mcp_servers::McpTransportType, services::mcp_client::McpServerClient};
        use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
        use sea_orm::{ConnectionTrait, Database};
        use serde_json::{Value, json};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use uuid::Uuid;

        let url = std::env::var("GRENGIN_SECURITY_TEST_DATABASE_URL").expect("test database URL");
        assert!(
            url.contains("/grengin_security_test"),
            "refusing a non-test database"
        );
        let db = Database::connect(&url).await.expect("test database");
        let calls = Arc::new(AtomicUsize::new(0));
        let router = Router::new().route(
            "/mcp",
            post({
                let calls = calls.clone();
                move |Json(message): Json<Value>| {
                    let calls = calls.clone();
                    async move {
                        if message.get("id").is_none() {
                            return StatusCode::ACCEPTED.into_response();
                        }
                        let result = match message["method"].as_str() {
                            Some("initialize") => {
                                json!({"protocolVersion":message["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"revocation","version":"1"}})
                            }
                            Some("tools/call") => {
                                calls.fetch_add(1, Ordering::SeqCst);
                                json!({"content":[],"structuredContent":{"ok":true}})
                            }
                            _ => return StatusCode::BAD_REQUEST.into_response(),
                        };
                        Json(json!({"jsonrpc":"2.0","id":message["id"],"result":result}))
                            .into_response()
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let mock = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let (user, server, tool) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        db.execute_unprepared(&format!(
            r#"INSERT INTO users (id, status, email, "emailVerified", "mfaEnabled", "createdAt", "updatedAt", "lastLoginAt")
                 VALUES ('{user}', 'active', 'revocation-{user}@example.invalid', true, false, now(), now(), now());
               INSERT INTO mcp_servers (id, name, "transportType", url, "connectionConfig", enabled, "toolCount", "defaultAccess", "createdAt", "updatedAt")
                 VALUES ('{server}', 'Revocation fixture', 'http', '{endpoint}', '{{}}', true, 1, 'all_users', now(), now());
               INSERT INTO mcp_tools (id, "serverId", "serverName", name, "originalName", "inputSchema", parameters, enabled, "isReadOnly", "inheritAccessFromServer", "createdAt", "updatedAt")
                 VALUES ('{tool}', '{server}', 'Revocation fixture', 'revocable_tool', 'revocable_tool', '{{}}', '{{}}', true, false, true, now(), now());"#
        ))
        .await
        .expect("fixture rows");
        let client = McpServerClient::new(
            server,
            "Revocation fixture".into(),
            McpTransportType::Http,
            Some(endpoint),
            json!({}),
            None,
        );
        let call = || {
            call_with_current_access(
                &db,
                &client,
                "revocable_tool",
                json!({}),
                Some("user-token".into()),
                user,
            )
        };
        let rule = |target: &str, tool_id: &str, permission: &str| {
            format!(
                r#"INSERT INTO mcp_access_policies (id, "targetType", "serverId", "toolId", "accessType", permission, "userId", "inheritDepartments", "createdAt")
                     VALUES ('{}', '{target}', '{server}', {tool_id}, 'user', '{permission}', '{user}', true, now())"#,
                Uuid::new_v4()
            )
        };
        let clear_rules =
            format!("DELETE FROM mcp_access_policies WHERE \"serverId\" = '{server}'");
        let revocations = [
            (
                "user deactivated",
                format!("UPDATE users SET status = 'deactivated' WHERE id = '{user}'"),
                format!("UPDATE users SET status = 'active' WHERE id = '{user}'"),
            ),
            (
                "server disabled",
                format!("UPDATE mcp_servers SET enabled = false WHERE id = '{server}'"),
                format!("UPDATE mcp_servers SET enabled = true WHERE id = '{server}'"),
            ),
            (
                "server access narrowed to explicit grants",
                format!(
                    "UPDATE mcp_servers SET \"defaultAccess\" = 'explicit_only' WHERE id = '{server}'"
                ),
                format!(
                    "UPDATE mcp_servers SET \"defaultAccess\" = 'all_users' WHERE id = '{server}'"
                ),
            ),
            (
                "server access denied to the user",
                rule("server", "NULL", "denied"),
                clear_rules.clone(),
            ),
            (
                "server access reduced to read-only for a write tool",
                rule("server", "NULL", "read_only"),
                clear_rules.clone(),
            ),
            (
                "tool disabled",
                format!("UPDATE mcp_tools SET enabled = false WHERE id = '{tool}'"),
                format!("UPDATE mcp_tools SET enabled = true WHERE id = '{tool}'"),
            ),
            (
                "tool no longer inherits server access",
                format!(
                    "UPDATE mcp_tools SET \"inheritAccessFromServer\" = false WHERE id = '{tool}'"
                ),
                format!(
                    "UPDATE mcp_tools SET \"inheritAccessFromServer\" = true WHERE id = '{tool}'"
                ),
            ),
            (
                "server destination changed",
                format!(
                    "UPDATE mcp_servers SET url = 'https://elsewhere.example/mcp' WHERE id = '{server}'"
                ),
                format!(
                    "UPDATE mcp_servers SET url = '{}' WHERE id = '{server}'",
                    client.url.as_deref().unwrap()
                ),
            ),
        ];

        let mut outcomes = vec![(
            "granted",
            call().await.is_ok(),
            calls.load(Ordering::SeqCst),
        )];
        for (name, revoke, restore) in &revocations {
            db.execute_unprepared(revoke).await.expect(name);
            let refused = call().await.is_err();
            let reached = calls.load(Ordering::SeqCst);
            db.execute_unprepared(restore).await.expect(name);
            outcomes.push((*name, refused, reached));
            outcomes.push((
                "restored",
                call().await.is_ok(),
                calls.load(Ordering::SeqCst),
            ));
        }
        db.execute_unprepared(&format!(
            r#"UPDATE mcp_servers SET "defaultAccess" = 'explicit_only' WHERE id = '{server}';
               UPDATE mcp_tools SET "inheritAccessFromServer" = false WHERE id = '{tool}';
               {}"#,
            rule("tool", &format!("'{tool}'"), "full")
        ))
        .await
        .expect("explicit tool grant");
        outcomes.push((
            "explicit tool grant without server access",
            call().await.is_ok(),
            calls.load(Ordering::SeqCst),
        ));

        mock.abort();
        db.execute_unprepared(&format!(
            "DELETE FROM mcp_access_policies WHERE \"serverId\" = '{server}'; DELETE FROM mcp_tools WHERE id = '{tool}'; \
             DELETE FROM mcp_servers WHERE id = '{server}'; DELETE FROM users WHERE id = '{user}'"
        ))
        .await
        .expect("cleanup");
        let mut expected_calls = 0;
        for (name, passed, reached) in outcomes {
            if name == "granted" || name == "restored" || name.starts_with("explicit") {
                expected_calls += 1;
            }
            assert!(passed, "{name}");
            assert_eq!(reached, expected_calls, "{name}");
        }
        assert_eq!(expected_calls, revocations.len() + 2);
    }
}
