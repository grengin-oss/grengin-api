use chrono::{Duration, NaiveDate, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, FromQueryResult, QueryFilter,
    QuerySelect, QueryOrder, Order,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    dto::analytics::{
        DepartmentAnalytics, DepartmentAnalyticsResponse, ModelUsage, OverviewResponse,
        TimeSeriesDataPoint, TimeSeriesResponse, UserAnalytics, UserAnalyticsResponse,
    },
    models::{usage_logs, usage_summary_daily, users},
};

#[derive(Debug, FromQueryResult, Serialize, Deserialize)]
struct OverviewMetrics {
    total_users: Option<i64>,
    active_users: Option<i64>,
    total_requests: Option<Decimal>,
    total_tokens: Option<Decimal>,
    total_cost: Option<Decimal>,
}

#[derive(Debug, FromQueryResult, Serialize, Deserialize)]
struct ModelMetrics {
    model_provider: String,
    model_name: String,
    total_requests: Option<Decimal>,
    total_tokens: Option<Decimal>,
    total_cost: Option<Decimal>,
}

#[derive(Debug, FromQueryResult, Serialize, Deserialize)]
struct UserMetrics {
    user_id: Uuid,
    user_email: String,
    user_name: Option<String>,
    department: Option<String>,
    total_requests: Option<Decimal>,
    total_tokens: Option<Decimal>,
    total_cost: Option<Decimal>,
    average_latency: Option<Decimal>,
    success_count: Option<Decimal>,
    error_count: Option<Decimal>,
    last_activity: Option<chrono::DateTime<Utc>>,
}

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

#[derive(Debug, FromQueryResult, Serialize, Deserialize)]
struct TimeSeriesMetrics {
    period: String,
    total_requests: Option<Decimal>,
    total_tokens: Option<Decimal>,
    total_cost: Option<Decimal>,
    average_latency: Option<Decimal>,
    success_count: Option<Decimal>,
    error_count: Option<Decimal>,
}

pub async fn get_overview_analytics(
    db: &DatabaseConnection,
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
) -> Result<OverviewResponse, DbErr> {
    let end = end_date.unwrap_or_else(|| Utc::now().date_naive());
    let start = start_date.unwrap_or_else(|| end - Duration::days(30));

    let query = r#"
        SELECT 
            COUNT(DISTINCT "userId") as total_users,
            COUNT(DISTINCT "userId") as active_users,
            COALESCE(SUM("totalRequests"), 0) as total_requests,
            COALESCE(SUM("totalTokens"), 0) as total_tokens,
            COALESCE(SUM("totalCost"), 0) as total_cost
        FROM usage_summary_daily
        WHERE date >= $1 AND date <= $2
    "#;

    let current_metrics: Option<OverviewMetrics> = OverviewMetrics::find_by_statement(
        sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            query,
            vec![start.into(), end.into()],
        ),
    )
    .one(db)
    .await?;

    let prev_start = start - Duration::days((end - start).num_days());
    let prev_end = start - Duration::days(1);

    let prev_query = r#"
        SELECT 
            0::bigint as total_users,
            0::bigint as active_users,
            COALESCE(SUM("totalRequests"), 0) as total_requests,
            COALESCE(SUM("totalTokens"), 0) as total_tokens,
            COALESCE(SUM("totalCost"), 0) as total_cost
        FROM usage_summary_daily
        WHERE date >= $1 AND date <= $2
    "#;

    let previous_metrics: Option<OverviewMetrics> = OverviewMetrics::find_by_statement(
        sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            prev_query,
            vec![prev_start.into(), prev_end.into()],
        ),
    )
    .one(db)
    .await?;

    let models_query = r#"
        SELECT 
            "modelProvider" as model_provider,
            "modelName" as model_name,
            COALESCE(SUM("totalRequests"), 0) as total_requests,
            COALESCE(SUM("totalTokens"), 0) as total_tokens,
            COALESCE(SUM("totalCost"), 0) as total_cost
        FROM usage_summary_daily
        WHERE date >= $1 AND date <= $2
        GROUP BY "modelProvider", "modelName"
        ORDER BY total_requests DESC
        LIMIT 5
    "#;

    let top_models: Vec<ModelMetrics> = ModelMetrics::find_by_statement(
        sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            models_query,
            vec![start.into(), end.into()],
        ),
    )
    .all(db)
    .await?;

    let current = current_metrics.unwrap_or(OverviewMetrics {
        total_users: Some(0),
        active_users: Some(0),
        total_requests: Some(Decimal::ZERO),
        total_tokens: Some(Decimal::ZERO),
        total_cost: Some(Decimal::ZERO),
    });

    let previous = previous_metrics.unwrap_or(OverviewMetrics {
        total_users: Some(0),
        active_users: Some(0),
        total_requests: Some(Decimal::ZERO),
        total_tokens: Some(Decimal::ZERO),
        total_cost: Some(Decimal::ZERO),
    });

    let total_users = current.total_users.unwrap_or(0);
    let active_users = current.active_users.unwrap_or(0);
    let total_requests = current.total_requests.unwrap_or(Decimal::ZERO).to_i64().unwrap_or(0);
    let total_tokens = current.total_tokens.unwrap_or(Decimal::ZERO).to_i64().unwrap_or(0);
    let total_cost = current.total_cost.unwrap_or(Decimal::ZERO);

    let prev_requests = previous.total_requests.unwrap_or(Decimal::ZERO).to_i64().unwrap_or(0);
    let prev_tokens = previous.total_tokens.unwrap_or(Decimal::ZERO).to_i64().unwrap_or(0);
    let prev_cost = previous.total_cost.unwrap_or(Decimal::ZERO);

    let request_growth_rate = if prev_requests > 0 {
        ((total_requests - prev_requests) as f64 / prev_requests as f64) * 100.0
    } else {
        0.0
    };

    let token_growth_rate = if prev_tokens > 0 {
        ((total_tokens - prev_tokens) as f64 / prev_tokens as f64) * 100.0
    } else {
        0.0
    };

    let cost_growth_rate = if prev_cost > Decimal::ZERO {
        let prev_cost_f64 = prev_cost.to_f64().unwrap_or(0.0);
        let total_cost_f64 = total_cost.to_f64().unwrap_or(0.0);
        ((total_cost_f64 - prev_cost_f64) / prev_cost_f64) * 100.0
    } else {
        0.0
    };

    let average_requests_per_user = if total_users > 0 {
        total_requests as f64 / total_users as f64
    } else {
        0.0
    };

    Ok(OverviewResponse {
        total_users,
        active_users,
        total_requests,
        total_tokens,
        total_cost: total_cost.to_f64().unwrap_or(0.0),
        average_requests_per_user,
        top_models: top_models
            .into_iter()
            .map(|m| ModelUsage {
                model_provider: m.model_provider,
                model_name: m.model_name,
                total_requests: m.total_requests.unwrap_or(Decimal::ZERO).to_i64().unwrap_or(0),
                total_tokens: m.total_tokens.unwrap_or(Decimal::ZERO).to_i64().unwrap_or(0),
                total_cost: m
                    .total_cost
                    .unwrap_or(Decimal::ZERO)
                    .to_f64()
                    .unwrap_or(0.0),
            })
            .collect(),
        request_growth_rate,
        token_growth_rate,
        cost_growth_rate,
    })
}

pub async fn get_user_analytics(
    db: &DatabaseConnection,
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
    page: u64,
    limit: u64,
    sort_by: Option<String>,
    order: Option<String>,
) -> Result<UserAnalyticsResponse, DbErr> {
    let end = end_date.unwrap_or_else(|| Utc::now().date_naive());
    let start = start_date.unwrap_or_else(|| end - Duration::days(30));
    let offset = page * limit;

    let query = r#"
        SELECT 
            u.id as user_id,
            u.email as user_email,
            u.name as user_name,
            u.department,
            COALESCE(SUM(usd."totalRequests"), 0) as total_requests,
            COALESCE(SUM(usd."totalTokens"), 0) as total_tokens,
            COALESCE(SUM(usd."totalCost"), 0) as total_cost,
            COALESCE(AVG(usd."averageLatency"), 0) as average_latency,
            COALESCE(SUM(usd."successCount"), 0) as success_count,
            COALESCE(SUM(usd."errorCount"), 0) as error_count,
            MAX(ul.timestamp) as last_activity
        FROM users u
        LEFT JOIN usage_summary_daily usd ON u.id = usd."userId" 
            AND usd.date >= $1 AND usd.date <= $2
        LEFT JOIN usage_logs ul ON u.id = ul."userId"
        GROUP BY u.id, u.email, u.name, u.department
        ORDER BY total_requests DESC
        LIMIT $3 OFFSET $4
    "#;

    let users_data = UserMetrics::find_by_statement(sea_orm::Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        query,
        vec![start.into(), end.into(), limit.into(), offset.into()],
    ))
    .all(db)
    .await?;

    let count_query = r#"
        SELECT COUNT(DISTINCT u.id) as count
        FROM users u
        LEFT JOIN usage_summary_daily usd ON u.id = usd."userId" 
            AND usd.date >= $1 AND usd.date <= $2
    "#;

    #[derive(Debug, FromQueryResult)]
    struct CountResult {
        count: Option<i64>,
    }

    let total_result = CountResult::find_by_statement(sea_orm::Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        count_query,
        vec![start.into(), end.into()],
    ))
    .one(db)
    .await?;

    let total = total_result.and_then(|r| r.count).unwrap_or(0);
    let total_pages = if limit > 0 {
        (total as f64 / limit as f64).ceil() as u64
    } else {
        0
    };

    Ok(UserAnalyticsResponse {
        users: users_data
            .into_iter()
            .map(|u| UserAnalytics {
                user_id: u.user_id,
                user_email: u.user_email,
                user_name: u.user_name,
                department: u.department,
                total_requests: u.total_requests.unwrap_or(Decimal::ZERO).to_i64().unwrap_or(0),
                total_tokens: u.total_tokens.unwrap_or(Decimal::ZERO).to_i64().unwrap_or(0),
                total_cost: u
                    .total_cost
                    .unwrap_or(Decimal::ZERO)
                    .to_f64()
                    .unwrap_or(0.0),
                average_latency: u
                    .average_latency
                    .unwrap_or(Decimal::ZERO)
                    .to_f64()
                    .unwrap_or(0.0),
                success_count: u.success_count.unwrap_or(Decimal::ZERO).to_i64().unwrap_or(0),
                error_count: u.error_count.unwrap_or(Decimal::ZERO).to_i64().unwrap_or(0),
                last_activity: u.last_activity,
            })
            .collect(),
        total,
        page,
        limit,
        total_pages,
    })
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

pub async fn get_timeseries_analytics(
    db: &DatabaseConnection,
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
    granularity: String,
) -> Result<TimeSeriesResponse, DbErr> {
    let end = end_date.unwrap_or_else(|| Utc::now().date_naive());
    let start = start_date.unwrap_or_else(|| end - Duration::days(30));

    let date_trunc = match granularity.as_str() {
        "hour" => "hour",
        "day" => "day",
        "week" => "week",
        "month" => "month",
        _ => "day",
    };

    let query = format!(
        r#"
        SELECT 
            TO_CHAR(DATE_TRUNC('{}', usd.date), 'YYYY-MM-DD') as period,
            COALESCE(SUM(usd."totalRequests"), 0) as total_requests,
            COALESCE(SUM(usd."totalTokens"), 0) as total_tokens,
            COALESCE(SUM(usd."totalCost"), 0) as total_cost,
            COALESCE(AVG(usd."averageLatency"), 0) as average_latency,
            COALESCE(SUM(usd."successCount"), 0) as success_count,
            COALESCE(SUM(usd."errorCount"), 0) as error_count
        FROM usage_summary_daily usd
        WHERE usd.date >= $1 AND usd.date <= $2
        GROUP BY DATE_TRUNC('{}', usd.date)
        ORDER BY DATE_TRUNC('{}', usd.date) ASC
    "#,
        date_trunc, date_trunc, date_trunc
    );

    let timeseries_data =
        TimeSeriesMetrics::find_by_statement(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            &query,
            vec![start.into(), end.into()],
        ))
        .all(db)
        .await?;

    Ok(TimeSeriesResponse {
        data: timeseries_data
            .into_iter()
            .map(|t| TimeSeriesDataPoint {
                timestamp: t.period,
                total_requests: t.total_requests.unwrap_or(Decimal::ZERO).to_i64().unwrap_or(0),
                total_tokens: t.total_tokens.unwrap_or(Decimal::ZERO).to_i64().unwrap_or(0),
                total_cost: t
                    .total_cost
                    .unwrap_or(Decimal::ZERO)
                    .to_f64()
                    .unwrap_or(0.0),
                average_latency: t
                    .average_latency
                    .unwrap_or(Decimal::ZERO)
                    .to_f64()
                    .unwrap_or(0.0),
                success_count: t.success_count.unwrap_or(Decimal::ZERO).to_i64().unwrap_or(0),
                error_count: t.error_count.unwrap_or(Decimal::ZERO).to_i64().unwrap_or(0),
            })
            .collect(),
        granularity: granularity.to_string(),
    })
}
