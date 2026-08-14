// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use reqwest::StatusCode;
use uuid::Uuid;

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
        permissions::PERMISSION_AUDIT_LOGS_VIEW,
    },
    dto::audit_logs::{
        AuditLogAction, AuditLogEntry, AuditLogExportFormat, AuditLogRedactResponse,
        AuditLogsExportQuery, AuditLogsQuery, AuditLogsResponse,
    },
    models::audit_logs,
    services::{
        audit_logs::{
            AuditLogFilters, EXPORT_BATCH_LIMIT, export_max_rows, list_audit_logs, parse_end_date,
            parse_start_date, redact_user_logs, render_csv, to_entry,
        },
        authorization::{AuthorizationService, PermissionScopeMode},
    },
    state::SharedState,
};

#[utoipa::path(
    get,
    path = "/audit/actions",
    tag = "admin",
    responses(
        (status = 200, body = Vec<AuditLogAction>),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
    )
)]
pub async fn get_audit_actions(
    claims: Claims,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<Vec<AuditLogAction>>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AUDIT_LOGS_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    Ok((StatusCode::OK, Json(AuditLogAction::all())))
}

#[utoipa::path(
    get,
    path = "/admin/audit-logs",
    tag = "admin",
    params(AuditLogsQuery),
    responses(
        (status = 200, body = AuditLogsResponse),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn get_audit_logs(
    claims: Claims,
    Query(query): Query<AuditLogsQuery>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<AuditLogsResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AUDIT_LOGS_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let filters = AuditLogFilters {
        user_id: query.user_id,
        action: query.action.map(|action| action.as_str().to_string()),
        start_date: parse_start_date(query.start_date.as_deref()),
        end_date: parse_end_date(query.end_date.as_deref()),
        page: query.page.unwrap_or(1),
        limit: query.limit.unwrap_or(50),
    };
    let page = list_audit_logs(&app_state.database, filters)
        .await
        .map_err(|err| {
            eprintln!("audit logs query error: {err}");
            AuthError::DbTimeout
        })?;
    let payload = AuditLogsResponse {
        items: page.items.into_iter().map(to_entry).collect(),
        total: page.total,
        page: page.page,
        limit: page.limit,
    };
    Ok((StatusCode::OK, Json(payload)))
}

#[utoipa::path(
    get,
    path = "/admin/audit-logs/export",
    tag = "admin",
    params(AuditLogsExportQuery),
    responses(
        (status = 200, description = "Audit log export in CSV or JSON"),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn export_audit_logs(
    claims: Claims,
    Query(query): Query<AuditLogsExportQuery>,
    State(app_state): State<SharedState>,
) -> Result<Response, AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AUDIT_LOGS_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let max_rows = export_max_rows();
    let mut page_number = 1u64;
    let mut rows: Vec<audit_logs::Model> = Vec::new();
    loop {
        let filters = AuditLogFilters {
            user_id: query.user_id,
            action: query.action.map(|action| action.as_str().to_string()),
            start_date: parse_start_date(query.start_date.as_deref()),
            end_date: parse_end_date(query.end_date.as_deref()),
            page: page_number,
            limit: EXPORT_BATCH_LIMIT,
        };
        let page = list_audit_logs(&app_state.database, filters)
            .await
            .map_err(|err| {
                eprintln!("audit logs export query error: {err}");
                AuthError::DbTimeout
            })?;
        if page.items.is_empty() {
            break;
        }
        rows.extend(page.items);
        if rows.len() as u64 >= page.total || rows.len() as u64 >= max_rows {
            break;
        }
        page_number += 1;
    }

    let entries: Vec<AuditLogEntry> = rows.into_iter().map(to_entry).collect();
    match query.format.unwrap_or(AuditLogExportFormat::Json) {
        AuditLogExportFormat::Json => Ok((StatusCode::OK, Json(entries)).into_response()),
        AuditLogExportFormat::Csv => {
            let body = render_csv(&entries);
            let mut response = (StatusCode::OK, body).into_response();
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/csv; charset=utf-8"),
            );
            response.headers_mut().insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_static("attachment; filename=\"audit-logs.csv\""),
            );
            Ok(response)
        }
    }
}

#[utoipa::path(
    post,
    path = "/admin/audit-logs/redact/{user_id}",
    tag = "admin",
    params(("user_id" = Uuid, Path, description = "User id to redact PII for")),
    responses(
        (status = 200, body = AuditLogRedactResponse),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn redact_audit_logs_for_user(
    claims: Claims,
    Path(user_id): Path<Uuid>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<AuditLogRedactResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AUDIT_LOGS_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let redacted_count = redact_user_logs(&app_state.database, user_id)
        .await
        .map_err(|err| {
            eprintln!("audit logs redact error: {err}");
            AuthError::DbTimeout
        })?;
    Ok((
        StatusCode::OK,
        Json(AuditLogRedactResponse {
            user_id,
            redacted_count,
        }),
    ))
}
