// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::{Duration, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect, Set,
};
use serde::Serialize;
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    auth::encryption::{decrypt_key, encrypt_key},
    dto::mcp::{McpAccessRule, McpServer, McpTool, McpToolExecution},
    error::AppError,
    models::{
        departments, mcp_access_policies, mcp_connections, mcp_executions, mcp_oauth_states,
        mcp_servers, mcp_servers::McpTransportType, mcp_tools, roles, users,
    },
    services::mcp_tools::mcp_server_short_id,
    services::{
        mcp_client::{
            McpOAuthConfig, McpOAuthTokens, build_authorization_url, oauth_config_from_connection,
            refresh_token,
        },
        mcp_tools::{McpServerSummary, McpToolDescriptor},
    },
    state::SharedState,
};

pub fn encrypt_db_url_in_config(
    app_key: &[u8; 32],
    connection_config: &mut serde_json::Value,
) -> Result<(), AppError> {
    let Some(obj) = connection_config.as_object_mut() else {
        return Ok(());
    };
    let Some(db_url_value) = obj.get("db_url").cloned() else {
        return Ok(());
    };
    let Some(db_url) = db_url_value.as_str() else {
        return Ok(());
    };
    if decrypt_key(app_key, db_url).is_ok() {
        return Ok(());
    }
    let encrypted = encrypt_key(app_key, db_url.as_bytes())
        .map_err(|_| AppError::ServiceTemporarilyUnavailable)?;
    obj.insert("db_url".to_string(), serde_json::Value::String(encrypted));
    Ok(())
}

pub fn to_server_dto(state: &SharedState, model: mcp_servers::Model, connected: bool) -> McpServer {
    McpServer {
        id: model.id,
        name: model.name,
        description: model.description,
        icon: model.icon,
        transport_type: model.transport_type,
        connection_config: model.connection_config,
        client_id: model.client_id,
        client_secret_configured: model.client_secret.is_some(),
        client_secret_preview: state.get_decrypted_api_key_preview(&model.client_secret),
        url: model.url,
        enabled: model.enabled,
        status: model.status,
        status_message: model.status_message,
        tool_count: model.tool_count,
        default_access: model.default_access,
        connected,
        last_connected_at: model.last_connected_at,
        last_synced_at: model.last_synced_at,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

pub fn to_tool_dto(model: &mcp_tools::Model) -> McpTool {
    McpTool {
        id: model.id,
        server_id: model.server_id,
        server_name: model.server_name.clone(),
        name: model.name.clone(),
        original_name: model.original_name.clone(),
        description: model.description.clone(),
        input_schema: model.input_schema.clone(),
        parameters: model.parameters.clone(),
        enabled: model.enabled,
        is_read_only: model.is_read_only,
        inherit_access_from_server: model.inherit_access_from_server,
        last_synced_at: model.last_synced_at,
    }
}

pub fn to_execution_dto(model: mcp_executions::Model) -> McpToolExecution {
    McpToolExecution {
        id: model.id,
        server_id: model.server_id,
        server_name: model.server_name,
        tool_name: model.tool_name,
        conversation_id: model.conversation_id,
        user_id: model.user_id,
        user_email: model.user_email,
        arguments: model.arguments,
        result: model.result,
        is_error: model.is_error,
        duration_ms: model.duration_ms,
        executed_at: model.executed_at,
    }
}

pub fn to_access_rule_dto(model: mcp_access_policies::Model) -> McpAccessRule {
    let priority = match model.access_type {
        mcp_access_policies::McpAccessType::User => 300,
        mcp_access_policies::McpAccessType::Department => 200,
        mcp_access_policies::McpAccessType::Role => 100,
    };
    McpAccessRule {
        id: Some(model.id),
        access_type: model.access_type,
        role_id: model.role_id,
        role_name: model.role_name,
        department_id: model.department_id,
        department_name: None,
        user_id: model.user_id,
        user_email: None,
        permission: model.permission,
        inherit_departments: model.inherit_departments,
        priority,
    }
}

pub async fn build_access_rule_dtos(
    db: &DatabaseConnection,
    rules: Vec<mcp_access_policies::Model>,
) -> Result<Vec<McpAccessRule>, AppError> {
    let user_ids: Vec<Uuid> = rules.iter().filter_map(|rule| rule.user_id).collect();
    let department_ids: Vec<Uuid> = rules.iter().filter_map(|rule| rule.department_id).collect();
    let role_ids: Vec<Uuid> = rules.iter().filter_map(|rule| rule.role_id).collect();

    let mut user_email_map: HashMap<Uuid, String> = HashMap::new();
    if !user_ids.is_empty() {
        let rows = users::Entity::find()
            .select_only()
            .column(users::Column::Id)
            .column(users::Column::Email)
            .filter(users::Column::Id.is_in(user_ids))
            .into_tuple::<(Uuid, String)>()
            .all(db)
            .await
            .map_err(|e| {
                eprintln!("user lookup error: {e}");
                AppError::DbTimeout
            })?;
        for (id, email) in rows {
            user_email_map.insert(id, email);
        }
    }

    let mut department_name_map: HashMap<Uuid, String> = HashMap::new();
    if !department_ids.is_empty() {
        let rows = departments::Entity::find()
            .select_only()
            .column(departments::Column::Id)
            .column(departments::Column::Name)
            .filter(departments::Column::Id.is_in(department_ids))
            .into_tuple::<(Uuid, String)>()
            .all(db)
            .await
            .map_err(|e| {
                eprintln!("department lookup error: {e}");
                AppError::DbTimeout
            })?;
        for (id, name) in rows {
            department_name_map.insert(id, name);
        }
    }

    let mut role_name_map: HashMap<Uuid, String> = HashMap::new();
    if !role_ids.is_empty() {
        let rows = roles::Entity::find()
            .select_only()
            .column(roles::Column::Id)
            .column(roles::Column::Name)
            .filter(roles::Column::Id.is_in(role_ids))
            .into_tuple::<(Uuid, String)>()
            .all(db)
            .await
            .map_err(|e| {
                eprintln!("role lookup error: {e}");
                AppError::DbTimeout
            })?;
        for (id, name) in rows {
            role_name_map.insert(id, name);
        }
    }

    let dtos = rules
        .into_iter()
        .map(|rule| {
            let user_id = rule.user_id;
            let department_id = rule.department_id;
            let role_id = rule.role_id;
            let mut dto = to_access_rule_dto(rule);
            dto.user_email = user_id.and_then(|id| user_email_map.get(&id).cloned());
            dto.department_name =
                department_id.and_then(|id| department_name_map.get(&id).cloned());
            dto.role_name = role_id
                .and_then(|id| role_name_map.get(&id).cloned())
                .or(dto.role_name);
            dto
        })
        .collect();

    Ok(dtos)
}

pub async fn upsert_connection(
    state: &SharedState,
    user_id: Uuid,
    server_id: Uuid,
    connected: bool,
) -> Result<(), AppError> {
    if let Some(existing) = mcp_connections::Entity::find()
        .filter(mcp_connections::Column::UserId.eq(user_id))
        .filter(mcp_connections::Column::ServerId.eq(server_id))
        .one(&state.database)
        .await
        .map_err(|_| AppError::DbUnavailable)?
    {
        let mut active: mcp_connections::ActiveModel = existing.into();
        active.connected = Set(connected);
        if connected {
            active.connected_at = Set(Some(Utc::now()));
        } else {
            active.connected_at = Set(None);
            active.expires_at = Set(None);
            active.scopes = Set(None);
            active.access_token = Set(None);
            active.refresh_token = Set(None);
            active.token_type = Set(None);
        }
        active.updated_at = Set(Utc::now());
        active.update(&state.database).await.map_err(|e| {
            eprintln!("Db update error {}", e);
            AppError::DbTimeout
        })?;
    } else {
        let model = mcp_connections::ActiveModel {
            id: Set(Uuid::new_v4()),
            user_id: Set(user_id),
            server_id: Set(server_id),
            server_name: Set("".into()),
            connected: Set(connected),
            connected_at: Set(if connected { Some(Utc::now()) } else { None }),
            expires_at: Set(None),
            scopes: Set(None),
            access_token: Set(None),
            refresh_token: Set(None),
            token_type: Set(None),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
        };
        model.insert(&state.database).await.map_err(|e| {
            eprintln!("Db insert error {}", e);
            AppError::DbTimeout
        })?;
    }
    Ok(())
}

pub async fn resolve_mcp_oauth_token(
    state: &SharedState,
    user_id: Uuid,
    server_id: Uuid,
) -> Result<Option<String>, AppError> {
    let connection = mcp_connections::Entity::find()
        .filter(mcp_connections::Column::UserId.eq(user_id))
        .filter(mcp_connections::Column::ServerId.eq(server_id))
        .one(&state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp connection lookup error: {e}");
            AppError::DbTimeout
        })?;

    let Some(connection) = connection else {
        return Ok(None);
    };
    if !connection.connected {
        return Ok(None);
    }

    let Some(access_token) = connection.access_token.clone() else {
        return Ok(None);
    };

    let needs_refresh = match connection.expires_at {
        Some(exp) => exp <= Utc::now(),
        None => false,
    };

    if !needs_refresh {
        return Ok(Some(access_token));
    }

    let Some(refresh_token_value) = connection.refresh_token.clone() else {
        return Ok(None);
    };

    let server = mcp_servers::Entity::find_by_id(server_id)
        .one(&state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp server lookup error: {e}");
            AppError::DbTimeout
        })?
        .ok_or(AppError::ResourceNotFound)?;

    let oauth_config = build_oauth_config(&state, &server)?;
    let refreshed = refresh_token(&oauth_config, &refresh_token_value, &state.req_client)
        .await
        .map_err(|e| {
            eprintln!("mcp oauth refresh error: {e}");
            AppError::ServiceTemporarilyUnavailable
        })?;

    store_oauth_tokens(state, user_id, server_id, &server.name, refreshed).await?;
    let updated = mcp_connections::Entity::find()
        .filter(mcp_connections::Column::UserId.eq(user_id))
        .filter(mcp_connections::Column::ServerId.eq(server_id))
        .one(&state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp connection reload error: {e}");
            AppError::DbTimeout
        })?
        .and_then(|row| row.access_token);

    Ok(updated)
}

pub async fn store_oauth_tokens(
    state: &SharedState,
    user_id: Uuid,
    server_id: Uuid,
    server_name: &str,
    tokens: McpOAuthTokens,
) -> Result<(), AppError> {
    let now = Utc::now();
    let scopes_json = if tokens.scopes.is_empty() {
        None
    } else {
        Some(serde_json::to_value(tokens.scopes.clone()).map_err(|e| {
            eprintln!("mcp oauth scopes serialize error: {e}");
            AppError::DbTimeout
        })?)
    };

    if let Some(existing) = mcp_connections::Entity::find()
        .filter(mcp_connections::Column::UserId.eq(user_id))
        .filter(mcp_connections::Column::ServerId.eq(server_id))
        .one(&state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp connection lookup error: {e}");
            AppError::DbTimeout
        })?
    {
        let mut active: mcp_connections::ActiveModel = existing.into();
        active.connected = Set(true);
        active.connected_at = Set(Some(now));
        active.expires_at = Set(tokens.expires_at);
        active.scopes = Set(scopes_json);
        active.access_token = Set(Some(tokens.access_token));
        active.refresh_token = Set(tokens.refresh_token);
        active.token_type = Set(tokens.token_type);
        active.updated_at = Set(now);
        active.update(&state.database).await.map_err(|e| {
            eprintln!("mcp connection update error: {e}");
            AppError::DbTimeout
        })?;
        return Ok(());
    }

    let model = mcp_connections::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(user_id),
        server_id: Set(server_id),
        server_name: Set(server_name.to_string()),
        connected: Set(true),
        connected_at: Set(Some(now)),
        expires_at: Set(tokens.expires_at),
        scopes: Set(scopes_json),
        access_token: Set(Some(tokens.access_token)),
        refresh_token: Set(tokens.refresh_token),
        token_type: Set(tokens.token_type),
        created_at: Set(now),
        updated_at: Set(now),
    };
    model.insert(&state.database).await.map_err(|e| {
        eprintln!("mcp connection insert error: {e}");
        AppError::DbTimeout
    })?;
    Ok(())
}

pub fn build_oauth_config(
    state: &SharedState,
    server: &mcp_servers::Model,
) -> Result<McpOAuthConfig, AppError> {
    let client_id = server
        .client_id
        .clone()
        .ok_or(AppError::ServiceTemporarilyUnavailable)?;
    let client_secret = match server.client_secret.as_deref() {
        Some(secret) => Some(
            decrypt_key(&state.settings.auth.app_key, secret)
                .map_err(|_| AppError::ServiceTemporarilyUnavailable)?,
        ),
        None => None,
    };
    oauth_config_from_connection(&server.connection_config, &client_id, client_secret).map_err(
        |e| {
            eprintln!("mcp oauth config error: {e}");
            AppError::ServiceTemporarilyUnavailable
        },
    )
}

#[derive(Clone)]
pub struct McpOauthPrompt {
    pub authorization_url: String,
    pub server_name: String,
}

#[derive(Serialize)]
pub struct McpOauthRequiredEvent {
    pub text: String,
    pub server_id: Uuid,
    pub server_name: String,
    pub authorization_url: String,
    pub tool_name: String,
    pub tool_call_id: Option<String>,
}

#[derive(Serialize)]
pub struct McpOauthErrorPayload {
    pub error: String,
    pub is_error: bool,
    pub authorization_url: String,
    pub server_id: Uuid,
    pub server_name: String,
}

pub fn build_mcp_server_context(
    servers: &[McpServerSummary],
    tool_lookup: &HashMap<String, McpToolDescriptor>,
) -> Option<String> {
    if servers.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    lines.push("MCP servers selected for this request:".to_string());
    for server in servers {
        let short_id = mcp_server_short_id(&server.server_id);
        let mut line = format!(
            "- {} (id: {}, short: {})",
            server.name, server.server_id, short_id
        );
        if let Some(description) = server
            .description
            .as_ref()
            .map(|text| text.trim())
            .filter(|text| !text.is_empty())
        {
            line.push_str(&format!(" — {}", description));
        }
        lines.push(line);
    }
    lines.push(
        "Tools starting with mcp__<short>__ map to the corresponding server above.".to_string(),
    );
    let has_sqlx_query_tool = tool_lookup.values().any(|tool| {
        tool.original_name == "sql_query" && tool.server_name.to_ascii_lowercase().contains("sqlx")
    });
    if has_sqlx_query_tool {
        lines.push("SQL dialect hint: The sqlx MCP server uses PostgreSQL. Do not use sqlite_master, PRAGMA, or SHOW TABLES.".to_string());
        lines.push(
            "Use PostgreSQL catalog queries such as information_schema.tables when listing tables."
                .to_string(),
        );
    }
    Some(lines.join("\n"))
}

pub fn resolve_mcp_tool_descriptor<'a>(
    lookup: &'a HashMap<String, McpToolDescriptor>,
    tool_name: &str,
) -> Option<&'a McpToolDescriptor> {
    if let Some(found) = lookup.get(tool_name) {
        return Some(found);
    }
    let is_mcp_name = tool_name.starts_with("mcp__");
    let full_prefix = if is_mcp_name {
        Some(format!("{tool_name}__"))
    } else {
        None
    };
    // Only valid when there are 3+ double-underscore separators; otherwise rsplit_once
    // would strip the tool name itself and yield a server-only prefix that matches
    // every tool on the same server.
    let truncated_prefix = if is_mcp_name && tool_name.matches("__").count() >= 3 {
        tool_name
            .rsplit_once("__")
            .map(|(prefix, _)| format!("{prefix}__"))
    } else {
        None
    };

    let matches: Vec<&McpToolDescriptor> = lookup
        .iter()
        .filter_map(|(name, descriptor)| {
            if !is_mcp_name && descriptor.original_name == tool_name {
                return Some(descriptor);
            }
            if !is_mcp_name {
                return None;
            }
            if full_prefix
                .as_ref()
                .map(|prefix| name.starts_with(prefix))
                .unwrap_or(false)
            {
                return Some(descriptor);
            }
            if truncated_prefix
                .as_ref()
                .map(|prefix| name.starts_with(prefix))
                .unwrap_or(false)
            {
                return Some(descriptor);
            }
            None
        })
        .collect();

    if matches.len() == 1 {
        matches.first().copied()
    } else {
        None
    }
}

pub async fn build_mcp_oauth_prompt(
    state: &SharedState,
    server_id: Uuid,
    user_id: Uuid,
) -> Result<McpOauthPrompt, AppError> {
    let server = mcp_servers::Entity::find_by_id(server_id)
        .one(&state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp server lookup error: {e}");
            AppError::DbTimeout
        })?
        .ok_or(AppError::McpServerNotFound)?;

    if !server.enabled {
        return Err(AppError::McpServerNotFound);
    }

    if !matches!(
        server.transport_type,
        McpTransportType::Http | McpTransportType::Sse
    ) {
        return Err(AppError::ServiceTemporarilyUnavailable);
    }

    let oauth_config = build_oauth_config(state, &server)?;
    let authorization = build_authorization_url(&oauth_config).map_err(|e| {
        eprintln!("mcp oauth authorize url error: {e}");
        AppError::ServiceTemporarilyUnavailable
    })?;

    let now = Utc::now();
    let expires_at = now + Duration::minutes(10);
    let model = mcp_oauth_states::ActiveModel {
        id: Set(Uuid::new_v4()),
        server_id: Set(server_id),
        user_id: Set(user_id),
        state: Set(authorization.state.clone()),
        pkce_verifier: Set(authorization.pkce_verifier.clone()),
        redirect_uri: Set(None),
        expires_at: Set(Some(expires_at)),
        created_at: Set(now),
    };
    model.insert(&state.database).await.map_err(|e| {
        eprintln!("mcp oauth state insert error: {e}");
        AppError::DbTimeout
    })?;

    Ok(McpOauthPrompt {
        authorization_url: authorization.authorization_url,
        server_name: server.name,
    })
}
