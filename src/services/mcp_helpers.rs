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

pub fn to_server_dto(model: mcp_servers::Model) -> McpServer {
    McpServer {
        id: model.id,
        name: model.name,
        description: model.description,
        transport_type: model.transport_type,
        connection_config: model.connection_config,
        client_id: model.client_id,
        client_secret_configured: model.client_secret.is_some(),
        url: model.url,
        enabled: model.enabled,
        status: model.status,
        status_message: model.status_message,
        tool_count: model.tool_count,
        default_access: model.default_access,
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
