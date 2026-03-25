use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use crate::models::users::UserStatus;

#[derive(Debug, Deserialize, ToSchema)]
pub struct AnalyticsQuery {
    pub start_date: Option<NaiveDate>,
    pub end_date: Option<NaiveDate>,
    pub department: Option<String>,
    pub model_provider: Option<String>,
    pub model_name: Option<String>,
    pub live: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UserAnalyticsQuery {
    pub start_date: Option<NaiveDate>,
    pub end_date: Option<NaiveDate>,
    pub page: Option<u64>,
    pub limit: Option<u64>,
    pub sort_by: Option<String>,
    pub order: Option<String>,
    pub search:Option<String>,
    pub role_id:Option<Uuid>,
    pub status:Option<UserStatus>,
    pub unassigned_department:Option<bool>,
    pub live: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ScopedUserAnalyticsQuery {
    pub start_date: Option<NaiveDate>,
    pub end_date: Option<NaiveDate>,
    pub page: Option<u64>,
    pub limit: Option<u64>,
    pub sort_by: Option<String>,
    pub order: Option<String>,
    pub search: Option<String>,
    pub role_id: Option<Uuid>,
    pub status: Option<UserStatus>,
    pub department_id: Option<Uuid>,
    pub live: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DepartmentAnalyticsQuery {
    pub start_date: Option<NaiveDate>,
    pub end_date: Option<NaiveDate>,
    pub offset: Option<u64>,
    pub limit: Option<u64>,
    pub search: Option<String>,
    pub live: Option<bool>,
    pub department_id:Option<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TimeSeriesQuery {
    pub start_date: Option<NaiveDate>,
    pub end_date: Option<NaiveDate>,
    pub granularity: Option<String>,
    pub group_by: Option<String>,
    pub live: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct OverviewResponse {
    pub total_users: i64,
    pub active_users: i64,
    pub total_requests: i64,
    pub total_tokens: i64,
    pub total_cost: f64,
    pub average_requests_per_user: f64,
    pub top_models: Vec<ModelUsage>,
    pub request_growth_rate: f64,
    pub token_growth_rate: f64,
    pub cost_growth_rate: f64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ModelUsage {
    pub model_provider: String,
    pub model_name: String,
    pub total_requests: i64,
    pub total_tokens: i64,
    pub total_cost: f64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UserAnalytics {
    pub user_id: Uuid,
    pub user_email: String,
    pub user_name: Option<String>,
    pub department_id: Option<Uuid>,
    pub total_requests: i64,
    pub total_tokens: i64,
    pub total_cost: f64,
    pub average_latency: f64,
    pub success_count: i64,
    pub error_count: i64,
    pub department:Option<String>,
    pub last_activity: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UserAnalyticsResponse {
    pub users: Vec<UserAnalytics>,
    pub total: i64,
    pub page: u64,
    pub limit: u64,
    pub total_pages: u64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DepartmentAnalytics {
    pub department: String,
    pub total_users: i64,
    pub total_requests: i64,
    pub total_tokens: i64,
    pub total_cost: f64,
    pub average_latency: f64,
    pub success_count: i64,
    pub error_count: i64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DepartmentAnalyticsResponse {
    pub departments: Vec<DepartmentAnalytics>,
    pub total: i64,
    pub limit: u64,
    pub offset: u64,
    pub total_pages: u64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TimeSeriesDataPoint {
    pub timestamp: String,
    pub total_requests: i64,
    pub total_tokens: i64,
    pub total_cost: f64,
    pub average_latency: f64,
    pub success_count: i64,
    pub error_count: i64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TimeSeriesResponse {
    pub data: Vec<TimeSeriesDataPoint>,
    pub granularity: String,
}
