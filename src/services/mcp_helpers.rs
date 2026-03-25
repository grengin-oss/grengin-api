use chrono::Utc;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

use crate::{
    auth::encryption::{decrypt_key, encrypt_key},
    dto::mcp::{McpAccessRule, McpServer, McpTool, McpToolExecution},
    error::AppError,
    models::{
        mcp_access_policies,
        mcp_connections,
        mcp_executions,
        mcp_servers,
        mcp_tools,
    },
    services::mcp_client::{refresh_token, McpOAuthConfig, McpOAuthTokens, oauth_config_from_connection},
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

pub fn to_server_dto(
    state: &SharedState,
    model: mcp_servers::Model,
    connected: bool,
) -> McpServer {
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
    McpAccessRule {
        id: Some(model.id),
        access_type: model.access_type,
        role_name: model.role_name,
        department_id: model.department_id,
        user_id: model.user_id,
        permission: model.permission,
    }
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
        active.updated_at = Set(Utc::now());
        active
            .update(&state.database)
            .await
            .map_err(|e| {
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
        model
            .insert(&state.database)
            .await
            .map_err(|e| {
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
        return Ok(Some(access_token));
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
        active
            .update(&state.database)
            .await
            .map_err(|e| {
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
    model
        .insert(&state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp connection insert error: {e}");
            AppError::DbTimeout
        })?;
    Ok(())
}

pub fn build_oauth_config(
    state: &SharedState,
    server: &mcp_servers::Model,
) -> Result<McpOAuthConfig, AppError> {
    let client_id = server.client_id.clone().ok_or(AppError::ServiceTemporarilyUnavailable)?;
    let client_secret = match server.client_secret.as_deref() {
        Some(secret) => Some(
            decrypt_key(&state.settings.auth.app_key, secret)
                .map_err(|_| AppError::ServiceTemporarilyUnavailable)?,
        ),
        None => None,
    };
    oauth_config_from_connection(&server.connection_config, &client_id, client_secret)
        .map_err(|e| {
            eprintln!("mcp oauth config error: {e}");
            AppError::ServiceTemporarilyUnavailable
        })
}
