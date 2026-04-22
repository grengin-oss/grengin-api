use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SystemMetricsResponse {
    pub generated_at: DateTime<Utc>,
    pub machine: MachineMetrics,
    pub container: ContainerMetrics,
    pub database: DatabaseMetrics,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MachineMetrics {
    pub cpu_usage_percent: f32,
    pub total_memory_bytes: u64,
    pub used_memory_bytes: u64,
    pub free_memory_bytes: u64,
    pub total_swap_bytes: u64,
    pub used_swap_bytes: u64,
    pub uptime_seconds: u64,
    pub load_average_1m: f64,
    pub load_average_5m: f64,
    pub load_average_15m: f64,
    pub disks: Vec<DiskMetrics>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiskMetrics {
    pub mount_point: String,
    pub total_space_bytes: u64,
    pub available_space_bytes: u64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContainerMetrics {
    pub inside_container: bool,
    pub cgroup_version: Option<String>,
    pub memory_limit_bytes: Option<u64>,
    pub memory_usage_bytes: Option<u64>,
    pub memory_available_bytes: Option<u64>,
    pub cpu_quota_cores: Option<f64>,
    pub cpu_usage_seconds: Option<f64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseMetrics {
    pub roundtrip_latency_ms: Option<f64>,
    pub total_connections: Option<i64>,
    pub active_connections: Option<i64>,
    pub idle_connections: Option<i64>,
    pub database_size_bytes: Option<i64>,
    pub numbackends: Option<i64>,
    pub xact_commit: Option<i64>,
    pub xact_rollback: Option<i64>,
    pub blks_read: Option<i64>,
    pub blks_hit: Option<i64>,
    pub tup_returned: Option<i64>,
    pub tup_fetched: Option<i64>,
    pub tup_inserted: Option<i64>,
    pub tup_updated: Option<i64>,
    pub tup_deleted: Option<i64>,
}
