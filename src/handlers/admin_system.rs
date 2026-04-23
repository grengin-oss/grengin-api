use std::{fs, path::Path, time::Instant};

use axum::{extract::State, Json};
use chrono::Utc;
use reqwest::StatusCode;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use sysinfo::{Disks, System};

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
        permissions::PERMISSION_ANALYTICS_VIEW,
    },
    dto::system_metrics::{
        ContainerMetrics, DatabaseMetrics, DiskMetrics, MachineMetrics, SystemMetricsResponse,
    },
    services::authorization::{AuthorizationService, PermissionScopeMode},
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
            PERMISSION_ANALYTICS_VIEW,
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

fn collect_machine_metrics() -> MachineMetrics {
    let mut system = System::new_all();
    system.refresh_all();
    system.refresh_cpu_usage();

    let disks = Disks::new_with_refreshed_list()
        .iter()
        .map(|disk| DiskMetrics {
            mount_point: disk.mount_point().to_string_lossy().to_string(),
            total_space_bytes: disk.total_space(),
            available_space_bytes: disk.available_space(),
        })
        .collect::<Vec<_>>();

    let load = System::load_average();
    MachineMetrics {
        cpu_usage_percent: system.global_cpu_usage(),
        total_memory_bytes: system.total_memory(),
        used_memory_bytes: system.used_memory(),
        free_memory_bytes: system.free_memory(),
        total_swap_bytes: system.total_swap(),
        used_swap_bytes: system.used_swap(),
        uptime_seconds: System::uptime(),
        load_average_1m: load.one,
        load_average_5m: load.five,
        load_average_15m: load.fifteen,
        disks,
    }
}

fn collect_container_metrics() -> ContainerMetrics {
    let inside_container = is_inside_container();
    let cgroup_v2 = Path::new("/sys/fs/cgroup/cgroup.controllers").exists();
    let cgroup_version = if cgroup_v2 {
        Some("v2".to_string())
    } else if Path::new("/sys/fs/cgroup").exists() {
        Some("v1".to_string())
    } else {
        None
    };

    let (memory_usage_bytes, memory_limit_bytes, cpu_quota_cores, cpu_usage_seconds) =
        if cgroup_v2 {
            (
                read_u64("/sys/fs/cgroup/memory.current"),
                read_u64_maybe_max("/sys/fs/cgroup/memory.max"),
                read_cpu_quota_v2("/sys/fs/cgroup/cpu.max"),
                read_cpu_usage_seconds_v2("/sys/fs/cgroup/cpu.stat"),
            )
        } else {
            (
                read_u64("/sys/fs/cgroup/memory/memory.usage_in_bytes"),
                read_u64_maybe_max("/sys/fs/cgroup/memory/memory.limit_in_bytes"),
                read_cpu_quota_v1(
                    "/sys/fs/cgroup/cpu/cpu.cfs_quota_us",
                    "/sys/fs/cgroup/cpu/cpu.cfs_period_us",
                ),
                read_cpu_usage_seconds_v1("/sys/fs/cgroup/cpuacct/cpuacct.usage"),
            )
        };

    let memory_available_bytes = match (memory_limit_bytes, memory_usage_bytes) {
        (Some(limit), Some(usage)) if limit > usage => Some(limit - usage),
        _ => None,
    };

    ContainerMetrics {
        inside_container,
        cgroup_version,
        memory_limit_bytes,
        memory_usage_bytes,
        memory_available_bytes,
        cpu_quota_cores,
        cpu_usage_seconds,
    }
}

async fn collect_database_metrics(app_state: &SharedState) -> Result<DatabaseMetrics, AuthError> {
    let latency_started = Instant::now();
    app_state
        .database
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT 1".to_string(),
        ))
        .await
        .map_err(|e| {
            eprintln!("system metrics db ping error: {e}");
            AuthError::DbTimeout
        })?;
    let roundtrip_latency_ms = Some(latency_started.elapsed().as_secs_f64() * 1000.0);

    let conn_row = query_row(
        app_state,
        r#"
            SELECT
                COUNT(*)::bigint AS total_connections,
                COUNT(*) FILTER (WHERE state = 'active')::bigint AS active_connections,
                COUNT(*) FILTER (WHERE state = 'idle')::bigint AS idle_connections
            FROM pg_stat_activity
            WHERE datname = current_database()
        "#,
    )
    .await;
    let size_row = query_row(
        app_state,
        "SELECT pg_database_size(current_database())::bigint AS database_size_bytes",
    )
    .await;
    let stat_row = query_row(
        app_state,
        r#"
            SELECT
                numbackends::bigint AS numbackends,
                xact_commit::bigint AS xact_commit,
                xact_rollback::bigint AS xact_rollback,
                blks_read::bigint AS blks_read,
                blks_hit::bigint AS blks_hit,
                tup_returned::bigint AS tup_returned,
                tup_fetched::bigint AS tup_fetched,
                tup_inserted::bigint AS tup_inserted,
                tup_updated::bigint AS tup_updated,
                tup_deleted::bigint AS tup_deleted
            FROM pg_stat_database
            WHERE datname = current_database()
        "#,
    )
    .await;

    Ok(DatabaseMetrics {
        roundtrip_latency_ms,
        total_connections: get_i64_col(conn_row.as_ref(), "total_connections"),
        active_connections: get_i64_col(conn_row.as_ref(), "active_connections"),
        idle_connections: get_i64_col(conn_row.as_ref(), "idle_connections"),
        database_size_bytes: get_i64_col(size_row.as_ref(), "database_size_bytes"),
        numbackends: get_i64_col(stat_row.as_ref(), "numbackends"),
        xact_commit: get_i64_col(stat_row.as_ref(), "xact_commit"),
        xact_rollback: get_i64_col(stat_row.as_ref(), "xact_rollback"),
        blks_read: get_i64_col(stat_row.as_ref(), "blks_read"),
        blks_hit: get_i64_col(stat_row.as_ref(), "blks_hit"),
        tup_returned: get_i64_col(stat_row.as_ref(), "tup_returned"),
        tup_fetched: get_i64_col(stat_row.as_ref(), "tup_fetched"),
        tup_inserted: get_i64_col(stat_row.as_ref(), "tup_inserted"),
        tup_updated: get_i64_col(stat_row.as_ref(), "tup_updated"),
        tup_deleted: get_i64_col(stat_row.as_ref(), "tup_deleted"),
    })
}

async fn query_row(app_state: &SharedState, sql: &str) -> Option<sea_orm::QueryResult> {
    app_state
        .database
        .query_one(Statement::from_string(
            DatabaseBackend::Postgres,
            sql.to_string(),
        ))
        .await
        .ok()
        .flatten()
}

fn get_i64_col(row: Option<&sea_orm::QueryResult>, col: &str) -> Option<i64> {
    row.and_then(|record| record.try_get::<i64>("", col).ok())
}

fn is_inside_container() -> bool {
    if Path::new("/.dockerenv").exists() {
        return true;
    }
    fs::read_to_string("/proc/1/cgroup")
        .ok()
        .map(|v| {
            let lowered = v.to_lowercase();
            lowered.contains("docker")
                || lowered.contains("kubepods")
                || lowered.contains("containerd")
                || lowered.contains("podman")
        })
        .unwrap_or(false)
}

fn read_u64(path: &str) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse::<u64>().ok()
}

fn read_u64_maybe_max(path: &str) -> Option<u64> {
    let raw = fs::read_to_string(path).ok()?;
    let value = raw.trim();
    if value.eq_ignore_ascii_case("max") {
        return None;
    }
    value.parse::<u64>().ok()
}

fn read_cpu_quota_v2(path: &str) -> Option<f64> {
    let raw = fs::read_to_string(path).ok()?;
    let mut parts = raw.split_whitespace();
    let quota = parts.next()?;
    let period = parts.next()?.parse::<f64>().ok()?;
    if quota.eq_ignore_ascii_case("max") || period <= 0.0 {
        return None;
    }
    let quota = quota.parse::<f64>().ok()?;
    Some(quota / period)
}

fn read_cpu_usage_seconds_v2(path: &str) -> Option<f64> {
    let raw = fs::read_to_string(path).ok()?;
    for line in raw.lines() {
        let mut parts = line.split_whitespace();
        if parts.next()? == "usage_usec" {
            let usage_usec = parts.next()?.parse::<f64>().ok()?;
            return Some(usage_usec / 1_000_000.0);
        }
    }
    None
}

fn read_cpu_quota_v1(quota_path: &str, period_path: &str) -> Option<f64> {
    let quota = read_u64(quota_path)? as f64;
    let period = read_u64(period_path)? as f64;
    if period <= 0.0 {
        return None;
    }
    Some(quota / period)
}

fn read_cpu_usage_seconds_v1(path: &str) -> Option<f64> {
    let usage_ns = read_u64(path)? as f64;
    Some(usage_ns / 1_000_000_000.0)
}
