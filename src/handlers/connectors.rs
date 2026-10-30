// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::claims::Claims,
    dto::connectors::{CompleteConnectorRequest, ConnectConnectorRequest, ConnectorConnection},
    error::AppError,
    services::connector_broker::ConnectorError,
    services::connectors_helpers,
    state::SharedState,
};
use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use uuid::Uuid;

#[utoipa::path(get,path="/connectors",tag="connectors",description="List this user's stored broker connections. GET an individual connection to verify its current broker status. Credentials are never returned.",
    responses((status=200,body=Vec<ConnectorConnection>),(status=401,description="Sign-in required"),(status=503,description="Database unavailable")))]
pub async fn list(
    claims: Claims,
    State(state): State<SharedState>,
) -> Result<impl IntoResponse, AppError> {
    Ok((
        [("Cache-Control", "no-store")],
        Json(connectors_helpers::list_connections(&state, claims.user_id).await?),
    ))
}

#[utoipa::path(post,path="/connectors/{connector_key}/connect",tag="connectors",description="Start or resume explicit vendor OAuth consent through Grengin's broker. Pin an exact catalog version. Optional server_id must identify an accessible Streamable HTTP MCP server at the same catalog URL. No OAuth client secret is accepted.",
    params(("connector_key"=String,Path,description="Catalog connector key")),request_body=ConnectConnectorRequest,
    responses((status=200,body=ConnectorConnection),(status=400,description="Unsupported connector or invalid request"),(status=401,description="Sign-in required"),(status=403,description="Installation not authorized"),(status=404,description="Invalid connector or inaccessible MCP server"),(status=409,description="Disconnect the existing connection before changing its catalog version or server"),(status=422,description="Invalid JSON fields"),(status=429,description="Rate limit; obey Retry-After"),(status=503,description="Broker unavailable, disabled or not configured")))]
pub async fn connect(
    claims: Claims,
    State(state): State<SharedState>,
    Path(key): Path<String>,
    Json(request): Json<ConnectConnectorRequest>,
) -> Result<impl IntoResponse, ConnectorError> {
    Ok((
        [("Cache-Control", "no-store")],
        Json(connectors_helpers::start_connection(&state, claims.user_id, key, request).await?),
    ))
}

#[utoipa::path(get,path="/connectors/{connection_id}",tag="connectors",description="Verify a connection's current status with the broker, restricted to its local owner. Broker outages never fall back to claiming a cached connection is verified.",
    params(("connection_id"=Uuid,Path,description="User connection UUID")),responses((status=200,body=ConnectorConnection),(status=401,description="Sign-in required"),(status=403,description="Installation not authorized"),(status=404,description="Connection not found or not owned by caller"),(status=429,description="Rate limit; obey Retry-After"),(status=503,description="Broker unavailable")))]
pub async fn status(
    claims: Claims,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ConnectorError> {
    Ok((
        [("Cache-Control", "no-store")],
        Json(connectors_helpers::connection_status(&state, claims.user_id, id).await?),
    ))
}

#[utoipa::path(post,path="/connectors/{connection_id}/disconnect",tag="connectors",description="Disable local use immediately, then revoke the broker grant and remove local credentials. An outage leaves revoked status with revocation_pending=true; retry this operation to complete cleanup. Does not change SSO or another user's connection.",
    params(("connection_id"=Uuid,Path,description="User connection UUID")),responses((status=200,body=ConnectorConnection),(status=401,description="Sign-in required"),(status=403,description="Installation proof invalid"),(status=404,description="Connection not found or not owned by caller"),(status=429,description="Rate limit; obey Retry-After"),(status=503,description="Broker unavailable or persistence failure; refresh status and retry")))]
pub async fn disconnect(
    claims: Claims,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ConnectorError> {
    Ok((
        [("Cache-Control", "no-store")],
        Json(connectors_helpers::disconnect_connection(&state, claims.user_id, id).await?),
    ))
}

#[utoipa::path(post,path="/connectors/{connection_id}/complete",tag="connectors",description="Activate a connection only after its signed-in owner submits the one-time code delivered in the vendor callback browser. Polling never returns this code.",
params(("connection_id"=Uuid,Path,description="User connection UUID")),request_body=CompleteConnectorRequest,
responses((status=200,body=ConnectorConnection),(status=400,description="Invalid confirmation code format"),(status=401,description="Sign-in required"),(status=403,description="Installation not authorized"),(status=404,description="Connection not owned by caller"),(status=409,description="Code expired, incorrect or already used"),(status=429,description="Rate limit; obey Retry-After"),(status=503,description="Broker unavailable")))]
pub async fn complete(
    claims: Claims,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(request): Json<CompleteConnectorRequest>,
) -> Result<impl IntoResponse, ConnectorError> {
    Ok(Json(
        connectors_helpers::complete_connection(&state, claims.user_id, id, request).await?,
    ))
}
