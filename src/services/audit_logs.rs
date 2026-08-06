// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{env, time::Duration};

use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, DbErr, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder,
};
use serde_json::Value;
use uuid::Uuid;

use crate::{dto::audit_logs::AuditLogEntry, models::audit_logs};

const DEFAULT_RETENTION_DAYS: i64 = 365;
const DEFAULT_PAGE: u64 = 1;
const DEFAULT_LIMIT: u64 = 50;
const MAX_LIMIT: u64 = 500;

#[derive(Debug, Clone)]
pub struct AuditLogCreate {
    pub user_id: Option<Uuid>,
    pub action: String,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub details: Option<Value>,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AuditLogFilters {
    pub user_id: Option<Uuid>,
    pub action: Option<String>,
    pub start_date: Option<DateTime<Utc>>,
    pub end_date: Option<DateTime<Utc>>,
    pub page: u64,
    pub limit: u64,
}

#[derive(Debug, Clone)]
pub struct AuditLogPage {
    pub items: Vec<audit_logs::Model>,
    pub total: u64,
    pub page: u64,
    pub limit: u64,
}

pub async fn record_audit_log(db: &DatabaseConnection, input: AuditLogCreate) -> Result<(), DbErr> {
    let row = audit_logs::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(input.user_id),
        action: Set(input.action),
        resource_type: Set(input.resource_type),
        resource_id: Set(input.resource_id),
        details: Set(input.details),
        ip_address: Set(input.ip_address),
        user_agent: Set(input.user_agent),
        created_at: Set(Utc::now()),
    };
    row.insert(db).await?;
    Ok(())
}

pub async fn list_audit_logs(
    db: &DatabaseConnection,
    filters: AuditLogFilters,
) -> Result<AuditLogPage, DbErr> {
    let page = if filters.page == 0 {
        DEFAULT_PAGE
    } else {
        filters.page
    };
    let limit = normalize_limit(filters.limit);

    let mut query = audit_logs::Entity::find();
    if let Some(user_id) = filters.user_id {
        query = query.filter(audit_logs::Column::UserId.eq(user_id));
    }
    if let Some(action) = filters.action.as_ref() {
        query = query.filter(audit_logs::Column::Action.eq(action));
    }
    if let Some(start_date) = filters.start_date {
        query = query.filter(audit_logs::Column::CreatedAt.gte(start_date));
    }
    if let Some(end_date) = filters.end_date {
        query = query.filter(audit_logs::Column::CreatedAt.lte(end_date));
    }

    let total = query.clone().count(db).await?;
    let paginator = query
        .order_by_desc(audit_logs::Column::CreatedAt)
        .paginate(db, limit);
    let items = paginator.fetch_page(page.saturating_sub(1)).await?;

    Ok(AuditLogPage {
        items,
        total,
        page,
        limit,
    })
}

pub async fn redact_user_logs(db: &DatabaseConnection, user_id: Uuid) -> Result<u64, DbErr> {
    let rows = audit_logs::Entity::find()
        .filter(audit_logs::Column::UserId.eq(user_id))
        .all(db)
        .await?;
    let mut updated = 0u64;
    for row in rows {
        let mut active: audit_logs::ActiveModel = row.into();
        active.ip_address = Set(None);
        active.user_agent = Set(None);
        active.details = Set(Some(serde_json::json!({
            "gdpr_redacted": true
        })));
        active.update(db).await?;
        updated += 1;
    }
    Ok(updated)
}

pub async fn prune_expired_logs(
    db: &DatabaseConnection,
    retention_days: i64,
) -> Result<u64, DbErr> {
    if retention_days <= 0 {
        return Ok(0);
    }
    let cutoff = Utc::now() - ChronoDuration::days(retention_days);
    let result = audit_logs::Entity::delete_many()
        .filter(audit_logs::Column::CreatedAt.lt(cutoff))
        .exec(db)
        .await?;
    Ok(result.rows_affected)
}

pub fn spawn_audit_log_retention_worker(db: DatabaseConnection) {
    let retention_days = audit_log_retention_days();
    if retention_days <= 0 {
        return;
    }
    let sweep_hours = env::var("AUDIT_LOG_RETENTION_SWEEP_HOURS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(24);
    tokio::spawn(async move {
        if let Err(err) = prune_expired_logs(&db, retention_days).await {
            eprintln!("audit log retention cleanup error: {err}");
        }
        let interval = Duration::from_secs(sweep_hours * 3600);
        loop {
            tokio::time::sleep(interval).await;
            if let Err(err) = prune_expired_logs(&db, retention_days).await {
                eprintln!("audit log retention cleanup error: {err}");
            }
        }
    });
}

pub fn parse_start_date(raw: Option<&str>) -> Option<DateTime<Utc>> {
    raw.and_then(|value| parse_datetime_or_date(value, true))
}

pub fn parse_end_date(raw: Option<&str>) -> Option<DateTime<Utc>> {
    raw.and_then(|value| parse_datetime_or_date(value, false))
}

fn parse_datetime_or_date(value: &str, start_of_day: bool) -> Option<DateTime<Utc>> {
    if let Ok(datetime) = DateTime::parse_from_rfc3339(value) {
        return Some(datetime.with_timezone(&Utc));
    }
    let parsed_date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    let naive = if start_of_day {
        parsed_date.and_hms_opt(0, 0, 0)?
    } else {
        parsed_date.and_hms_opt(23, 59, 59)?
    };
    Some(DateTime::from_naive_utc_and_offset(naive, Utc))
}

pub fn normalize_limit(limit: u64) -> u64 {
    if limit == 0 {
        return DEFAULT_LIMIT;
    }
    limit.min(MAX_LIMIT)
}

fn audit_log_retention_days() -> i64 {
    env::var("AUDIT_LOG_RETENTION_DAYS")
        .ok()
        .and_then(|raw| raw.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_RETENTION_DAYS)
}

pub const EXPORT_BATCH_LIMIT: u64 = 500;

pub fn to_entry(model: audit_logs::Model) -> AuditLogEntry {
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

pub fn export_max_rows() -> u64 {
    env::var("AUDIT_LOG_EXPORT_MAX_ROWS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(50_000)
}

pub fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        return format!("\"{}\"", value.replace('"', "\"\""));
    }
    value.to_string()
}

pub fn render_csv(entries: &[AuditLogEntry]) -> String {
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
