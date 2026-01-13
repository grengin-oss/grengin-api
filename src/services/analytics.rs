use chrono::{Duration, NaiveDate, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use sea_orm::{ DatabaseConnection, DbErr, FromQueryResult};
use serde::{Deserialize, Serialize};
use crate::{dto::analytics::{DepartmentAnalytics, DepartmentAnalyticsResponse}};

#[derive(Debug, FromQueryResult, Serialize, Deserialize)]
struct DepartmentMetrics {
    department: String,
    total_users: Option<i64>,
    total_requests: Option<Decimal>,
    total_tokens: Option<Decimal>,
    total_cost: Option<Decimal>,
    average_latency: Option<Decimal>,
    success_count: Option<Decimal>,
    error_count: Option<Decimal>,
}

pub async fn get_department_analytics(
    db: &DatabaseConnection,
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
) -> Result<DepartmentAnalyticsResponse, DbErr> {
    let end = end_date.unwrap_or_else(|| Utc::now().date_naive());
    let start = start_date.unwrap_or_else(|| end - Duration::days(30));

    let query = r#"
        SELECT 
            COALESCE(usd.department, 'Unknown') as department,
            COUNT(DISTINCT usd."userId") as total_users,
            COALESCE(SUM(usd."totalRequests"), 0) as total_requests,
            COALESCE(SUM(usd."totalTokens"), 0) as total_tokens,
            COALESCE(SUM(usd."totalCost"), 0) as total_cost,
            COALESCE(AVG(usd."averageLatency"), 0) as average_latency,
            COALESCE(SUM(usd."successCount"), 0) as success_count,
            COALESCE(SUM(usd."errorCount"), 0) as error_count
        FROM usage_summary_daily usd
        WHERE usd.date >= $1 AND usd.date <= $2
        GROUP BY usd.department
        ORDER BY total_requests DESC
    "#;

    let departments_data =
        DepartmentMetrics::find_by_statement(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            query,
            vec![start.into(), end.into()],
        ))
        .all(db)
        .await?;

    let total = departments_data.len() as i64;

    Ok(DepartmentAnalyticsResponse {
        departments: departments_data
            .into_iter()
            .map(|d| DepartmentAnalytics {
                department: d.department,
                total_users: d.total_users.unwrap_or(0),
                total_requests: d.total_requests.unwrap_or(Decimal::ZERO).to_i64().unwrap_or(0),
                total_tokens: d.total_tokens.unwrap_or(Decimal::ZERO).to_i64().unwrap_or(0),
                total_cost: d
                    .total_cost
                    .unwrap_or(Decimal::ZERO)
                    .to_f64()
                    .unwrap_or(0.0),
                average_latency: d
                    .average_latency
                    .unwrap_or(Decimal::ZERO)
                    .to_f64()
                    .unwrap_or(0.0),
                success_count: d.success_count.unwrap_or(Decimal::ZERO).to_i64().unwrap_or(0),
                error_count: d.error_count.unwrap_or(Decimal::ZERO).to_i64().unwrap_or(0),
            })
            .collect(),
        total,
    })
}
