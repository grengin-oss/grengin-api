// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use axum::{Json, extract::State};
use chrono::Utc;
use reqwest::StatusCode;

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
        permissions::PERMISSION_SYSTEM_MAINTAIN,
    },
    dto::system_metrics::SystemMetricsResponse,
    services::{
        authorization::{AuthorizationService, PermissionScopeMode},
        system_metrics::{collect_container_metrics, collect_database_metrics, collect_machine_metrics},
    },
    state::SharedState,
};

#[utoipa::path(
    get,
    path = "/admin/system-metrics",
    tag = "admin",
    responses(
        (status = 200, body = SystemMetricsResponse),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn get_system_metrics(
    claims: Claims,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<SystemMetricsResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_SYSTEM_MAINTAIN,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let machine = collect_machine_metrics();
    let container = collect_container_metrics();
    let database = collect_database_metrics(&app_state).await?;

    Ok((
        StatusCode::OK,
        Json(SystemMetricsResponse {
            generated_at: Utc::now(),
            machine,
            container,
            database,
        }),
    ))
}
