// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{collections::HashMap, sync::LazyLock, time::Duration};

pub use crate::services::mcp_plugin_config::{compile_plugin, plugin_tools};
use chrono::Utc;
use connector_server_plugin::{CompiledConnector, ConnectorAuth};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, Iterable, QueryFilter, QuerySelect, Set,
    TransactionTrait, TryIntoModel,
};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    auth::error::AuthError,
    dto::mcp::{McpCredentialRequest, McpSyncResult, McpUserConnection},
    error::AppError,
    models::{
        connectors, mcp_access_policies, mcp_connections, mcp_oauth_states, mcp_server_tools,
        mcp_servers, mcp_tools, project_mcp_servers, users,
    },
    services::{
        auth_audit::record_auth_event,
        mcp_client::{McpClientError, McpServerClient},
        mcp_oauth::OAuthRefreshLocks,
        mcp_tools::sanitize_tool_name,
    },
    state::SharedState,
};

static CONFIG_LOCKS: LazyLock<OAuthRefreshLocks> = LazyLock::new(OAuthRefreshLocks::default);
static CREDENTIAL_LIMITER: LazyLock<crate::services::connector_limits::ConnectorStartLimiter> =
    LazyLock::new(crate::services::connector_limits::ConnectorStartLimiter::default);

pub async fn save_credential(
    state: &SharedState,
    user: Uuid,
    id: Uuid,
    request: McpCredentialRequest,
) -> Result<McpUserConnection, crate::services::connector_broker::ConnectorError> {
    use crate::services::connector_broker::{BrokerIssue, ConnectorError as ApiError};
    CREDENTIAL_LIMITER
        .admit(user, std::time::Instant::now())
        .map_err(|retry| ApiError::Broker(BrokerIssue::RateLimited(retry)))?;
    if request.credential.is_empty()
        || request.credential.len() > 16384
        || request
            .credential
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
        || request
            .expires_at
            .is_some_and(|expiry| expiry <= Utc::now())
    {
        return Err(ApiError::Broker(BrokerIssue::InvalidRequest));
    }
    let _config = tokio::time::timeout(Duration::from_secs(5), CONFIG_LOCKS.lock(Uuid::nil(), id))
        .await
        .map_err(|_| AppError::ServiceTemporarilyUnavailable)?;
    let _connection = state.mcp_refresh_locks.lock(user, id).await;
    let server = crate::services::mcp_helpers::require_server_access(state, user, id).await?;
    let plugin = compile_plugin(
        &server.connection_config,
        server.transport_type,
        server.url.as_deref(),
    )
    .map_err(|_| AppError::ResourceNotFound)?
    .ok_or(AppError::ResourceNotFound)?;
    if plugin.manifest().auth != ConnectorAuth::ApiKey {
        return Err(AppError::ResourceNotFound.into());
    }
    let transaction = state
        .database
        .begin()
        .await
        .map_err(|_| AppError::DbTimeout)?;
    crate::services::mcp_helpers::store_oauth_tokens_with(
        &transaction,
        &state.settings.auth.app_key,
        user,
        id,
        &server.name,
        crate::services::mcp_client::McpOAuthTokens {
            access_token: request.credential,
            refresh_token: None,
            expires_at: request.expires_at,
            token_type: None,
            scopes: vec![],
        },
    )
    .await?;
    record_auth_event(
        &transaction,
        "mcp.credential_saved",
        Some(user),
        serde_json::json!({"server_id":id,"plugin_id":plugin.manifest().id}),
    )
    .await
    .map_err(|_| AppError::DbTimeout)?;
    transaction
        .commit()
        .await
        .map_err(|_| AppError::DbTimeout)?;
    Ok(McpUserConnection {
        server_id: id,
        server_name: server.name,
        description: server.description,
        connected: true,
        connected_at: Some(Utc::now()),
        expires_at: request.expires_at,
        scopes: None,
    })
}

pub async fn call_with_current_access(
    state: &SharedState,
    client: &McpServerClient,
    tool_name: &str,
    arguments: Value,
    token: Option<String>,
    user: Uuid,
) -> Result<rmcp::model::CallToolResult, McpClientError> {
    if client.connection_config.get("connector_plugin").is_some() {
        let database_error =
            |_| McpClientError::Config("connector authorization unavailable".into());
        let denied =
            || McpClientError::Config("connector access denied or configuration changed".into());
        let owner = users::Entity::find_by_id(user)
            .one(&state.database)
            .await
            .map_err(database_error)?
            .ok_or_else(denied)?;
        if owner.status != users::UserStatus::Active {
            return Err(denied());
        }
        let server = mcp_servers::Entity::find_by_id(client.server_id)
            .one(&state.database)
            .await
            .map_err(database_error)?
            .ok_or_else(denied)?;
        if !server.enabled
            || server.url != client.url
            || server.connection_config != client.connection_config
        {
            return Err(denied());
        }
        let tool = mcp_tools::Entity::find()
            .filter(mcp_tools::Column::ServerId.eq(client.server_id))
            .filter(mcp_tools::Column::OriginalName.eq(tool_name))
            .filter(mcp_tools::Column::Enabled.eq(true))
            .one(&state.database)
            .await
            .map_err(database_error)?
            .ok_or_else(denied)?;
        if crate::services::mcp_tools::filter_tools_for_user(&state.database, user, vec![tool])
            .await
            .map_err(|_| denied())?
            .is_empty()
        {
            return Err(denied());
        }
    }
    client
        .call_tool(tool_name, arguments, token, Some(user))
        .await
}

pub async fn sync_installed_plugin(
    state: &SharedState,
    id: Uuid,
    actor: Uuid,
) -> Result<Option<McpSyncResult>, AuthError> {
    let _guard = tokio::time::timeout(Duration::from_secs(5), CONFIG_LOCKS.lock(Uuid::nil(), id))
        .await
        .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    let server = mcp_servers::Entity::find_by_id(id)
        .one(&state.database)
        .await
        .map_err(database_error)?
        .ok_or(AuthError::ResourceNotFound)?;
    let Some(plugin) = compile_plugin(
        &server.connection_config,
        server.transport_type,
        server.url.as_deref(),
    )
    .map_err(|_| AuthError::InvalidRequest {
        field: "connection_config.connector_plugin",
    })?
    else {
        return Ok(None);
    };
    let transaction = state.database.begin().await.map_err(database_error)?;
    let result = sync_tools(&transaction, &server, &plugin).await?;
    record_auth_event(
        &transaction,
        "mcp.plugin_tools_synced",
        Some(actor),
        serde_json::json!({"server_id":id}),
    )
    .await
    .map_err(|_| AuthError::DbTimeout)?;
    transaction.commit().await.map_err(database_error)?;
    Ok(Some(result))
}

fn database_error(error: sea_orm::DbErr) -> AuthError {
    eprintln!("MCP plugin persistence failed: {error}");
    AuthError::DbTimeout
}

pub async fn save_server(
    state: &SharedState,
    mut model: mcp_servers::ActiveModel,
    create: bool,
    actor: Uuid,
) -> Result<mcp_servers::Model, AuthError> {
    let id = model.clone().try_into_model().map_err(database_error)?.id;
    let _guard = tokio::time::timeout(Duration::from_secs(5), CONFIG_LOCKS.lock(Uuid::nil(), id))
        .await
        .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    if !create {
        let current = mcp_servers::Entity::find_by_id(id)
            .one(&state.database)
            .await
            .map_err(database_error)?
            .ok_or(AuthError::ResourceNotFound)?;
        let mut fresh: mcp_servers::ActiveModel = current.into();
        // Handler snapshots can precede another admin's commit; apply only explicitly changed fields.
        for column in mcp_servers::Column::iter() {
            let value = model.get(column);
            if value.is_set() {
                fresh.set(column, value.unwrap());
            }
        }
        model = fresh;
    }
    let preview = model.clone().try_into_model().map_err(database_error)?;
    let plugin = compile_plugin(
        &preview.connection_config,
        preview.transport_type,
        preview.url.as_deref(),
    )
    .map_err(|error| {
        eprintln!("MCP plugin validation failed: {error}");
        AuthError::InvalidRequest {
            field: "connection_config.connector_plugin",
        }
    })?;
    if plugin
        .as_ref()
        .is_some_and(|plugin| plugin.manifest().auth == ConnectorAuth::Oauth2)
    {
        let client_id = preview
            .client_id
            .as_ref()
            .filter(|id| !id.trim().is_empty())
            .ok_or(AuthError::InvalidRequest { field: "client_id" })?;
        crate::services::mcp_client::oauth_config_from_connection(
            &preview.connection_config,
            client_id,
            preview.client_secret.clone(),
        )
        .map_err(|_| AuthError::InvalidRequest {
            field: "connection_config.oauth",
        })?;
    }
    if let Some(plugin) = &plugin {
        model.tool_count = Set(plugin.manifest().tools.len() as i32);
        model.last_synced_at = Set(Some(Utc::now()));
        model.status = Set(Some(
            if plugin.manifest().auth == ConnectorAuth::None {
                "connected"
            } else {
                "configured"
            }
            .into(),
        ));
    }
    let transaction = state.database.begin().await.map_err(database_error)?;
    let previous = if create {
        None
    } else {
        mcp_servers::Entity::find_by_id(preview.id)
            .lock_exclusive()
            .one(&transaction)
            .await
            .map_err(database_error)?
    };
    if plugin.is_some()
        && connectors::Entity::find()
            .filter(connectors::Column::ServerId.eq(preview.id))
            .one(&transaction)
            .await
            .map_err(database_error)?
            .is_some()
    {
        return Err(AuthError::InvalidRequest {
            field: "connection_config.connector_plugin",
        });
    }
    let reset_credentials = previous.as_ref().is_some_and(|old| {
        (plugin.is_some() || old.connection_config.get("connector_plugin").is_some())
            && (old.url != preview.url
                || old.transport_type != preview.transport_type
                || old.client_id != preview.client_id
                || old.client_secret != preview.client_secret
                || old.connection_config.get("oauth") != preview.connection_config.get("oauth")
                || old.connection_config.pointer("/connector_plugin/auth_mode")
                    != preview
                        .connection_config
                        .pointer("/connector_plugin/auth_mode")
                || old
                    .connection_config
                    .pointer("/connector_plugin/manifest/auth")
                    != preview
                        .connection_config
                        .pointer("/connector_plugin/manifest/auth")
                || old
                    .connection_config
                    .pointer("/connector_plugin/manifest/credential_delivery")
                    != preview
                        .connection_config
                        .pointer("/connector_plugin/manifest/credential_delivery")
                || old.connection_config.get("connector_plugin").is_some() != plugin.is_some())
    });
    let saved = if create {
        model.insert(&transaction).await
    } else {
        model.update(&transaction).await
    }
    .map_err(|error| {
        if matches!(
            error.sql_err(),
            Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
        ) {
            AuthError::McpServerNameConflict {
                name: Some(preview.name),
            }
        } else {
            database_error(error)
        }
    })?;
    if reset_credentials {
        mcp_connections::Entity::update_many()
            .col_expr(
                mcp_connections::Column::Connected,
                sea_orm::sea_query::Expr::value(false),
            )
            .col_expr(
                mcp_connections::Column::AccessToken,
                sea_orm::sea_query::Expr::value(None::<String>),
            )
            .col_expr(
                mcp_connections::Column::RefreshToken,
                sea_orm::sea_query::Expr::value(None::<String>),
            )
            .col_expr(
                mcp_connections::Column::ExpiresAt,
                sea_orm::sea_query::Expr::value(None::<chrono::DateTime<Utc>>),
            )
            .col_expr(
                mcp_connections::Column::ConnectedAt,
                sea_orm::sea_query::Expr::value(None::<chrono::DateTime<Utc>>),
            )
            .col_expr(
                mcp_connections::Column::Scopes,
                sea_orm::sea_query::Expr::value(None::<Value>),
            )
            .col_expr(
                mcp_connections::Column::TokenType,
                sea_orm::sea_query::Expr::value(None::<String>),
            )
            .col_expr(
                mcp_connections::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(Utc::now()),
            )
            .filter(mcp_connections::Column::ServerId.eq(saved.id))
            .exec(&transaction)
            .await
            .map_err(database_error)?;
        mcp_oauth_states::Entity::delete_many()
            .filter(mcp_oauth_states::Column::ServerId.eq(saved.id))
            .exec(&transaction)
            .await
            .map_err(database_error)?;
    }
    if let Some(plugin) = &plugin {
        sync_tools(&transaction, &saved, plugin).await?;
        record_auth_event(&transaction, "mcp.plugin_saved", Some(actor), serde_json::json!({
            "server_id": saved.id, "plugin_id": plugin.manifest().id, "plugin_version": plugin.manifest().version,
        })).await.map_err(|_| AuthError::DbTimeout)?;
    } else if previous
        .as_ref()
        .is_some_and(|old| old.connection_config.get("connector_plugin").is_some())
    {
        remove_tools(&transaction, saved.id).await?;
        record_auth_event(
            &transaction,
            "mcp.plugin_removed",
            Some(actor),
            serde_json::json!({"server_id":saved.id}),
        )
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    }
    transaction.commit().await.map_err(database_error)?;
    state.upsert_mcp_client(&saved).await;
    Ok(saved)
}

async fn sync_tools<C: sea_orm::ConnectionTrait>(
    database: &C,
    server: &mcp_servers::Model,
    plugin: &CompiledConnector,
) -> Result<McpSyncResult, AuthError> {
    let existing = mcp_tools::Entity::find()
        .filter(mcp_tools::Column::ServerId.eq(server.id))
        .all(database)
        .await
        .map_err(database_error)?;
    let mut existing: HashMap<_, _> = existing
        .into_iter()
        .map(|tool| (tool.original_name.clone(), tool))
        .collect();
    let mut added = 0;
    let mut updated = 0;
    for tool in &plugin.manifest().tools {
        let old = existing.remove(&tool.name);
        let is_new = old.is_none();
        let mut model: mcp_tools::ActiveModel = if let Some(old) = old {
            updated += 1;
            old.into()
        } else {
            added += 1;
            mcp_tools::ActiveModel {
                id: Set(Uuid::new_v4()),
                server_id: Set(server.id),
                enabled: Set(true),
                original_name: Set(tool.name.clone()),
                inherit_access_from_server: Set(true),
                created_at: Set(Utc::now()),
                ..Default::default()
            }
        };
        model.server_name = Set(server.name.clone());
        model.name = Set(sanitize_tool_name(&tool.name));
        model.description = Set(Some(tool.description.clone()));
        model.input_schema = Set(tool.input_schema.clone());
        model.parameters = Set(tool.input_schema.clone());
        model.is_read_only = Set(tool.read_only);
        model.last_synced_at = Set(Some(Utc::now()));
        model.updated_at = Set(Utc::now());
        if is_new {
            model.insert(database).await.map_err(database_error)?;
        } else {
            model.update(database).await.map_err(database_error)?;
        }
    }
    let removed = existing.len() as i32;
    if !existing.is_empty() {
        remove_tool_ids(
            database,
            existing.into_values().map(|tool| tool.id).collect(),
        )
        .await?;
    }
    Ok(McpSyncResult {
        success: true,
        tools_added: Some(added),
        tools_updated: Some(updated),
        tools_removed: Some(removed),
        total_tools: Some(plugin.manifest().tools.len() as i32),
        synced_at: Some(Utc::now()),
        error: None,
    })
}

async fn remove_tool_ids<C: sea_orm::ConnectionTrait>(
    database: &C,
    ids: Vec<Uuid>,
) -> Result<(), AuthError> {
    if ids.is_empty() {
        return Ok(());
    }
    mcp_access_policies::Entity::delete_many()
        .filter(mcp_access_policies::Column::ToolId.is_in(ids.clone()))
        .exec(database)
        .await
        .map_err(database_error)?;
    mcp_server_tools::Entity::delete_many()
        .filter(mcp_server_tools::Column::ToolId.is_in(ids.clone()))
        .exec(database)
        .await
        .map_err(database_error)?;
    mcp_tools::Entity::delete_many()
        .filter(mcp_tools::Column::Id.is_in(ids))
        .exec(database)
        .await
        .map_err(database_error)?;
    Ok(())
}

async fn remove_tools<C: sea_orm::ConnectionTrait>(
    database: &C,
    server: Uuid,
) -> Result<(), AuthError> {
    let tools = mcp_tools::Entity::find()
        .filter(mcp_tools::Column::ServerId.eq(server))
        .all(database)
        .await
        .map_err(database_error)?;
    remove_tool_ids(database, tools.into_iter().map(|tool| tool.id).collect()).await
}

pub async fn delete_server(state: &SharedState, id: Uuid, actor: Uuid) -> Result<(), AuthError> {
    let _guard = tokio::time::timeout(Duration::from_secs(5), CONFIG_LOCKS.lock(Uuid::nil(), id))
        .await
        .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    let transaction = state.database.begin().await.map_err(database_error)?;
    let previous = mcp_servers::Entity::find_by_id(id)
        .lock_exclusive()
        .one(&transaction)
        .await
        .map_err(database_error)?;
    if previous
        .as_ref()
        .is_some_and(|server| server.connection_config.get("connector_plugin").is_some())
    {
        remove_tools(&transaction, id).await?;
        mcp_access_policies::Entity::delete_many()
            .filter(mcp_access_policies::Column::ServerId.eq(id))
            .exec(&transaction)
            .await
            .map_err(database_error)?;
        mcp_connections::Entity::delete_many()
            .filter(mcp_connections::Column::ServerId.eq(id))
            .exec(&transaction)
            .await
            .map_err(database_error)?;
        mcp_oauth_states::Entity::delete_many()
            .filter(mcp_oauth_states::Column::ServerId.eq(id))
            .exec(&transaction)
            .await
            .map_err(database_error)?;
        project_mcp_servers::Entity::delete_many()
            .filter(project_mcp_servers::Column::ServerId.eq(id))
            .exec(&transaction)
            .await
            .map_err(database_error)?;
        record_auth_event(
            &transaction,
            "mcp.plugin_deleted",
            Some(actor),
            serde_json::json!({"server_id":id}),
        )
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    }
    mcp_servers::Entity::delete_by_id(id)
        .exec(&transaction)
        .await
        .map_err(database_error)?;
    transaction.commit().await.map_err(database_error)?;
    state.remove_mcp_client(&id).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::mcp_servers::McpTransportType;
    use crate::services::mcp_client::McpServerClient;
    use serde_json::json;

    fn config(base: &str, auth: &str) -> Value {
        let mut manifest: Value =
            serde_json::from_str(include_str!("../../connector-server-plugin/example.json"))
                .unwrap();
        manifest["base_url"] = json!(base);
        manifest["auth"] = json!(auth);
        let mut config = json!({"connector_plugin":{"manifest":manifest}});
        if auth == "oauth2" {
            config["oauth"] = json!({"auth_url":"https://login.microsoftonline.com/organizations/oauth2/v2.0/authorize","token_url":"https://login.microsoftonline.com/organizations/oauth2/v2.0/token","redirect_url":"http://localhost:5173/mcp/oauth/callback","scopes":["Mail.Read"]});
        }
        config
    }

    #[test]
    fn existing_mcp_servers_are_not_reinterpreted_as_plugins() {
        assert!(
            compile_plugin(
                &json!({"command":"sqlx-mcp"}),
                McpTransportType::Stdio,
                None
            )
            .unwrap()
            .is_none()
        );
        assert!(
            compile_plugin(
                &json!({"oauth":{}}),
                McpTransportType::Http,
                Some("https://example.com/mcp")
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn rejects_mixed_or_inconsistent_plugin_configuration() {
        let base = "https://graph.microsoft.com/v1.0";
        let value = config(base, "oauth2");
        assert!(compile_plugin(&value, McpTransportType::Stdio, Some(base)).is_err());
        assert!(
            compile_plugin(&value, McpTransportType::Http, Some("https://example.com")).is_err()
        );
        assert!(compile_plugin(&value, McpTransportType::Http, None).is_err());
        for key in [
            "grengin_connector_key",
            "command",
            "transport_config",
            "db_url",
        ] {
            let mut value = value.clone();
            value[key] = json!("not-allowed");
            assert!(
                compile_plugin(&value, McpTransportType::Http, Some(base)).is_err(),
                "{key}"
            );
        }
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove("oauth");
        assert!(compile_plugin(&missing, McpTransportType::Http, Some(base)).is_err());
        let mut public = config(base, "none");
        public["oauth"] = json!({});
        assert!(compile_plugin(&public, McpTransportType::Http, Some(base)).is_err());
    }

    #[test]
    fn rejects_incomplete_or_unsafe_oauth_metadata() {
        let base = "https://graph.microsoft.com/v1.0";
        for (field, value) in [
            ("auth_url", json!("http://example.com/authorize")),
            ("token_url", json!("https://169.254.169.254/token")),
            ("redirect_url", json!("http://evil.example/callback")),
            (
                "redirect_url",
                json!("https://user:password@example.com/callback"),
            ),
            (
                "redirect_url",
                json!("https://example.com/callback#fragment"),
            ),
            ("scopes", json!(["bad scope"])),
            ("scopes", json!(["Mail.Read Files.Read"])),
        ] {
            let mut value_config = config(base, "oauth2");
            value_config["oauth"][field] = value;
            assert!(
                compile_plugin(&value_config, McpTransportType::Http, Some(base)).is_err(),
                "{field}"
            );
        }
        let mut value = config(base, "oauth2");
        value["oauth"] = json!({});
        assert!(compile_plugin(&value, McpTransportType::Http, Some(base)).is_err());
    }

    #[test]
    fn tools_preserve_input_schema_and_read_write_annotations() {
        let base = "https://graph.microsoft.com/v1.0";
        let plugin = compile_plugin(&config(base, "oauth2"), McpTransportType::Http, Some(base))
            .unwrap()
            .unwrap();
        let tools = plugin_tools(&plugin);
        assert_eq!(tools.len(), 4);
        assert_eq!(
            tools[0].annotations.as_ref().unwrap().read_only_hint,
            Some(true)
        );
        assert_eq!(
            tools[3].annotations.as_ref().unwrap().read_only_hint,
            Some(false)
        );
        assert!(tools[1].input_schema.contains_key("required"));
    }

    #[tokio::test]
    async fn installed_plugin_metadata_is_listed_locally_without_oauth_or_network() {
        let base = "https://graph.microsoft.com/v1.0";
        let client = McpServerClient::new(
            Uuid::new_v4(),
            "Plugin".into(),
            McpTransportType::Http,
            Some(base.into()),
            config(base, "oauth2"),
            None,
        );
        assert_eq!(client.list_tools().await.unwrap().len(), 4);
        let error = client
            .call_tool("list_mail", json!({}), None, Some(Uuid::new_v4()))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("authentication required"));
    }

    #[tokio::test]
    async fn invalid_persisted_plugin_fails_closed_without_mcp_fallback() {
        let base = "http://127.0.0.1:1";
        let client = McpServerClient::new(
            Uuid::new_v4(),
            "Invalid".into(),
            McpTransportType::Http,
            Some(base.into()),
            json!({"connector_plugin":null}),
            None,
        );
        assert!(
            client
                .list_tools()
                .await
                .unwrap_err()
                .to_string()
                .contains("config")
        );
        assert!(
            client
                .call_tool(
                    "anything",
                    json!({}),
                    Some("token".into()),
                    Some(Uuid::new_v4())
                )
                .await
                .is_err()
        );
    }
}
