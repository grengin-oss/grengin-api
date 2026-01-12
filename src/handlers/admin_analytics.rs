use axum::{
    extract::{Query, State},
    Json,
};
use reqwest::StatusCode;

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, AuthErrorResponse},
    },
    dto::analytics::{
        AnalyticsQuery, DepartmentAnalyticsResponse, OverviewResponse, TimeSeriesQuery,
        TimeSeriesResponse, UserAnalyticsQuery, UserAnalyticsResponse,
    },
    models::users::UserRole,
    services::{aggregation, analytics},
    state::SharedState,
};

#[utoipa::path(
    get,
    path = "/admin/analytics/overview",
    tag = "analytics",
    params(
        ("start_date" = Option<String>, Query, description = "Start date (YYYY-MM-DD)"),
        ("end_date" = Option<String>, Query, description = "End date (YYYY-MM-DD)"),
    ),
    responses(
        (status = 200, description = "Dashboard overview statistics", body = OverviewResponse),

        (status = 400, content_type = "application/json", body = AuthErrorResponse, description = "Missing credentials (code=6102)"),
        (status = 401, content_type = "application/json", body = AuthErrorResponse, description = "Invalid/expired access token (code=6103)"),
        (status = 403, content_type = "application/json", body = AuthErrorResponse, description = "Permission denied (code=6300)"),
        (status = 503, content_type = "application/json", body = AuthErrorResponse, description = "DB timeout/unavailable (code=5001/5000)"),
    )
)]
pub async fn get_analytics_overview(
    claims: Claims,
    Query(query): Query<AnalyticsQuery>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<OverviewResponse>), AuthError> {
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
    }

    let result = analytics::get_overview_analytics(
        &app_state.database,
        query.start_date,
        query.end_date,
    )
    .await
    .map_err(|e| {
        eprintln!("Analytics overview error: {}", e);
        AuthError::DbTimeout
    })?;

    Ok((StatusCode::OK, Json(result)))
}

#[utoipa::path(
    get,
    path = "/admin/analytics/users",
    tag = "analytics",
    params(
        ("start_date" = Option<String>, Query, description = "Start date (YYYY-MM-DD)"),
        ("end_date" = Option<String>, Query, description = "End date (YYYY-MM-DD)"),
        ("page" = Option<u64>, Query, description = "Page number (default: 0)"),
        ("limit" = Option<u64>, Query, description = "Items per page (default: 20)"),
        ("sort_by" = Option<String>, Query, description = "Sort field"),
        ("order" = Option<String>, Query, description = "Sort order (asc/desc)"),
    ),
    responses(
        (status = 200, description = "User analytics with pagination", body = UserAnalyticsResponse),

        (status = 400, content_type = "application/json", body = AuthErrorResponse, description = "Missing credentials (code=6102)"),
        (status = 401, content_type = "application/json", body = AuthErrorResponse, description = "Invalid/expired access token (code=6103)"),
        (status = 403, content_type = "application/json", body = AuthErrorResponse, description = "Permission denied (code=6300)"),
        (status = 503, content_type = "application/json", body = AuthErrorResponse, description = "DB timeout/unavailable (code=5001/5000)"),
    )
)]
pub async fn get_user_analytics(
    claims: Claims,
    Query(query): Query<UserAnalyticsQuery>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<UserAnalyticsResponse>), AuthError> {
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
    }

    let page = query.page.unwrap_or(0);
    let limit = query.limit.unwrap_or(20);

    let result = analytics::get_user_analytics(
        &app_state.database,
        query.start_date,
        query.end_date,
        page,
        limit,
        query.sort_by,
        query.order,
    )
    .await
    .map_err(|e| {
        eprintln!("User analytics error: {}", e);
        AuthError::DbTimeout
    })?;

    Ok((StatusCode::OK, Json(result)))
}

#[utoipa::path(
    get,
    path = "/admin/analytics/departments",
    tag = "analytics",
    params(
        ("start_date" = Option<String>, Query, description = "Start date (YYYY-MM-DD)"),
        ("end_date" = Option<String>, Query, description = "End date (YYYY-MM-DD)"),
    ),
    responses(
        (status = 200, description = "Department analytics", body = DepartmentAnalyticsResponse),

        (status = 400, content_type = "application/json", body = AuthErrorResponse, description = "Missing credentials (code=6102)"),
        (status = 401, content_type = "application/json", body = AuthErrorResponse, description = "Invalid/expired access token (code=6103)"),
        (status = 403, content_type = "application/json", body = AuthErrorResponse, description = "Permission denied (code=6300)"),
        (status = 503, content_type = "application/json", body = AuthErrorResponse, description = "DB timeout/unavailable (code=5001/5000)"),
    )
)]
pub async fn get_department_analytics(
    claims: Claims,
    Query(query): Query<AnalyticsQuery>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<DepartmentAnalyticsResponse>), AuthError> {
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
    }

    let result = analytics::get_department_analytics(
        &app_state.database,
        query.start_date,
        query.end_date,
    )
    .await
    .map_err(|e| {
        eprintln!("Department analytics error: {}", e);
        AuthError::DbTimeout
    })?;

    Ok((StatusCode::OK, Json(result)))
}

#[utoipa::path(
    get,
    path = "/admin/analytics/timeseries",
    tag = "analytics",
    params(
        ("start_date" = Option<String>, Query, description = "Start date (YYYY-MM-DD)"),
        ("end_date" = Option<String>, Query, description = "End date (YYYY-MM-DD)"),
        ("granularity" = Option<String>, Query, description = "Time granularity (hour/day/week/month)"),
        ("group_by" = Option<String>, Query, description = "Group by dimension"),
    ),
    responses(
        (status = 200, description = "Time series analytics data", body = TimeSeriesResponse),

        (status = 400, content_type = "application/json", body = AuthErrorResponse, description = "Missing credentials (code=6102)"),
        (status = 401, content_type = "application/json", body = AuthErrorResponse, description = "Invalid/expired access token (code=6103)"),
        (status = 403, content_type = "application/json", body = AuthErrorResponse, description = "Permission denied (code=6300)"),
        (status = 503, content_type = "application/json", body = AuthErrorResponse, description = "DB timeout/unavailable (code=5001/5000)"),
    )
)]
pub async fn get_timeseries_analytics(
    claims: Claims,
    Query(query): Query<TimeSeriesQuery>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<TimeSeriesResponse>), AuthError> {
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
    }

    let granularity = query.granularity.unwrap_or_else(|| "day".to_string());

    let result = analytics::get_timeseries_analytics(
        &app_state.database,
        query.start_date,
        query.end_date,
        granularity,
    )
    .await
    .map_err(|e| {
        eprintln!("Timeseries analytics error: {}", e);
        AuthError::DbTimeout
    })?;

    Ok((StatusCode::OK, Json(result)))
}

#[utoipa::path(
    post,
    path = "/admin/analytics/aggregate",
    tag = "analytics",
    responses(
        (status = 200, description = "Aggregation job completed successfully", body = serde_json::Value),

        (status = 400, content_type = "application/json", body = AuthErrorResponse, description = "Missing credentials (code=6102)"),
        (status = 401, content_type = "application/json", body = AuthErrorResponse, description = "Invalid/expired access token (code=6103)"),
        (status = 403, content_type = "application/json", body = AuthErrorResponse, description = "Permission denied (code=6300)"),
        (status = 503, content_type = "application/json", body = AuthErrorResponse, description = "DB timeout/unavailable (code=5001/5000)"),
    )
)]
pub async fn trigger_aggregation_job(
    claims: Claims,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<serde_json::Value>), AuthError> {
    match claims.role {
        UserRole::SuperAdmin => {}
        _ => return Err(AuthError::PermissionDenied),
    }

    let result = aggregation::run_daily_aggregation_job(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("Aggregation job error: {}", e);
            AuthError::DbTimeout
        })?;

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "message": result,
            "status": "success"
        })),
    ))
}
