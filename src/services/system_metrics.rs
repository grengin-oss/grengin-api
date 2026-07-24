use std::{fs, path::Path, time::Instant};

use sea_orm::{DatabaseBackend, FromQueryResult, Statement};

use crate::{
    auth::error::AuthError,
    dto::system_metrics::{ContainerMetrics, DatabaseMetrics, DiskMetrics, MachineMetrics},
    state::SharedState,
};
use sysinfo::{Disks, System};

#[derive(Debug, FromQueryResult)]
struct ConnectionStatsRow {
    total_connections: i64,
    active_connections: i64,
    idle_connections: i64,
}

#[derive(Debug, FromQueryResult)]
struct DatabaseSizeRow {
    database_size_bytes: i64,
}

#[derive(Debug, FromQueryResult)]
struct DatabaseStatRow {
    numbackends: i64,
    xact_commit: i64,
    xact_rollback: i64,
    blks_read: i64,
    blks_hit: i64,
    tup_returned: i64,
    tup_fetched: i64,
    tup_inserted: i64,
    tup_updated: i64,
    tup_deleted: i64,
}

pub fn collect_machine_metrics() -> MachineMetrics {
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

pub fn collect_container_metrics() -> ContainerMetrics {
    let inside_container = is_inside_container();
    let cgroup_v2 = Path::new("/sys/fs/cgroup/cgroup.controllers").exists();
    let cgroup_version = if cgroup_v2 {
        Some("v2".to_string())
    } else if Path::new("/sys/fs/cgroup").exists() {
        Some("v1".to_string())
    } else {
        None
    };

    let (memory_usage_bytes, memory_limit_bytes, cpu_quota_cores, cpu_usage_seconds) = if cgroup_v2
    {
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

pub async fn collect_database_metrics(app_state: &SharedState) -> Result<DatabaseMetrics, AuthError> {
    let db = &app_state.database;

    let latency_started = Instant::now();
    db.ping().await.map_err(|e| {
        eprintln!("system metrics db ping error: {e}");
        AuthError::DbTimeout
    })?;
    let roundtrip_latency_ms = Some(latency_started.elapsed().as_secs_f64() * 1000.0);

    let conn_stats = ConnectionStatsRow::find_by_statement(Statement::from_string(
        DatabaseBackend::Postgres,
        r#"SELECT
            COUNT(*)::bigint AS total_connections,
            COUNT(*) FILTER (WHERE state = 'active')::bigint AS active_connections,
            COUNT(*) FILTER (WHERE state = 'idle')::bigint AS idle_connections
        FROM pg_stat_activity WHERE datname = current_database()"#,
    ))
    .one(db)
    .await
    .ok()
    .flatten();

    let db_size = DatabaseSizeRow::find_by_statement(Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT pg_database_size(current_database())::bigint AS database_size_bytes",
    ))
    .one(db)
    .await
    .ok()
    .flatten();

    let db_stat = DatabaseStatRow::find_by_statement(Statement::from_string(
        DatabaseBackend::Postgres,
        r#"SELECT
            numbackends::bigint, xact_commit::bigint, xact_rollback::bigint,
            blks_read::bigint, blks_hit::bigint, tup_returned::bigint,
            tup_fetched::bigint, tup_inserted::bigint, tup_updated::bigint, tup_deleted::bigint
        FROM pg_stat_database WHERE datname = current_database()"#,
    ))
    .one(db)
    .await
    .ok()
    .flatten();

    Ok(DatabaseMetrics {
        roundtrip_latency_ms,
        total_connections: conn_stats.as_ref().map(|r| r.total_connections),
        active_connections: conn_stats.as_ref().map(|r| r.active_connections),
        idle_connections: conn_stats.as_ref().map(|r| r.idle_connections),
        database_size_bytes: db_size.map(|r| r.database_size_bytes),
        numbackends: db_stat.as_ref().map(|r| r.numbackends),
        xact_commit: db_stat.as_ref().map(|r| r.xact_commit),
        xact_rollback: db_stat.as_ref().map(|r| r.xact_rollback),
        blks_read: db_stat.as_ref().map(|r| r.blks_read),
        blks_hit: db_stat.as_ref().map(|r| r.blks_hit),
        tup_returned: db_stat.as_ref().map(|r| r.tup_returned),
        tup_fetched: db_stat.as_ref().map(|r| r.tup_fetched),
        tup_inserted: db_stat.as_ref().map(|r| r.tup_inserted),
        tup_updated: db_stat.as_ref().map(|r| r.tup_updated),
        tup_deleted: db_stat.as_ref().map(|r| r.tup_deleted),
    })
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
