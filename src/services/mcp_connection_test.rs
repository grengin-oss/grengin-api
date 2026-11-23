// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm::EntityTrait;
use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::{
    auth::{encryption::decrypt_key, error::AuthError, permissions::PERMISSION_MCP_ADMIN},
    dto::mcp::McpServerTestResult,
    models::mcp_servers,
    services::{
        authorization::{AuthorizationService, PermissionScopeMode},
        mcp_client::McpServerClient,
        mcp_helpers::resolve_mcp_oauth_token,
        mcp_plugin_helpers::compile_plugin,
    },
    state::SharedState,
};

fn failed(code: &str, message: &str, started: Instant) -> McpServerTestResult {
    McpServerTestResult {
        success: false,
        message: Some(message.into()),
        latency_ms: Some(started.elapsed().as_millis().min(i32::MAX as u128) as i32),
        available_tools: None,
        error: Some(code.into()),
    }
}

pub async fn test_server(
    state: &SharedState,
    actor: Uuid,
    id: Uuid,
) -> Result<McpServerTestResult, AuthError> {
    AuthorizationService::new(&state.database)
        .ensure_permission(
            actor,
            PERMISSION_MCP_ADMIN,
            None,
            PermissionScopeMode::RequireOrgWide,
            Some(id),
        )
        .await?;
    let server = mcp_servers::Entity::find_by_id(id)
        .one(&state.database)
        .await
        .map_err(|_| AuthError::DbTimeout)?
        .ok_or(AuthError::ResourceNotFound)?;
    let started = Instant::now();
    if !server.enabled {
        return Ok(failed(
            "server_disabled",
            "Enable the server before testing its connection.",
            started,
        ));
    }
    match compile_plugin(
        &server.connection_config,
        server.transport_type,
        server.url.as_deref(),
    ) {
        Ok(Some(_)) => {
            return Ok(failed(
                "connection_test_unavailable",
                "Native connector connectivity must be verified through its authenticated connection flow; local tool metadata is not a live connection test.",
                started,
            ));
        }
        Err(_) => {
            return Ok(failed(
                "invalid_configuration",
                "The connector configuration is invalid.",
                started,
            ));
        }
        Ok(None) => (),
    }
    let db_url = match server
        .connection_config
        .get("db_url")
        .and_then(|value| value.as_str())
    {
        Some(encrypted) => match decrypt_key(&state.settings.auth.app_key, encrypted) {
            Ok(value) => Some(value),
            Err(_) => {
                return Ok(failed(
                    "invalid_configuration",
                    "The saved credential cannot be read.",
                    started,
                ));
            }
        },
        None => None,
    };
    // A new client proves connectivity now, rather than trusting an older cached handshake.
    let client = McpServerClient::new(
        server.id,
        server.name,
        server.transport_type,
        server.url,
        server.connection_config,
        db_url,
    );
    let operation = async {
        let token = if client.connection_requirement().needs_user_token() {
            match resolve_mcp_oauth_token(state, actor, id, client.http_url_for_transport().ok())
                .await
            {
                Ok(Some(token)) => Some(token),
                _ => {
                    return failed(
                        "authentication_required",
                        "Connect your account before testing this server.",
                        started,
                    );
                }
            }
        } else {
            None
        };
        match client.list_tools_with_auth(token, Some(actor)).await {
            Ok(tools) => McpServerTestResult {
                success: true,
                message: Some(
                    "The server completed initialization and returned its tool list.".into(),
                ),
                latency_ms: Some(started.elapsed().as_millis().min(i32::MAX as u128) as i32),
                available_tools: Some(tools.len().min(i32::MAX as usize) as i32),
                error: None,
            },
            Err(_) => failed(
                "connection_failed",
                "The server could not be initialized or its tools could not be listed.",
                started,
            ),
        }
    };
    Ok(tokio::time::timeout(Duration::from_secs(10), operation)
        .await
        .unwrap_or_else(|_| {
            failed(
                "connection_timeout",
                "The connection test exceeded its ten-second deadline.",
                started,
            )
        }))
}
