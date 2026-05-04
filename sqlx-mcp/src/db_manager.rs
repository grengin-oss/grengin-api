use crate::read_only::is_read_only_sql;
use anyhow::{Context, Result, anyhow};
use serde_json::{Map, Value, json};
use sqlx::{
    Arguments, Column, Row, TypeInfo, ValueRef,
    postgres::{PgArguments, PgPool, PgPoolOptions, PgRow},
    query_with,
    types::Json,
};
use std::time::Duration;

pub struct DatabaseManager {
    pool: PgPool,
    read_only: bool,
}

impl DatabaseManager {
    pub fn new(
        url: &str,
        max_connections: u32,
        timeout_seconds: u64,
        read_only: bool,
    ) -> Result<Self> {
        if !is_postgres_url(url) {
            return Err(anyhow!(
                "Unsupported database URL format for sqlx-mcp: expected postgres:// or postgresql://"
            ));
        }

        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_secs(timeout_seconds))
            .connect_lazy(url)
            .context("failed to configure postgres pool")?;

        Ok(Self { pool, read_only })
    }

    pub async fn execute_query(&self, sql: &str, params: Vec<Value>) -> Result<Value> {
        if !is_read_only_sql(sql) {
            return Err(anyhow!("Read-only query validation failed"));
        }
        if looks_like_non_postgres_table_listing_query(sql) {
            return Err(anyhow!(
                "PostgreSQL dialect expected. Do not use sqlite_master or SHOW TABLES. \
Use information_schema.tables, for example: \
SELECT table_schema, table_name \
FROM information_schema.tables \
WHERE table_schema NOT IN ('pg_catalog', 'information_schema') \
ORDER BY table_schema, table_name"
            ));
        }

        if can_wrap_as_subquery(sql) {
            match self.execute_wrapped_query(sql, &params).await {
                Ok(value) => return Ok(value),
                Err(err) => {
                    tracing::debug!("wrapped query path failed, falling back to raw query: {err}");
                }
            }
        }

        self.execute_raw_query(sql, &params).await
    }

    pub async fn get_pool_state(&self) -> Value {
        json!({
            "database_type": "PostgreSQL",
            "read_only": self.read_only,
            "size": self.pool.size(),
            "num_idle": self.pool.num_idle(),
            "is_closed": self.pool.is_closed(),
        })
    }

    pub async fn test_connection(&self) -> Result<()> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .context("database connection test failed")?;
        Ok(())
    }

    async fn execute_wrapped_query(&self, sql: &str, params: &[Value]) -> Result<Value> {
        let normalized_sql = trim_single_trailing_semicolon(sql);
        let wrapped_sql = format!(
            "SELECT COALESCE(json_agg(to_jsonb(_sqlx_mcp_row)), '[]'::json) AS rows FROM ({normalized_sql}) AS _sqlx_mcp_row"
        );
        let arguments = build_arguments(params)?;

        let row = query_with(&wrapped_sql, arguments)
            .fetch_one(&self.pool)
            .await
            .context("query execution failed")?;

        let rows: Value = row
            .try_get("rows")
            .context("failed to decode query rows as JSON")?;
        Ok(rows)
    }

    async fn execute_raw_query(&self, sql: &str, params: &[Value]) -> Result<Value> {
        let arguments = build_arguments(params)?;
        let rows = query_with(sql, arguments)
            .fetch_all(&self.pool)
            .await
            .context("query execution failed")?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(row_to_json(&row));
        }
        Ok(Value::Array(out))
    }
}

fn row_to_json(row: &PgRow) -> Value {
    let mut obj = Map::new();
    for (idx, column) in row.columns().iter().enumerate() {
        let key = column.name().to_string();
        let value = decode_value(row, idx);
        obj.insert(key, value);
    }
    Value::Object(obj)
}

fn decode_value(row: &PgRow, idx: usize) -> Value {
    match row.try_get_raw(idx) {
        Ok(raw) if raw.is_null() => return Value::Null,
        Ok(_) => {}
        Err(_) => return Value::Null,
    }

    if let Ok(v) = row.try_get::<String, _>(idx) {
        return Value::String(v);
    }
    if let Ok(v) = row.try_get::<bool, _>(idx) {
        return Value::Bool(v);
    }
    if let Ok(v) = row.try_get::<i64, _>(idx) {
        return json!(v);
    }
    if let Ok(v) = row.try_get::<i32, _>(idx) {
        return json!(v);
    }
    if let Ok(v) = row.try_get::<i16, _>(idx) {
        return json!(v);
    }
    if let Ok(v) = row.try_get::<f64, _>(idx) {
        return json!(v);
    }
    if let Ok(v) = row.try_get::<f32, _>(idx) {
        return json!(v);
    }
    if let Ok(v) = row.try_get::<Value, _>(idx) {
        return v;
    }

    let fallback = row
        .try_get_raw(idx)
        .ok()
        .map(|raw| raw.type_info().name().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    Value::String(format!("<unserializable:{fallback}>"))
}

fn build_arguments(params: &[Value]) -> Result<PgArguments> {
    let mut arguments = PgArguments::default();
    for param in params {
        add_argument(&mut arguments, param)?;
    }
    Ok(arguments)
}

fn add_argument(arguments: &mut PgArguments, value: &Value) -> Result<()> {
    match value {
        Value::Null => arguments
            .add(Option::<String>::None)
            .map_err(|e| anyhow!("failed to bind NULL value: {e}")),
        Value::Bool(v) => arguments
            .add(*v)
            .map_err(|e| anyhow!("failed to bind bool value: {e}")),
        Value::Number(n) => {
            if let Some(v) = n.as_i64() {
                arguments
                    .add(v)
                    .map_err(|e| anyhow!("failed to bind i64 value: {e}"))
            } else if let Some(v) = n.as_u64() {
                let converted = i64::try_from(v)
                    .map_err(|_| anyhow!("u64 value is too large for PostgreSQL BIGINT: {v}"))?;
                arguments
                    .add(converted)
                    .map_err(|e| anyhow!("failed to bind converted u64 value: {e}"))
            } else if let Some(v) = n.as_f64() {
                arguments
                    .add(v)
                    .map_err(|e| anyhow!("failed to bind f64 value: {e}"))
            } else {
                Err(anyhow!("unsupported numeric parameter: {n}"))
            }
        }
        Value::String(v) => arguments
            .add(v.clone())
            .map_err(|e| anyhow!("failed to bind string value: {e}")),
        Value::Array(_) | Value::Object(_) => arguments
            .add(Json(value.clone()))
            .map_err(|e| anyhow!("failed to bind json value: {e}")),
    }
}

fn is_postgres_url(url: &str) -> bool {
    url.starts_with("postgres://") || url.starts_with("postgresql://")
}

fn can_wrap_as_subquery(sql: &str) -> bool {
    let first = sql
        .trim_start()
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(first.as_str(), "SELECT" | "WITH")
}

fn trim_single_trailing_semicolon(sql: &str) -> &str {
    let trimmed = sql.trim_end();
    if let Some(stripped) = trimmed.strip_suffix(';') {
        stripped.trim_end()
    } else {
        trimmed
    }
}

fn looks_like_non_postgres_table_listing_query(sql: &str) -> bool {
    let normalized = sql.to_ascii_lowercase();
    normalized.contains("sqlite_master")
        || normalized.contains("pragma table_info")
        || normalized.contains("show tables")
}

#[cfg(test)]
mod tests {
    use super::DatabaseManager;
    use serde_json::json;

    #[tokio::test]
    #[ignore = "requires DB_URL to point at a reachable PostgreSQL instance"]
    async fn smoke_executes_read_only_query_when_db_url_present() {
        let db_url = std::env::var("DB_URL").expect("DB_URL env var must be set");
        let manager = DatabaseManager::new(&db_url, 1, 10, true).expect("manager init should pass");

        manager
            .test_connection()
            .await
            .expect("connection test should succeed");

        let rows = manager
            .execute_query("SELECT 1 AS ok", vec![])
            .await
            .expect("read-only query should succeed");
        assert_eq!(rows, json!([{ "ok": 1 }]));

        let write_attempt = manager
            .execute_modification("CREATE TEMP TABLE _sqlx_mcp_smoke(id INT)", vec![])
            .await;
        assert!(write_attempt.is_err(), "read-only mode must block writes");
    }
}
