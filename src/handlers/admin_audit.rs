use std::env;

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
    Json,
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
        AuditLogEntry, AuditLogExportFormat, AuditLogRedactResponse, AuditLogsExportQuery,
        AuditLogsQuery, AuditLogsResponse,
    },
    models::audit_logs,
    services::{
        audit_logs::{
            list_audit_logs, parse_end_date, parse_start_date, redact_user_logs, AuditLogFilters,
        },
        authorization::{AuthorizationService, PermissionScopeMode},
    },
    state::SharedState,
};

const EXPORT_BATCH_LIMIT: u64 = 500;

fn to_entry(model: audit_logs::Model) -> AuditLogEntry {
    AuditLogEntry {
        id: model.id,
        user_id: model.user_id,
        action: model.action,
        resource_type: model.resource_type,
        resource_id: model.resource_id,
        details: model.details,
        ip_address: model.ip_address,
        user_agent: model.user_agent,
        created_at: model.created_at,
    }
}

fn export_max_rows() -> u64 {
    env::var("AUDIT_LOG_EXPORT_MAX_ROWS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(50_000)
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        return format!("\"{}\"", value.replace('"', "\"\""));
    }
    value.to_string()
}

fn render_csv(entries: &[AuditLogEntry]) -> String {
    let mut lines = Vec::with_capacity(entries.len() + 1);
    lines.push(
        "id,user_id,action,resource_type,resource_id,ip_address,user_agent,created_at,details"
            .to_string(),
    );
    for entry in entries {
        let details = entry
            .details
            .as_ref()
            .map(|value| value.to_string())
            .unwrap_or_default();
        let row = [
            csv_escape(&entry.id.to_string()),
            csv_escape(&entry.user_id.map(|v| v.to_string()).unwrap_or_default()),
            csv_escape(&entry.action),
            csv_escape(&entry.resource_type.clone().unwrap_or_default()),
            csv_escape(&entry.resource_id.clone().unwrap_or_default()),
            csv_escape(&entry.ip_address.clone().unwrap_or_default()),
            csv_escape(&entry.user_agent.clone().unwrap_or_default()),
            csv_escape(&entry.created_at.to_rfc3339()),
            csv_escape(&details),
        ]
        .join(",");
        lines.push(row);
    }
    lines.join("\n")
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
