// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    dto::connectors::{
        BrokerStatus, BrokerToken, CompleteConnectorRequest, ConnectConnectorRequest,
        ConnectorConnection,
    },
    error::AppError,
    models::{
        connectors::{self, ConnectorStatus},
        mcp_access_policies::McpPermission,
        mcp_connections,
        mcp_servers::{self, McpTransportType},
        users::{self, UserStatus},
    },
    services::{
        auth_audit::record_auth_event,
        connector_broker::{BrokerIssue, ConnectorBroker, ConnectorError, random_secret},
        mcp_access::{build_access_context, load_server_rules, resolve_server_access_with_rules},
        mcp_oauth::{OAuthRefreshLocks, StoredToken, seal_token},
    },
    state::SharedState,
};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, QueryTrait, Set, TransactionTrait,
};
use serde_json::json;
use std::{
    sync::LazyLock,
    time::{Duration, Instant},
};
use uuid::Uuid;

static LOCKS: LazyLock<OAuthRefreshLocks> = LazyLock::new(OAuthRefreshLocks::default);
static CREATION_LOCKS: LazyLock<OAuthRefreshLocks> = LazyLock::new(OAuthRefreshLocks::default);
static START_LIMITER: LazyLock<crate::services::connector_limits::ConnectorStartLimiter> =
    LazyLock::new(Default::default);

fn database_error(error: sea_orm::DbErr) -> AppError {
    eprintln!("connector database operation failed: {error}");
    if matches!(
        error.sql_err(),
        Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
    ) {
        return AppError::DbConflict;
    }
    AppError::DbTimeout
}

pub fn valid_catalog_selection(key: &str, version: &str) -> bool {
    let key_valid = !key.is_empty()
        && key.len() <= 63
        && key.as_bytes()[0].is_ascii_lowercase()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    let parts = version.split('.').collect::<Vec<_>>();
    key_valid
        && parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.len() <= 9
                && (part.len() == 1 || !part.starts_with('0'))
                && part.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn lock_id(key: &str) -> Uuid {
    let digest = openssl::sha::sha256(key.as_bytes());
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

fn public(row: connectors::Model) -> ConnectorConnection {
    ConnectorConnection {
        id: row.id,
        connector_key: row.connector_key,
        catalog_version: row.catalog_version,
        server_id: row.server_id,
        status: row.status.clone(),
        revocation_pending: matches!(
            row.status,
            ConnectorStatus::Revoked | ConnectorStatus::Expired | ConnectorStatus::Failed
        ) && row.encrypted_grant.is_some(),
        provider_revocation: None,
        scopes: serde_json::from_value(row.scopes).unwrap_or_default(),
        expires_at: row.expires_at,
        connected_at: row.connected_at,
        authorization_url: (row.status == ConnectorStatus::Pending)
            .then_some(row.authorization_url)
            .flatten(),
    }
}

async fn owned(state: &SharedState, user: Uuid, id: Uuid) -> Result<connectors::Model, AppError> {
    connectors::Entity::find_by_id(id)
        .filter(connectors::Column::UserId.eq(user))
        .one(&state.database)
        .await
        .map_err(database_error)?
        .ok_or(AppError::ResourceNotFound)
}

fn grant(state: &SharedState, row: &connectors::Model) -> Result<String, AppError> {
    let stored = row
        .encrypted_grant
        .as_deref()
        .ok_or(AppError::ResourceNotFound)?;
    match StoredToken::open(&state.settings.auth.app_key, stored)
        .map_err(|_| AppError::ServiceTemporarilyUnavailable)?
    {
        StoredToken::Sealed(secret) => Ok(secret),
        StoredToken::LegacyPlaintext(_) => Err(AppError::ServiceTemporarilyUnavailable),
    }
}

async fn allowed_server(
    state: &SharedState,
    user: Uuid,
    id: Uuid,
    expected_url: Option<&str>,
) -> Result<(), AppError> {
    let server = mcp_servers::Entity::find_by_id(id)
        .one(&state.database)
        .await
        .map_err(database_error)?
        .ok_or(AppError::ResourceNotFound)?;
    if !server.enabled || server.transport_type != McpTransportType::Http {
        return Err(AppError::ResourceNotFound);
    }
    if !server
        .connection_config
        .get("oauth")
        .is_some_and(serde_json::Value::is_object)
    {
        return Err(AppError::ResourceNotFound);
    }
    if let Some(expected) = expected_url
        && (url::Url::parse(&server.url.clone().unwrap_or_default()).ok()
            != url::Url::parse(expected).ok()
            || url::Url::parse(expected).is_err())
    {
        return Err(AppError::ResourceNotFound);
    }
    let context = build_access_context(&state.database, user).await?;
    let rules = load_server_rules(&state.database, &[id]).await?;
    if resolve_server_access_with_rules(&state.database, &context, &server, &rules)
        .await?
        .permission
        == McpPermission::Denied
    {
        return Err(AppError::ResourceNotFound);
    }
    Ok(())
}

async fn save_status(
    state: &SharedState,
    user: Uuid,
    row: connectors::Model,
    reply: BrokerStatus,
) -> Result<connectors::Model, AppError> {
    if reply.authorization_url.as_ref().is_some_and(|raw| {
        url::Url::parse(raw).map_or(true, |url| {
            url.scheme() != "https"
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
        })
    }) {
        return Err(AppError::ServiceTemporarilyUnavailable);
    }
    if reply.transport.as_deref() != Some("http")
        || reply
            .mcp_url
            .as_deref()
            .and_then(|url| url::Url::parse(url).ok())
            .is_none_or(|url| url.scheme() != "https")
    {
        return Err(AppError::ServiceTemporarilyUnavailable);
    }
    if let Some(server) = row.server_id {
        allowed_server(state, user, server, reply.mcp_url.as_deref()).await?;
    }
    if let Some(previous) = row.mcp_url.as_ref()
        && reply.mcp_url.as_ref() != Some(previous)
    {
        return Err(AppError::ServiceTemporarilyUnavailable);
    }
    let changed = row.status != reply.status;
    let id = row.id;
    let newly_connected = reply.status == ConnectorStatus::Connected && row.connected_at.is_none();
    let mut model: connectors::ActiveModel = row.into();
    if newly_connected {
        model.connected_at = Set(Some(Utc::now()));
    }
    model.status = Set(reply.status.clone());
    model.mcp_url = Set(reply.mcp_url);
    model.authorization_url = Set(if reply.status == ConnectorStatus::Pending {
        reply.authorization_url
    } else {
        None
    });
    model.expires_at = Set(reply.expires_at);
    model.scopes = Set(json!(reply.scopes));
    model.updated_at = Set(Utc::now());
    if !reply.revocation_pending
        && matches!(
            reply.status,
            ConnectorStatus::Revoked | ConnectorStatus::Expired | ConnectorStatus::Failed
        )
    {
        model.encrypted_grant = Set(None);
    }
    let transaction = state.database.begin().await.map_err(database_error)?;
    let saved = model.update(&transaction).await.map_err(database_error)?;
    if changed {
        record_auth_event(
            &transaction,
            "connector.status_changed",
            Some(user),
            json!({"connection_id":id,"status":reply.status}),
        )
        .await
        .map_err(|_| AppError::DbTimeout)?;
    }
    transaction.commit().await.map_err(database_error)?;
    Ok(saved)
}

pub async fn list_connections(
    state: &SharedState,
    user: Uuid,
) -> Result<Vec<ConnectorConnection>, AppError> {
    Ok(connectors::Entity::find()
        .filter(connectors::Column::UserId.eq(user))
        .all(&state.database)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(public)
        .collect())
}

pub async fn start_connection(
    state: &SharedState,
    user: Uuid,
    key: String,
    request: ConnectConnectorRequest,
) -> Result<ConnectorConnection, ConnectorError> {
    if !valid_catalog_selection(&key, &request.version) {
        return Err(AppError::ResourceNotFound.into());
    }
    START_LIMITER
        .admit(user, std::time::Instant::now())
        .map_err(|retry| ConnectorError::Broker(BrokerIssue::RateLimited(retry)))?;
    let _lock = LOCKS.lock(user, lock_id(&key)).await;
    if let Some(server) = request.server_id {
        allowed_server(state, user, server, None).await?;
    }
    let broker = ConnectorBroker::from_env()?;
    let creation_lock = CREATION_LOCKS.lock(user, Uuid::nil()).await;
    let existing = connectors::Entity::find()
        .filter(connectors::Column::UserId.eq(user))
        .filter(connectors::Column::ConnectorKey.eq(&key))
        .one(&state.database)
        .await
        .map_err(database_error)?;
    let mut row = if let Some(existing) = existing {
        if existing.catalog_version != request.version || existing.server_id != request.server_id {
            return Err(AppError::DbConflict.into());
        }
        existing
    } else {
        if connectors::Entity::find()
            .filter(connectors::Column::UserId.eq(user))
            .count(&state.database)
            .await
            .map_err(database_error)?
            >= 64
        {
            return Err(ConnectorError::Broker(BrokerIssue::CapacityExceeded));
        }
        let timestamp = Utc::now();
        let model = connectors::ActiveModel {
            id: Set(Uuid::new_v4()),
            user_id: Set(user),
            connector_key: Set(key.clone()),
            catalog_version: Set(request.version.clone()),
            server_id: Set(request.server_id),
            status: Set(ConnectorStatus::Pending),
            encrypted_grant: Set(Some(seal_token(
                &state.settings.auth.app_key,
                &random_secret()?,
            )?)),
            authorization_url: Set(None),
            mcp_url: Set(None),
            scopes: Set(json!([])),
            expires_at: Set(None),
            connected_at: Set(None),
            created_at: Set(timestamp),
            updated_at: Set(timestamp),
        };
        let transaction = state.database.begin().await.map_err(database_error)?;
        let row = model.insert(&transaction).await.map_err(database_error)?;
        record_auth_event(
            &transaction,
            "connector.started",
            Some(user),
            json!({"connection_id":row.id,"connector_key":key,"catalog_version":request.version}),
        )
        .await
        .map_err(|_| AppError::DbTimeout)?;
        transaction.commit().await.map_err(database_error)?;
        row
    };
    drop(creation_lock);
    if matches!(
        row.status,
        ConnectorStatus::Expired | ConnectorStatus::Failed | ConnectorStatus::Revoked
    ) {
        return Err(AppError::DbConflict.into());
    }
    let body = json!({"connection_id":row.id,"user_id":user,"connector_key":key,"version":request.version,"connection_grant":grant(state,&row)?});
    let reply: BrokerStatus = broker.post("/v1/connectors/start", &body).await?;
    row = save_status(state, user, row, reply).await?;
    Ok(public(row))
}

pub async fn connection_status(
    state: &SharedState,
    user: Uuid,
    id: Uuid,
) -> Result<ConnectorConnection, ConnectorError> {
    let row = owned(state, user, id).await?;
    let _lock = LOCKS.lock(user, lock_id(&row.connector_key)).await;
    let row = owned(state, user, id).await?;
    if row.encrypted_grant.is_none() || row.status == ConnectorStatus::Revoked {
        return Ok(public(row));
    }
    let reply = ConnectorBroker::from_env()?
        .post(
            &format!("/v1/connectors/{id}/status"),
            &json!({"user_id":user,"connection_grant":grant(state,&row)?}),
        )
        .await?;
    Ok(public(save_status(state, user, row, reply).await?))
}

pub async fn disconnect_connection(
    state: &SharedState,
    user: Uuid,
    id: Uuid,
) -> Result<ConnectorConnection, ConnectorError> {
    disconnect_connection_as(state, user, id, Some(user)).await
}

async fn disconnect_connection_as(
    state: &SharedState,
    user: Uuid,
    id: Uuid,
    actor: Option<Uuid>,
) -> Result<ConnectorConnection, ConnectorError> {
    let row = owned(state, user, id).await?;
    let _lock = LOCKS.lock(user, lock_id(&row.connector_key)).await;
    let mut row = owned(state, user, id).await?;
    if row.status != ConnectorStatus::Revoked {
        let transaction = state.database.begin().await.map_err(database_error)?;
        let mut active: connectors::ActiveModel = row.into();
        active.status = Set(ConnectorStatus::Revoked);
        active.authorization_url = Set(None);
        active.updated_at = Set(Utc::now());
        row = active.update(&transaction).await.map_err(database_error)?;
        record_auth_event(
            &transaction,
            "connector.disconnect_requested",
            actor,
            json!({"connection_id":id,"owner_user_id":user,"system_cleanup":actor.is_none()}),
        )
        .await
        .map_err(|_| AppError::DbTimeout)?;
        transaction.commit().await.map_err(database_error)?;
    }
    let mut provider_revocation = None;
    if row.encrypted_grant.is_some() {
        let reply: BrokerStatus = ConnectorBroker::from_env()?
            .post(
                &format!("/v1/connectors/{id}/disconnect"),
                &json!({"user_id":user,"connection_grant":grant(state,&row)?}),
            )
            .await?;
        if reply.revocation_pending {
            let mut response = public(row);
            response.provider_revocation = reply.provider_revocation;
            return Ok(response);
        }
        provider_revocation = reply.provider_revocation;
    }
    let transaction = state.database.begin().await.map_err(database_error)?;
    if let Some(server) = row.server_id
        && let Some(legacy) = mcp_connections::Entity::find()
            .filter(mcp_connections::Column::UserId.eq(user))
            .filter(mcp_connections::Column::ServerId.eq(server))
            .one(&transaction)
            .await
            .map_err(database_error)?
    {
        let mut legacy: mcp_connections::ActiveModel = legacy.into();
        legacy.connected = Set(false);
        legacy.connected_at = Set(None);
        legacy.access_token = Set(None);
        legacy.refresh_token = Set(None);
        legacy.expires_at = Set(None);
        legacy.updated_at = Set(Utc::now());
        legacy.update(&transaction).await.map_err(database_error)?;
    }
    connectors::Entity::delete_by_id(id)
        .exec(&transaction)
        .await
        .map_err(database_error)?;
    record_auth_event(
        &transaction,
        "connector.disconnected",
        actor,
        json!({"connection_id":id,"owner_user_id":user,"system_cleanup":actor.is_none()}),
    )
    .await
    .map_err(|_| AppError::DbTimeout)?;
    transaction.commit().await.map_err(database_error)?;
    let mut result = public(row);
    result.status = ConnectorStatus::Revoked;
    result.revocation_pending = false;
    result.provider_revocation = provider_revocation;
    result.authorization_url = None;
    Ok(result)
}

fn cleanup_interval(raw: Option<&str>) -> Duration {
    Duration::from_secs(
        raw.and_then(|value| value.parse::<u64>().ok())
            .filter(|value| (5..=3600).contains(value))
            .unwrap_or(60),
    )
}

pub fn spawn_deleted_connector_cleanup(state: SharedState) {
    if ConnectorBroker::from_env().is_err() {
        return;
    }
    let period = cleanup_interval(
        std::env::var("GRENGIN_CONNECTOR_CLEANUP_INTERVAL_SECS")
            .ok()
            .as_deref(),
    );
    tokio::spawn(async move {
        let mut ticks = tokio::time::interval(period);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticks.tick().await;
            if cleanup_deleted_connections(&state).await.is_err() {
                eprintln!(
                    "connector deleted-owner cleanup failed; retained credentials will be retried"
                );
            }
        }
    });
}

async fn cleanup_deleted_connections(state: &SharedState) -> Result<(), AppError> {
    let deleted = users::Entity::find()
        .select_only()
        .column(users::Column::Id)
        .filter(users::Column::Status.eq(UserStatus::Deleted))
        .into_query();
    let rows = connectors::Entity::find()
        .filter(connectors::Column::UserId.in_subquery(deleted))
        .order_by_asc(connectors::Column::UpdatedAt)
        .limit(5)
        .all(&state.database)
        .await
        .map_err(database_error)?;
    let started = Instant::now();
    for row in rows {
        if started.elapsed() >= Duration::from_secs(20) {
            break;
        }
        let result = disconnect_connection_as(state, row.user_id, row.id, None).await;
        if !matches!(&result,Ok(response) if !response.revocation_pending) {
            // Rotate retries so one unavailable provider cannot starve other deleted owners.
            connectors::ActiveModel {
                id: Set(row.id),
                updated_at: Set(Utc::now()),
                ..Default::default()
            }
            .update(&state.database)
            .await
            .map_err(database_error)?;
        }
    }
    Ok(())
}

pub async fn complete_connection(
    state: &SharedState,
    user: Uuid,
    id: Uuid,
    request: CompleteConnectorRequest,
) -> Result<ConnectorConnection, ConnectorError> {
    if request.confirmation_code.len() != 64
        || !request
            .confirmation_code
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ConnectorError::Broker(BrokerIssue::InvalidRequest));
    }
    let row = owned(state, user, id).await?;
    let _lock = LOCKS.lock(user, lock_id(&row.connector_key)).await;
    let row = owned(state, user, id).await?;
    if row.status == ConnectorStatus::Revoked || row.encrypted_grant.is_none() {
        return Err(AppError::DbConflict.into());
    }
    let reply:BrokerStatus=ConnectorBroker::from_env()?.post(&format!("/v1/connectors/{id}/complete"),
        &json!({"user_id":user,"connection_grant":grant(state,&row)?,"confirmation_code":request.confirmation_code})).await?;
    Ok(public(save_status(state, user, row, reply).await?))
}

pub async fn resolve_mcp_connection_token(
    state: &SharedState,
    user: Uuid,
    server: Uuid,
    client_url: Option<&str>,
) -> Result<Option<Option<String>>, AppError> {
    let row = connectors::Entity::find()
        .filter(connectors::Column::UserId.eq(user))
        .filter(connectors::Column::ServerId.eq(server))
        .one(&state.database)
        .await
        .map_err(database_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let _lock = LOCKS.lock(user, lock_id(&row.connector_key)).await;
    let mut row = owned(state, user, row.id).await?;
    allowed_server(state, user, server, row.mcp_url.as_deref()).await?;
    if matches!(
        row.status,
        ConnectorStatus::Pending | ConnectorStatus::AwaitingConfirmation
    ) {
        let reply = ConnectorBroker::from_env()?
            .post(
                &format!("/v1/connectors/{}/status", row.id),
                &json!({"user_id":user,"connection_grant":grant(state,&row)?}),
            )
            .await?;
        row = save_status(state, user, row, reply).await?;
    }
    if row.status != ConnectorStatus::Connected {
        return Ok(Some(None));
    }
    if !same_mcp_endpoint(row.mcp_url.as_deref(), client_url) {
        return Err(AppError::ResourceNotFound);
    }
    let token: BrokerToken = ConnectorBroker::from_env()?
        .post(
            &format!("/v1/connectors/{}/token", row.id),
            &json!({"user_id":user,"connection_grant":grant(state,&row)?}),
        )
        .await?;
    if token.access_token.trim().is_empty()
        || token.access_token.len() > 16384
        || token.expires_at.is_some_and(|expiry| expiry <= Utc::now())
    {
        return Err(AppError::ServiceTemporarilyUnavailable);
    }
    Ok(Some(Some(token.access_token)))
}

fn same_mcp_endpoint(expected: Option<&str>, actual: Option<&str>) -> bool {
    let parse = |raw: Option<&str>| raw.and_then(|url| url::Url::parse(url).ok());
    match (parse(expected), parse(actual)) {
        (Some(expected), Some(actual)) => expected.scheme() == "https" && expected == actual,
        _ => false,
    }
}

pub async fn pending_authorization(
    state: &SharedState,
    user: Uuid,
    server: Uuid,
) -> Result<Option<String>, AppError> {
    let row = connectors::Entity::find()
        .filter(connectors::Column::UserId.eq(user))
        .filter(connectors::Column::ServerId.eq(server))
        .one(&state.database)
        .await
        .map_err(database_error)?;
    if let Some(row) = row {
        let status = connection_status(state, user, row.id).await?;
        return status
            .authorization_url
            .map(Some)
            .ok_or(AppError::ServiceTemporarilyUnavailable);
    }
    Ok(None)
}

pub async fn cached_mcp_statuses(
    state: &SharedState,
    user: Uuid,
) -> Result<std::collections::HashMap<Uuid, bool>, AppError> {
    Ok(connectors::Entity::find()
        .filter(connectors::Column::UserId.eq(user))
        .all(&state.database)
        .await
        .map_err(database_error)?
        .into_iter()
        .filter_map(|row| {
            row.server_id
                .map(|id| (id, row.status == ConnectorStatus::Connected))
        })
        .collect())
}

pub async fn merge_mcp_connections(
    state: &SharedState,
    user: Uuid,
    mut existing: Vec<crate::dto::mcp::McpUserConnection>,
) -> Result<Vec<crate::dto::mcp::McpUserConnection>, AppError> {
    let rows = connectors::Entity::find()
        .filter(connectors::Column::UserId.eq(user))
        .all(&state.database)
        .await
        .map_err(database_error)?;
    let ids = rows
        .iter()
        .filter_map(|row| row.server_id)
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Ok(existing);
    }
    let servers = mcp_servers::Entity::find()
        .filter(mcp_servers::Column::Id.is_in(ids))
        .all(&state.database)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|server| (server.id, server))
        .collect::<std::collections::HashMap<_, _>>();
    for row in rows {
        let Some(server) = row.server_id.and_then(|id| servers.get(&id)) else {
            continue;
        };
        existing.retain(|old| old.server_id != server.id);
        existing.push(crate::dto::mcp::McpUserConnection {
            server_id: server.id,
            server_name: server.name.clone(),
            description: server.description.clone(),
            connected: row.status == ConnectorStatus::Connected,
            connected_at: row.connected_at,
            expires_at: row.expires_at,
            scopes: serde_json::from_value(row.scopes).ok(),
        });
    }
    Ok(existing)
}

pub async fn disconnect_linked_connection(
    state: &SharedState,
    user: Uuid,
    server: Uuid,
) -> Result<(), AppError> {
    let row = connectors::Entity::find()
        .filter(connectors::Column::UserId.eq(user))
        .filter(connectors::Column::ServerId.eq(server))
        .one(&state.database)
        .await
        .map_err(database_error)?;
    if let Some(row) = row {
        disconnect_connection(state, user, row.id).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleted_owner_cleanup_interval_is_bounded_and_defaults_safely() {
        for value in [
            None,
            Some(""),
            Some("0"),
            Some("4"),
            Some("3601"),
            Some("invalid"),
            Some("-1"),
        ] {
            assert_eq!(cleanup_interval(value), Duration::from_secs(60));
        }
        assert_eq!(cleanup_interval(Some("5")), Duration::from_secs(5));
        assert_eq!(cleanup_interval(Some("3600")), Duration::from_secs(3600));
    }

    #[test]
    fn broker_credentials_bind_to_the_actual_client_not_just_the_current_database_row() {
        let expected = Some("https://vendor.test/mcp");
        assert!(same_mcp_endpoint(
            expected,
            Some("https://vendor.test:443/mcp")
        ));
        for url in [
            None,
            Some("https://other.test/mcp"),
            Some("https://vendor.test/other"),
            Some("http://vendor.test/mcp"),
        ] {
            assert!(!same_mcp_endpoint(expected, url));
        }
        let stale = crate::services::mcp_client::McpServerClient::new(
            Uuid::new_v4(),
            "stale client".into(),
            McpTransportType::Sse,
            expected.map(str::to_owned),
            json!({"sse_url":"https://other.test/events"}),
            None,
        );
        assert!(!same_mcp_endpoint(
            expected,
            stale.http_url_for_transport().ok()
        ));
    }

    #[test]
    fn catalog_selection_cannot_inject_paths_or_ambiguous_versions() {
        for (key, version) in [
            ("../google", "1.0.0"),
            ("Google", "1.0.0"),
            ("google", "01.0.0"),
            ("google", "1.0"),
            ("google", "1.0.0/../../x"),
        ] {
            assert!(!valid_catalog_selection(key, version));
        }
        assert!(valid_catalog_selection("google-calendar", "1.0.0"));
    }

    #[test]
    fn public_connection_never_serializes_credentials() {
        let row = connectors::Model {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            connector_key: "gmail".into(),
            catalog_version: "1.0.0".into(),
            server_id: None,
            status: ConnectorStatus::Connected,
            encrypted_grant: Some("SECRET".into()),
            authorization_url: Some("SECRET_URL".into()),
            mcp_url: None,
            scopes: json!([]),
            expires_at: None,
            connected_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let encoded = serde_json::to_string(&public(row)).unwrap();
        assert!(!encoded.contains("SECRET"));
        assert!(!encoded.contains("grant"));
        assert!(!encoded.contains("user_id"));
    }

    #[test]
    fn completion_payload_uses_only_snake_case_and_rejects_extra_fields() {
        assert!(
            serde_json::from_value::<CompleteConnectorRequest>(
                json!({"confirmation_code":"a".repeat(64)})
            )
            .is_ok()
        );
        assert!(
            serde_json::from_value::<CompleteConnectorRequest>(
                json!({"confirmationCode":"a".repeat(64)})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<CompleteConnectorRequest>(
                json!({"confirmation_code":"a".repeat(64),"user_id":Uuid::new_v4()})
            )
            .is_err()
        );
    }

    #[tokio::test]
    #[ignore = "requires the disposable connectors regression database"]
    async fn connector_migration_round_trip() {
        use migration::MigratorTrait;
        let raw =
            std::env::var("GRENGIN_CONNECTOR_TEST_DATABASE_URL").expect("test database required");
        let url = url::Url::parse(&raw).unwrap();
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        assert_eq!(url.path(), "/grengin_connectors_test");
        let db = sea_orm::Database::connect(&raw).await.unwrap();
        migration::Migrator::up(&db, None).await.unwrap();
        migration::Migrator::down(&db, Some(1)).await.unwrap();
        assert!(connectors::Entity::find().all(&db).await.is_err());
        migration::Migrator::up(&db, None).await.unwrap();
        assert!(
            connectors::Entity::find()
                .all(&db)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
