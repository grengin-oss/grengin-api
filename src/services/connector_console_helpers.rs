// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use connector_server_plugin::{
    ConnectorAuth, ConnectorManifest, ConnectorTopicGroup as ManifestTopicGroup,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DbErr, EntityTrait, FromQueryResult, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, QueryTrait, Set, TransactionTrait, sea_query::Expr,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    auth::{
        error::AuthError,
        permissions::{PERMISSION_MCP_ADMIN, PERMISSION_MCP_VIEW},
    },
    dto::{
        connector_console::*,
        mcp::{ListExecutionsQuery, McpServerAccessList, PaginatedMcpToolExecutions},
    },
    models::{connectors, mcp_connections, mcp_executions, mcp_servers, mcp_tools, users},
    services::{
        auth_audit::record_auth_event,
        authorization::{AuthorizationService, PermissionScopeMode},
        mcp_access::load_server_rules,
        mcp_helpers::{build_access_rule_dtos, to_execution_dto, to_tool_dto},
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

fn installed_topic_groups(server: &mcp_servers::Model) -> Vec<ManifestTopicGroup> {
    #[derive(Deserialize)]
    struct Presentation {
        #[serde(default)]
        topic_groups: Vec<ManifestTopicGroup>,
    }
    server
        .connection_config
        .pointer("/connector_plugin/manifest/presentation")
        .and_then(|value| Presentation::deserialize(value).ok())
        .map(|presentation| presentation.topic_groups)
        .unwrap_or_default()
}

fn topic_groups(
    definitions: Vec<ManifestTopicGroup>,
    tools: &[mcp_tools::Model],
) -> Vec<ConnectorTopicGroup> {
    let by_name: HashMap<&str, &mcp_tools::Model> = tools
        .iter()
        .map(|tool| (tool.original_name.as_str(), tool))
        .collect();
    definitions
        .into_iter()
        .filter_map(|group| {
            let members: Vec<&mcp_tools::Model> = group
                .tools
                .iter()
                .filter_map(|name| by_name.get(name.as_str()).copied())
                .collect();
            let enabled = members.iter().filter(|tool| tool.enabled).count();
            let state = if members.is_empty() {
                return None;
            } else if enabled == members.len() {
                ConnectorTopicGroupState::Enabled
            } else if enabled == 0 {
                ConnectorTopicGroupState::Disabled
            } else {
                ConnectorTopicGroupState::Mixed
            };
            Some(ConnectorTopicGroup {
                id: group.id,
                name: group.name,
                description: group.description,
                state,
                tool_ids: members.iter().map(|tool| tool.id).collect(),
            })
        })
        .collect()
}

fn database_error(error: DbErr) -> AuthError {
    eprintln!("connector console database error: {error}");
    AuthError::DbTimeout
}

fn execution_pagination(query: &ListExecutionsQuery) -> Result<(u64, u64), AuthError> {
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let offset = query.offset.unwrap_or(0);
    if offset > 1_000_000 {
        return Err(AuthError::InvalidRequest { field: "offset" });
    }
    Ok((limit, offset))
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

    pub async fn executions(
        &self,
        actor: Uuid,
        id: Uuid,
        query: ListExecutionsQuery,
    ) -> Result<PaginatedMcpToolExecutions, AuthError> {
        self.server(actor, id, PERMISSION_MCP_VIEW).await?;
        let (limit, offset) = execution_pagination(&query)?;
        let mut finder =
            mcp_executions::Entity::find().filter(mcp_executions::Column::ServerId.eq(id));
        if let Some(tool) = query.tool_name {
            finder = finder.filter(mcp_executions::Column::ToolName.eq(tool));
        }
        if let Some(is_error) = query.is_error {
            finder = finder.filter(mcp_executions::Column::IsError.eq(is_error));
        }
        if let Some(user_id) = query.user_id {
            finder = finder.filter(mcp_executions::Column::UserId.eq(user_id));
        }
        let total = finder
            .clone()
            .count(&self.state.database)
            .await
            .map_err(database_error)?;
        let executions = finder
            .order_by_desc(mcp_executions::Column::ExecutedAt)
            .order_by_desc(mcp_executions::Column::Id)
            .limit(limit)
            .offset(offset)
            .all(&self.state.database)
            .await
            .map_err(database_error)?
            .into_iter()
            .map(to_execution_dto)
            .collect();
        Ok(PaginatedMcpToolExecutions {
            executions,
            total: total as i64,
            limit: limit as i64,
            offset: offset as i64,
        })
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
        let topic_groups = topic_groups(installed_topic_groups(&server), &tools);
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
            topic_groups,
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
        let mut connections: HashMap<_, _> = self
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

    pub async fn update_topic_group(
        &self,
        actor: Uuid,
        server_id: Uuid,
        group_id: &str,
        request: ConnectorToolUpdate,
    ) -> Result<ConnectorTopicGroupUpdateResult, AuthError> {
        self.server(actor, server_id, PERMISSION_MCP_ADMIN).await?;
        let tx = self.state.database.begin().await.map_err(database_error)?;
        // Locking the server row keeps membership stable against a concurrent plugin upgrade.
        let server = mcp_servers::Entity::find_by_id(server_id)
            .lock_exclusive()
            .one(&tx)
            .await
            .map_err(database_error)?
            .ok_or(AuthError::ResourceNotFound)?;
        let group = installed_topic_groups(&server)
            .into_iter()
            .find(|group| group.id == group_id)
            .ok_or(AuthError::ResourceNotFound)?;
        let members = mcp_tools::Entity::find()
            .filter(mcp_tools::Column::ServerId.eq(server_id))
            .filter(mcp_tools::Column::OriginalName.is_in(group.tools.iter().map(String::as_str)))
            .order_by_asc(mcp_tools::Column::Name)
            .lock_exclusive()
            .all(&tx)
            .await
            .map_err(database_error)?;
        if members.is_empty() {
            return Err(AuthError::ResourceNotFound);
        }
        let now = Utc::now();
        let tool_ids: Vec<Uuid> = members.iter().map(|tool| tool.id).collect();
        mcp_tools::Entity::update_many()
            .col_expr(mcp_tools::Column::Enabled, Expr::value(request.enabled))
            .col_expr(mcp_tools::Column::UpdatedAt, Expr::value(now))
            .filter(mcp_tools::Column::Id.is_in(tool_ids.iter().copied()))
            .exec(&tx)
            .await
            .map_err(database_error)?;
        record_auth_event(
            &tx,
            "mcp.tool_group_enabled_changed",
            Some(actor),
            serde_json::json!({"server_id":server_id,"group_id":group.id,"tool_ids":tool_ids,"enabled":request.enabled}),
        )
        .await
        .map_err(|_| AuthError::DbTimeout)?;
        tx.commit().await.map_err(database_error)?;
        let tools: Vec<mcp_tools::Model> = members
            .into_iter()
            .map(|mut tool| {
                tool.enabled = request.enabled;
                tool.updated_at = now;
                tool
            })
            .collect();
        let group = topic_groups(vec![group], &tools)
            .pop()
            .ok_or(AuthError::ResourceNotFound)?;
        Ok(ConnectorTopicGroupUpdateResult {
            group,
            tools: tools.iter().map(to_tool_dto).collect(),
        })
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

    fn installed_tool(name: &str, enabled: bool) -> mcp_tools::Model {
        mcp_tools::Model {
            id: Uuid::new_v4(),
            server_id: Uuid::nil(),
            server_name: "Gmail".into(),
            name: name.into(),
            original_name: name.into(),
            description: None,
            input_schema: json!({}),
            parameters: json!({}),
            enabled,
            is_read_only: true,
            inherit_access_from_server: true,
            last_synced_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn topic_group_state_follows_installed_tools_and_skips_missing_ones() {
        let definitions = serde_json::from_value(json!([
            {"id":"messages","name":"Messages","tools":["search_messages","get_message"]},
            {"id":"threads","name":"Threads","description":"Conversation threads.","tools":["get_thread"]},
            {"id":"labels","name":"Labels","tools":["list_labels"]},
            {"id":"drafts","name":"Drafts","tools":["list_drafts"]}
        ]))
        .unwrap();
        let tools = [
            installed_tool("get_message", true),
            installed_tool("search_messages", false),
            installed_tool("get_thread", true),
            installed_tool("list_labels", false),
        ];
        let groups = topic_groups(definitions, &tools);
        let summary: Vec<_> = groups
            .iter()
            .map(|group| (group.id.as_str(), group.state, group.tool_ids.len()))
            .collect();
        assert_eq!(
            summary,
            [
                ("messages", ConnectorTopicGroupState::Mixed, 2),
                ("threads", ConnectorTopicGroupState::Enabled, 1),
                ("labels", ConnectorTopicGroupState::Disabled, 1),
            ]
        );
        assert_eq!(groups[0].tool_ids, [tools[1].id, tools[0].id]);
        assert_eq!(
            groups[1].description.as_deref(),
            Some("Conversation threads.")
        );
    }

    fn test_server() -> mcp_servers::Model {
        mcp_servers::Model {
            id: Uuid::nil(),
            name: "Gmail".into(),
            description: None,
            icon: None,
            transport_type: mcp_servers::McpTransportType::Http,
            connection_config: json!({}),
            client_id: None,
            client_secret: None,
            url: None,
            enabled: true,
            status: None,
            status_message: None,
            tool_count: 0,
            default_access: mcp_servers::McpDefaultAccess::AllUsers,
            last_connected_at: None,
            last_synced_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn installed_topic_groups_ignore_servers_without_valid_plugin_groups() {
        let mut server = mcp_servers::Model {
            connection_config: json!({"connector_plugin":{"manifest":{"presentation":{
                "suggested_prompts":["Find invoices"],
                "topic_groups":[{"id":"messages","name":"Messages","tools":["get_message"]}]
            }}}}),
            ..test_server()
        };
        assert_eq!(installed_topic_groups(&server)[0].id, "messages");
        for config in [
            json!({}),
            json!({"connector_plugin":{"manifest":{"presentation":{"suggested_prompts":[]}}}}),
            json!({"connector_plugin":{"manifest":{"presentation":{"topic_groups":"invalid"}}}}),
        ] {
            server.connection_config = config;
            assert!(installed_topic_groups(&server).is_empty());
        }
    }

    #[test]
    fn execution_pagination_keeps_non_page_aligned_offsets() {
        let query = serde_json::from_value(json!({"limit": 2, "offset": 1})).unwrap();
        assert_eq!(execution_pagination(&query).unwrap(), (2, 1));
    }

    #[test]
    fn execution_pagination_preserves_legacy_limit_clamping() {
        for (input, expected) in [
            (json!({}), 50),
            (json!({"limit": 0}), 1),
            (json!({"limit": u64::MAX}), 100),
        ] {
            let query = serde_json::from_value(input).unwrap();
            assert_eq!(execution_pagination(&query).unwrap(), (expected, 0));
        }
    }

    #[test]
    fn execution_pagination_bounds_offsets_before_database_casts() {
        for offset in [1_000_001, u64::MAX] {
            let query = serde_json::from_value(json!({"offset": offset})).unwrap();
            assert!(matches!(
                execution_pagination(&query),
                Err(AuthError::InvalidRequest { field: "offset" })
            ));
        }
        let query = serde_json::from_value(json!({"offset": 1_000_000})).unwrap();
        assert_eq!(execution_pagination(&query).unwrap(), (50, 1_000_000));
    }

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
