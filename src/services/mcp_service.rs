use crate::{
    auth::error::AuthError,
    error::AppError,
    models::{mcp_connections, mcp_servers},
    state::SharedState,
};
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use chrono::Utc;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;
use uuid::Uuid;

pub async fn resolve_server_connected(
    state: &SharedState,
    server: &mcp_servers::Model,
) -> Result<bool, AuthError> {
    match server.transport_type {
        mcp_servers::McpTransportType::Stdio => Ok(server.status.as_deref() == Some("connected")),
        mcp_servers::McpTransportType::Http
        | mcp_servers::McpTransportType::Sse
        | mcp_servers::McpTransportType::Websocket => {
            let now = Utc::now();
            let connections = mcp_connections::Entity::find()
                .filter(mcp_connections::Column::ServerId.eq(server.id))
                .filter(mcp_connections::Column::Connected.eq(true))
                .all(&state.database)
                .await
                .map_err(|e| {
                    eprintln!("mcp connections lookup error: {e}");
                    AuthError::DbTimeout
                })?;
            Ok(connections.into_iter().any(|row| {
                let not_expired = row.expires_at.map(|exp| exp > now).unwrap_or(true);
                row.connected && not_expired
            }))
        }
    }
}

pub fn build_oauth_callback_response(
    redirect_uri: Option<&str>,
    server_id: Uuid,
    success: bool,
) -> Response {
    if let Some(redirect_uri) = redirect_uri {
        let separator = if redirect_uri.contains('?') { "&" } else { "?" };
        let status_value = if success { "success" } else { "error" };
        let url =
            format!("{redirect_uri}{separator}mcp_server_id={server_id}&status={status_value}");
        return Redirect::to(&url).into_response();
    }

    let body = json!({
        "success": success,
        "server_id": server_id,
        "status": if success { "success" } else { "error" }
    });
    (StatusCode::OK, Json(body)).into_response()
}

pub fn map_mcp_access_error(err: AppError) -> AuthError {
    match err {
        AppError::DbTimeout => AuthError::DbTimeout,
        AppError::DbUnavailable => AuthError::DbUnavailable,
        AppError::ResourceNotFound => AuthError::ResourceNotFound,
        _ => AuthError::ServiceTemporarilyUnavailable,
    }
}
