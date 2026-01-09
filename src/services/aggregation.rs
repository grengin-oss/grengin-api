use chrono::{Duration, NaiveDate, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, DbErr, EntityTrait,
    FromQueryResult, IntoActiveModel, QueryFilter, QuerySelect,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::{usage_logs, usage_summary_daily};

use rust_decimal::Decimal;

#[derive(Debug, FromQueryResult, Serialize, Deserialize)]
struct DailyAggregation {
    user_id: Uuid,
    department: Option<String>,
    model_provider: String,
    model_name: String,
    total_requests: Option<i64>,
    total_tokens: Option<i64>,
    total_cost: Option<Decimal>,
    average_latency: Option<Decimal>,
    success_count: Option<i64>,
    error_count: Option<i64>,
}

pub async fn aggregate_daily_usage(
    db: &DatabaseConnection,
    target_date: Option<NaiveDate>,
) -> Result<u64, DbErr> {
    let date = target_date.unwrap_or_else(|| (Utc::now() - Duration::days(1)).date_naive());

    let start_timestamp = date
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_local_timezone(Utc)
        .unwrap();
    let end_timestamp = date
        .and_hms_opt(23, 59, 59)
        .unwrap()
        .and_local_timezone(Utc)
        .unwrap();

    let query = r#"
        SELECT 
            "userId" as user_id,
            department,
            "modelProvider" as model_provider,
            "modelName" as model_name,
            COUNT(*) as total_requests,
            COALESCE(SUM("totalTokens"), 0) as total_tokens,
            COALESCE(SUM("costUsd"), 0) as total_cost,
            COALESCE(AVG("latencyMs"), 0) as average_latency,
            SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END) as success_count,
            SUM(CASE WHEN status = 'error' THEN 1 ELSE 0 END) as error_count
        FROM usage_logs
        WHERE timestamp >= $1 AND timestamp <= $2
        GROUP BY "userId", department, "modelProvider", "modelName"
    "#;

    let aggregations = DailyAggregation::find_by_statement(
        sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            query,
            vec![start_timestamp.into(), end_timestamp.into()],
        ),
    )
    .all(db)
    .await?;

    let mut inserted_count = 0u64;

    for agg in aggregations {
        let existing = usage_summary_daily::Entity::find()
            .filter(usage_summary_daily::Column::Date.eq(date))
            .filter(usage_summary_daily::Column::UserId.eq(agg.user_id))
            .filter(usage_summary_daily::Column::ModelProvider.eq(&agg.model_provider))
            .filter(usage_summary_daily::Column::ModelName.eq(&agg.model_name))
            .one(db)
            .await?;

        if let Some(existing_record) = existing {
            let mut active_model = existing_record.into_active_model();
            active_model.total_requests = Set(agg.total_requests.unwrap_or(0));
            active_model.total_tokens = Set(agg.total_tokens.unwrap_or(0));
            active_model.total_cost = Set(agg.total_cost.unwrap_or(Decimal::ZERO));
            active_model.average_latency =
                Set(agg.average_latency.unwrap_or(Decimal::ZERO));
            active_model.success_count = Set(agg.success_count.unwrap_or(0));
            active_model.error_count = Set(agg.error_count.unwrap_or(0));
            active_model.updated_at = Set(Utc::now());
            active_model.update(db).await?;
        } else {
            let new_summary = usage_summary_daily::ActiveModel {
                id: Set(Uuid::new_v4()),
                date: Set(date),
                user_id: Set(agg.user_id),
                department: Set(agg.department),
                model_provider: Set(agg.model_provider),
                model_name: Set(agg.model_name),
                total_requests: Set(agg.total_requests.unwrap_or(0)),
                total_tokens: Set(agg.total_tokens.unwrap_or(0)),
                total_cost: Set(agg.total_cost.unwrap_or(Decimal::ZERO)),
                average_latency: Set(agg
                    .average_latency
                    .unwrap_or(Decimal::ZERO)),
                success_count: Set(agg.success_count.unwrap_or(0)),
                error_count: Set(agg.error_count.unwrap_or(0)),
                created_at: Set(Utc::now()),
                updated_at: Set(Utc::now()),
            };
            new_summary.insert(db).await?;
            inserted_count += 1;
        }
    }

    Ok(inserted_count)
}

pub async fn run_daily_aggregation_job(db: &DatabaseConnection) -> Result<String, DbErr> {
    let yesterday = (Utc::now() - Duration::days(1)).date_naive();
    
    let count = aggregate_daily_usage(db, Some(yesterday)).await?;
    
    Ok(format!(
        "Daily aggregation completed for {}. Processed {} records.",
        yesterday, count
    ))
}
