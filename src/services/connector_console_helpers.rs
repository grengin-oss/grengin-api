// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Duration, Utc};
use connector_server_plugin::{ConnectorAuth, ConnectorManifest};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DbErr, EntityTrait, FromQueryResult, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, QueryTrait, Set, TransactionTrait,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    auth::{
        error::AuthError,
        permissions::{PERMISSION_MCP_ADMIN, PERMISSION_MCP_VIEW},
    },
    dto::{connector_console::*, mcp::McpServerAccessList},
    models::{connectors, mcp_connections, mcp_executions, mcp_servers, mcp_tools, users},
    services::{
        auth_audit::record_auth_event,
        authorization::{AuthorizationService, PermissionScopeMode},
        mcp_access::load_server_rules,
        mcp_helpers::{build_access_rule_dtos, to_tool_dto},
        mcp_service::map_mcp_access_error,
    },
    state::SharedState,
};

#[derive(Deserialize)]
struct InstalledPluginIdentity {
    manifest: ConnectorManifest,
    auth_mode: Option<ConnectorAuth>,
}

pub fn integration_metadata(server: &mcp_servers::Model) -> ConnectorIntegration {
    use ConnectorAuthenticationMethod as Method;
    let mut metadata = ConnectorIntegration {
        suggested_prompts: Vec::new(),
        plugin_id: None,
        plugin_version: None,
        authentication: Method::ServerManaged,
        supported_authentication: vec![Method::ServerManaged],
        per_user_credentials: false,
    };
    if let Some(raw) = server.connection_config.get("connector_plugin") {
        metadata.authentication = Method::Unavailable;
        metadata.supported_authentication.clear();
        if let Ok(identity) = serde_json::from_value::<InstalledPluginIdentity>(raw.clone()) {
            let primary = identity.manifest.auth;
            let Some(selected) = primary.resolve_mode(identity.auth_mode) else {
                return metadata;
            };
            let method = match selected {
                ConnectorAuth::Oauth2 => Method::Oauth2,
                ConnectorAuth::ApiKey => Method::ApiToken,
                ConnectorAuth::None => Method::ServerManaged,
            };
            metadata.plugin_id = Some(identity.manifest.id);
            metadata.plugin_version = Some(identity.manifest.version);
            metadata.suggested_prompts = identity
                .manifest
                .presentation
                .map(|p| {
                    p.suggested_prompts
                        .into_iter()
                        .filter(|p| {
                            !p.trim().is_empty()
                                && p.chars().count() <= 240
                                && !p.chars().any(char::is_control)
                        })
                        .take(8)
                        .collect()
                })
                .unwrap_or_default();
            metadata.authentication = method;
            metadata.supported_authentication = if primary == ConnectorAuth::Oauth2 {
                vec![Method::Oauth2, Method::ApiToken]
            } else {
                vec![method]
            };
            metadata.per_user_credentials = selected != ConnectorAuth::None;
        }
    } else if server
        .connection_config
        .get("grengin_connector_key")
        .is_some()
    {
        metadata.authentication = Method::GrenginBroker;
        metadata.supported_authentication = vec![Method::GrenginBroker];
        metadata.per_user_credentials = true;
    } else if server.connection_config.get("oauth").is_some() {
        metadata.authentication = Method::Oauth2;
        metadata.supported_authentication = vec![Method::Oauth2];
        metadata.per_user_credentials = true;
    }
    metadata
}

#[derive(Clone, FromQueryResult)]
struct ConnectionSummary {
    #[sea_orm(from_alias = "userId")]
    user_id: Uuid,
    connected: bool,
    #[sea_orm(from_alias = "connectedAt")]
    connected_at: Option<DateTime<Utc>>,
    #[sea_orm(from_alias = "expiresAt")]
    expires_at: Option<DateTime<Utc>>,
    has_token: bool,
    can_refresh: bool,
}

impl ConnectionSummary {
    fn state(&self, now: DateTime<Utc>) -> ConnectorConnectionState {
        connection_state(
            self.connected,
            self.has_token,
            self.can_refresh,
            self.expires_at,
            now,
        )
    }
}

pub fn connection_state(
    connected: bool,
    has_token: bool,
    can_refresh: bool,
    expiry: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> ConnectorConnectionState {
    if !connected {
        return ConnectorConnectionState::NotConnected;
    }
    if !has_token || (expiry.is_some_and(|expiry| expiry <= now) && !can_refresh) {
        return ConnectorConnectionState::NeedsReconnect;
    }
    ConnectorConnectionState::Connected
}

pub fn catalog_connection_state(
    server: &mcp_servers::Model,
    integration: &ConnectorIntegration,
    connection: Option<&mcp_connections::Model>,
    broker_connected: Option<bool>,
    now: DateTime<Utc>,
) -> ConnectorConnectionState {
    if !server.enabled || integration.authentication == ConnectorAuthenticationMethod::Unavailable {
        return ConnectorConnectionState::Unavailable;
    }
    if let Some(connected) = broker_connected {
        return if connected {
            ConnectorConnectionState::Connected
        } else {
            ConnectorConnectionState::NotConnected
        };
    }
    if !integration.per_user_credentials {
        return if server.status.as_deref() == Some("connected") {
            ConnectorConnectionState::Connected
        } else {
            ConnectorConnectionState::NotConnected
        };
    }
    connection
        .map(|row| {
            connection_state(
                row.connected,
                row.access_token
                    .as_ref()
                    .is_some_and(|value| !value.is_empty()),
                row.refresh_token
                    .as_ref()
                    .is_some_and(|value| !value.is_empty()),
                row.expires_at,
                now,
            )
        })
        .unwrap_or(ConnectorConnectionState::NotConnected)
}

fn database_error(error: DbErr) -> AuthError {
    eprintln!("connector console database error: {error}");
    AuthError::DbTimeout
}

pub struct ConnectorConsoleService<'a> {
    state: &'a SharedState,
}

impl<'a> ConnectorConsoleService<'a> {
    pub fn new(state: &'a SharedState) -> Self {
        Self { state }
    }

    async fn server(
        &self,
        actor: Uuid,
        id: Uuid,
        permission: &str,
    ) -> Result<mcp_servers::Model, AuthError> {
        AuthorizationService::new(&self.state.database)
            .ensure_permission(
                actor,
                permission,
                None,
                PermissionScopeMode::RequireOrgWide,
                Some(id),
            )
            .await?;
        mcp_servers::Entity::find_by_id(id)
            .one(&self.state.database)
            .await
            .map_err(database_error)?
            .ok_or(AuthError::ResourceNotFound)
    }

    async fn connection_rows(
        &self,
        server: &mcp_servers::Model,
        users: &[Uuid],
    ) -> Result<Vec<ConnectionSummary>, AuthError> {
        if users.is_empty() {
            return Ok(Vec::new());
        }
        if server
            .connection_config
            .get("grengin_connector_key")
            .is_some()
        {
            #[derive(FromQueryResult)]
            struct BrokerRow {
                #[sea_orm(from_alias = "userId")]
                user_id: Uuid,
                status: connectors::ConnectorStatus,
                #[sea_orm(from_alias = "connectedAt")]
                connected_at: Option<DateTime<Utc>>,
                #[sea_orm(from_alias = "expiresAt")]
                expires_at: Option<DateTime<Utc>>,
            }
            return Ok(connectors::Entity::find()
                .select_only()
                .columns([
                    connectors::Column::UserId,
                    connectors::Column::Status,
                    connectors::Column::ConnectedAt,
                    connectors::Column::ExpiresAt,
                ])
                .filter(connectors::Column::ServerId.eq(server.id))
                .filter(connectors::Column::UserId.is_in(users.iter().copied()))
                .into_model::<BrokerRow>()
                .all(&self.state.database)
                .await
                .map_err(database_error)?
                .into_iter()
                .map(|row| ConnectionSummary {
                    user_id: row.user_id,
                    connected: row.status == connectors::ConnectorStatus::Connected,
                    connected_at: row.connected_at,
                    expires_at: row.expires_at,
                    has_token: true,
                    can_refresh: false,
                })
                .collect());
        }
        mcp_connections::Entity::find()
            .select_only()
            .columns([
                mcp_connections::Column::UserId,
                mcp_connections::Column::Connected,
                mcp_connections::Column::ConnectedAt,
                mcp_connections::Column::ExpiresAt,
            ])
            .column_as(
                sea_orm::sea_query::Expr::col(mcp_connections::Column::AccessToken)
                    .is_not_null()
                    .and(
                        sea_orm::sea_query::Expr::col(mcp_connections::Column::AccessToken).ne(""),
                    ),
                "has_token",
            )
            .column_as(
                sea_orm::sea_query::Expr::col(mcp_connections::Column::RefreshToken)
                    .is_not_null()
                    .and(
                        sea_orm::sea_query::Expr::col(mcp_connections::Column::RefreshToken).ne(""),
                    ),
                "can_refresh",
            )
            .filter(mcp_connections::Column::ServerId.eq(server.id))
            .filter(mcp_connections::Column::UserId.is_in(users.iter().copied()))
            .into_model::<ConnectionSummary>()
            .all(&self.state.database)
            .await
            .map_err(database_error)
    }

    async fn counts(
        &self,
        server: &mcp_servers::Model,
        now: DateTime<Utc>,
    ) -> Result<(u64, u64), AuthError> {
        let active =
            users::Entity::find().filter(users::Column::Status.eq(users::UserStatus::Active));
        let total = active
            .clone()
            .count(&self.state.database)
            .await
            .map_err(database_error)?;
        if server
            .connection_config
            .get("grengin_connector_key")
            .is_some()
        {
            let connected_ids = connectors::Entity::find()
                .select_only()
                .column(connectors::Column::UserId)
                .filter(connectors::Column::ServerId.eq(server.id))
                .filter(connectors::Column::Status.eq(connectors::ConnectorStatus::Connected))
                .filter(
                    Condition::any()
                        .add(connectors::Column::ExpiresAt.is_null())
                        .add(connectors::Column::ExpiresAt.gt(now)),
                )
                .into_query();
            let connected = active
                .filter(users::Column::Id.in_subquery(connected_ids))
                .count(&self.state.database)
                .await
                .map_err(database_error)?;
            return Ok((total, connected));
        }
        let connected_ids = mcp_connections::Entity::find()
            .select_only()
            .column(mcp_connections::Column::UserId)
            .filter(mcp_connections::Column::ServerId.eq(server.id))
            .filter(mcp_connections::Column::Connected.eq(true))
            .filter(mcp_connections::Column::AccessToken.ne(""))
            .filter(
                Condition::any()
                    .add(mcp_connections::Column::ExpiresAt.is_null())
                    .add(mcp_connections::Column::ExpiresAt.gt(now))
                    .add(mcp_connections::Column::RefreshToken.ne("")),
            )
            .into_query();
        let connected = active
            .filter(users::Column::Id.in_subquery(connected_ids))
            .count(&self.state.database)
            .await
            .map_err(database_error)?;
        Ok((total, connected))
    }

    pub async fn overview(&self, actor: Uuid, id: Uuid) -> Result<ConnectorConsole, AuthError> {
        let server = self.server(actor, id, PERMISSION_MCP_VIEW).await?;
        let now = Utc::now();
        let (active_user_count, connected_user_count) = self.counts(&server, now).await?;
        let integration = integration_metadata(&server);
        let my_connection_state = if !server.enabled
            || integration.authentication == ConnectorAuthenticationMethod::Unavailable
        {
            ConnectorConnectionState::Unavailable
        } else if !integration.per_user_credentials {
            if server.status.as_deref() == Some("connected") {
                ConnectorConnectionState::Connected
            } else {
                ConnectorConnectionState::NotConnected
            }
        } else {
            self.connection_rows(&server, &[actor])
                .await?
                .first()
                .map(|c| c.state(now))
                .unwrap_or(ConnectorConnectionState::NotConnected)
        };
        let tools = mcp_tools::Entity::find()
            .filter(mcp_tools::Column::ServerId.eq(id))
            .order_by_asc(mcp_tools::Column::Name)
            .all(&self.state.database)
            .await
            .map_err(database_error)?;
        let period_start = now - Duration::days(7);
        let executions = mcp_executions::Entity::find()
            .filter(mcp_executions::Column::ServerId.eq(id))
            .filter(mcp_executions::Column::ExecutedAt.gte(period_start))
            .filter(mcp_executions::Column::ExecutedAt.lte(now));
        let successful_tool_calls = executions
            .clone()
            .filter(mcp_executions::Column::IsError.eq(false))
            .count(&self.state.database)
            .await
            .map_err(database_error)?;
        let failed_tool_calls = executions
            .filter(mcp_executions::Column::IsError.eq(true))
            .count(&self.state.database)
            .await
            .map_err(database_error)?;
        let rules = load_server_rules(&self.state.database, &[id])
            .await
            .map_err(map_mcp_access_error)?;
        let access = McpServerAccessList {
            server_id: id,
            server_name: server.name.clone(),
            default_access: server.default_access,
            rules: build_access_rule_dtos(&self.state.database, rules)
                .await
                .map_err(map_mcp_access_error)?,
        };
        Ok(ConnectorConsole {
            server_id: id,
            name: server.name,
            description: server.description,
            icon: server.icon,
            enabled: server.enabled,
            integration,
            my_connection_state,
            active_user_count,
            connected_user_count,
            default_access: server.default_access,
            access,
            tools: tools.iter().map(to_tool_dto).collect(),
            usage: ConnectorUsage {
                period_start,
                period_end: now,
                tool_calls: successful_tool_calls + failed_tool_calls,
                successful_tool_calls,
                failed_tool_calls,
            },
            reminders_supported: false,
            write_confirmation_supported: false,
        })
    }

    pub async fn people(
        &self,
        actor: Uuid,
        id: Uuid,
        query: ConnectorPeopleQuery,
    ) -> Result<ConnectorPeople, AuthError> {
        let server = self.server(actor, id, PERMISSION_MCP_VIEW).await?;
        let limit = query.limit.unwrap_or(20);
        let offset = query.offset.unwrap_or(0);
        if !(1..=100).contains(&limit) || offset > 1_000_000 {
            return Err(AuthError::InvalidRequest {
                field: "pagination",
            });
        }
        let now = Utc::now();
        let (total, connected) = self.counts(&server, now).await?;
        #[derive(FromQueryResult)]
        struct PersonRow {
            id: Uuid,
            email: String,
            name: Option<String>,
            picture: Option<String>,
        }
        let page = users::Entity::find()
            .select_only()
            .columns([
                users::Column::Id,
                users::Column::Email,
                users::Column::Name,
                users::Column::Picture,
            ])
            .filter(users::Column::Status.eq(users::UserStatus::Active))
            .order_by_asc(users::Column::Email)
            .order_by_asc(users::Column::Id)
            .limit(limit)
            .offset(offset)
            .into_model::<PersonRow>()
            .all(&self.state.database)
            .await
            .map_err(database_error)?;
        let mut connections: std::collections::HashMap<_, _> = self
            .connection_rows(&server, &page.iter().map(|p| p.id).collect::<Vec<_>>())
            .await?
            .into_iter()
            .map(|c| (c.user_id, c))
            .collect();
        let people = page
            .into_iter()
            .map(|person| {
                let connection = connections.remove(&person.id);
                ConnectorPerson {
                    user_id: person.id,
                    email: person.email,
                    name: person.name,
                    picture: person.picture,
                    connection_state: if !server.enabled {
                        ConnectorConnectionState::Unavailable
                    } else {
                        connection
                            .as_ref()
                            .map(|c| c.state(now))
                            .unwrap_or(ConnectorConnectionState::NotConnected)
                    },
                    connected_at: connection.as_ref().and_then(|c| c.connected_at),
                    expires_at: connection.and_then(|c| c.expires_at),
                }
            })
            .collect();
        Ok(ConnectorPeople {
            people,
            total,
            connected,
            limit,
            offset,
        })
    }

    pub async fn update_tool(
        &self,
        actor: Uuid,
        server_id: Uuid,
        tool_id: Uuid,
        request: ConnectorToolUpdate,
    ) -> Result<crate::dto::mcp::McpTool, AuthError> {
        self.server(actor, server_id, PERMISSION_MCP_ADMIN).await?;
        let tx = self.state.database.begin().await.map_err(database_error)?;
        mcp_servers::Entity::find_by_id(server_id)
            .lock_exclusive()
            .one(&tx)
            .await
            .map_err(database_error)?
            .ok_or(AuthError::ResourceNotFound)?;
        let tool = mcp_tools::Entity::find_by_id(tool_id)
            .filter(mcp_tools::Column::ServerId.eq(server_id))
            .lock_exclusive()
            .one(&tx)
            .await
            .map_err(database_error)?
            .ok_or(AuthError::ResourceNotFound)?;
        let mut update: mcp_tools::ActiveModel = tool.into();
        update.enabled = Set(request.enabled);
        update.updated_at = Set(Utc::now());
        let saved = update.update(&tx).await.map_err(database_error)?;
        record_auth_event(
            &tx,
            "mcp.tool_enabled_changed",
            Some(actor),
            serde_json::json!({"server_id":server_id,"tool_id":tool_id,"enabled":request.enabled}),
        )
        .await
        .map_err(|_| AuthError::DbTimeout)?;
        tx.commit().await.map_err(database_error)?;
        Ok(to_tool_dto(&saved))
    }
}

pub fn result_preview(value: &Value) -> McpResultPreview {
    let item_count = value.as_array().map(Vec::len).or_else(|| {
        [
            "value", "items", "results", "files", "messages", "events", "data",
        ]
        .iter()
        .find_map(|key| value.get(key).and_then(Value::as_array).map(Vec::len))
    });
    let empty = value.is_null()
        || value.as_array().is_some_and(Vec::is_empty)
        || value.as_object().is_some_and(serde_json::Map::is_empty)
        || value.as_str().is_some_and(str::is_empty);
    let mut writer = PreviewWriter {
        bytes: Vec::new(),
        truncated: false,
    };
    let text = if empty {
        std::borrow::Cow::Borrowed("No data returned")
    } else if let Some(text) = value.as_str() {
        std::borrow::Cow::Borrowed(text)
    } else {
        // Stop serialization at the preview budget instead of allocating a full vendor response.
        writer.truncated = serde_json::to_writer_pretty(&mut writer, value).is_err();
        String::from_utf8_lossy(&writer.bytes)
    };
    let truncated = writer.truncated || text.chars().nth(2048).is_some();
    McpResultPreview {
        text: text.chars().take(2048).collect(),
        truncated,
        empty,
        item_count,
    }
}

struct PreviewWriter {
    bytes: Vec<u8>,
    truncated: bool,
}

impl std::io::Write for PreviewWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let remaining = 8192usize.saturating_sub(self.bytes.len());
        if bytes.len() > remaining {
            self.bytes.extend_from_slice(&bytes[..remaining]);
            return Err(std::io::Error::other("preview limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn expired_refreshable_sessions_are_not_prematurely_disconnected() {
        let now = Utc::now();
        for (connected, token, refresh, expiry, expected) in [
            (true, true, false, None, ConnectorConnectionState::Connected),
            (
                true,
                true,
                false,
                Some(now),
                ConnectorConnectionState::NeedsReconnect,
            ),
            (
                true,
                true,
                true,
                Some(now),
                ConnectorConnectionState::Connected,
            ),
            (
                true,
                false,
                true,
                None,
                ConnectorConnectionState::NeedsReconnect,
            ),
            (
                false,
                true,
                true,
                None,
                ConnectorConnectionState::NotConnected,
            ),
        ] {
            assert_eq!(
                connection_state(connected, token, refresh, expiry, now),
                expected
            );
        }
    }

    #[test]
    fn result_previews_are_useful_bounded_and_unicode_safe() {
        assert!(result_preview(&Value::Null).empty);
        assert_eq!(result_preview(&json!({"files":[1,2]})).item_count, Some(2));
        assert!(
            result_preview(&json!({"message":"tool response"}))
                .text
                .contains("tool response")
        );
        let long = result_preview(&json!("\u{1f600}".repeat(3000)));
        assert!(long.truncated);
        assert_eq!(long.text.chars().count(), 2048);
        assert!(!result_preview(&json!(false)).empty);
        assert!(!result_preview(&json!(0)).empty);
        let nested = result_preview(&json!({"data":"x".repeat(100_000)}));
        assert!(nested.truncated);
        assert!(nested.text.len() <= 2048);
    }
}
