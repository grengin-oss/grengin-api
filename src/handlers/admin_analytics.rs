use axum::{
    extract::{Query, State},
    Json,
};
use reqwest::StatusCode;
use sea_orm::DatabaseConnection;

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, AuthErrorResponse},
        permissions::PERMISSION_ANALYTICS_VIEW,
    },
    dto::analytics::{
        AnalyticsQuery, DepartmentAnalyticsQuery, DepartmentAnalyticsResponse, OverviewResponse,
        TimeSeriesQuery, TimeSeriesResponse, UserAnalyticsQuery, UserAnalyticsResponse,
    },
    models::users::{UserRole, UserStatus},
    services::{analytics_cache, authorization::{AuthorizationService, PermissionScopeMode}},
    state::SharedState,
};

#[utoipa::path(
    get,
    path = "/admin/analytics/overview",
    tag = "analytics",
    params(
        ("start_date" = Option<String>, Query, description = "Start date (YYYY-MM-DD)"),
        ("end_date" = Option<String>, Query, description = "End date (YYYY-MM-DD)"),
        ("live" = Option<bool>, Query, description = "Bypass cache and fetch live data"),
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
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            claims.role,
            PERMISSION_ANALYTICS_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let result = analytics_cache::get_overview_cached(
        &app_state.database,
        query.start_date,
        query.end_date,
        query.live.unwrap_or(false),
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
        ("sort_by" = Option<String>, Query, description = "Sort field by name,email,totalRequests,totalTokens,totalCost,averageLatency,lastActivity"),
        ("order" = Option<String>, Query, description = "Sort order (asc/desc)"),
        ("search" = Option<String>, Query, description = "Search by name,email or department"),
        ("status" = Option<UserStatus>, Query, description = "Account status"),
        ("role" = Option<UserRole>, Query, description = "UserRole superadmin,admin,user,observer"),
        ("unassigned_department" = Option<bool>, Query, description = "Default false"),
        ("live" = Option<bool>, Query, description = "Bypass cache and fetch live data"),
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
) -> Result<(axum::http::StatusCode, Json<UserAnalyticsResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            claims.role,
            PERMISSION_ANALYTICS_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;
    let db: &DatabaseConnection = &app_state.database;
    let result = analytics_cache::get_user_analytics_cached(db, query)
        .await
        .map_err(|e| { 
            eprintln!("{}",e);
            AuthError::DbTimeout})?;

    Ok((StatusCode::OK, Json(result)))
}

#[utoipa::path(
    get,
    path = "/admin/analytics/departments",
    tag = "analytics",
    params(
        ("start_date" = Option<String>, Query, description = "Start date (YYYY-MM-DD)"),
        ("end_date" = Option<String>, Query, description = "End date (YYYY-MM-DD)"),
        ("offset" = Option<u64>, Query, description = "Number of items to skip (default: 0)"),
        ("limit" = Option<u64>, Query, description = "Items per page (default: 20)"),
        ("search" = Option<String>, Query, description = "Search by department name"),
        ("live" = Option<bool>, Query, description = "Bypass cache and fetch live data"),
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
    Query(query): Query<DepartmentAnalyticsQuery>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<DepartmentAnalyticsResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            claims.role,
            PERMISSION_ANALYTICS_VIEW,
            query.department_id,
            PermissionScopeMode::RequireOrgWide,
            query.department_id,
        )
        .await?;

    let result = analytics_cache::get_department_analytics_cached(&app_state.database, query)
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
        ("live" = Option<bool>, Query, description = "Bypass cache and fetch live data"),
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
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            claims.role,
            PERMISSION_ANALYTICS_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let result = analytics_cache::get_timeseries_analytics_cached(
        &app_state.database,
        query,
    )
    .await
    .map_err(|e| {
        eprintln!("Timeseries analytics error: {}", e);
        AuthError::DbTimeout
    })?;

    Ok((StatusCode::OK, Json(result)))
}
