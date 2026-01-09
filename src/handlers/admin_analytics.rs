use axum::{extract::State, Json, extract::Query};
use reqwest::StatusCode;

use crate::{
    auth::{claims::Claims, error::AuthError},
    dto::analytics::{
        AnalyticsQuery, DepartmentAnalyticsResponse, OverviewResponse, TimeSeriesQuery,
        TimeSeriesResponse, UserAnalyticsQuery, UserAnalyticsResponse,
    },
    error::{AppError, ErrorDetail, ErrorResponse, ErrorDetailVariant},
    models::users::UserRole,
    services::{analytics, aggregation},
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
        (status = 403, description = "Permission denied"),
        (status = 500, description = "Internal server error"),
    )
)]
pub async fn get_analytics_overview(
    claims: Claims,
    Query(query): Query<AnalyticsQuery>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<OverviewResponse>), (StatusCode, Json<ErrorResponse>)> {
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => {
            return Err((
                StatusCode::FORBIDDEN,
                Json(ErrorResponse {
                    detail: ErrorDetailVariant::Rich(ErrorDetail {
                        code: "ANALYTICS_PERMISSION_DENIED".to_string(),
                        message: "You do not have permission to access analytics data".to_string(),
                    }),
                }),
            ))
        }
    }

    let result = analytics::get_overview_analytics(
        &app_state.database,
        query.start_date,
        query.end_date,
    )
    .await
    .map_err(|e| {
        eprintln!("Analytics overview error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                detail: ErrorDetailVariant::Rich(ErrorDetail {
                    code: "ANALYTICS_QUERY_FAILED".to_string(),
                    message: "Failed to retrieve analytics overview".to_string(),
                }),
            }),
        )
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
        (status = 403, description = "Permission denied"),
        (status = 500, description = "Internal server error"),
    )
)]
pub async fn get_user_analytics(
    claims: Claims,
    Query(query): Query<UserAnalyticsQuery>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<UserAnalyticsResponse>), (StatusCode, Json<ErrorResponse>)> {
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => {
            return Err((
                StatusCode::FORBIDDEN,
                Json(ErrorResponse {
                    detail: ErrorDetailVariant::Rich(ErrorDetail {
                        code: "ANALYTICS_PERMISSION_DENIED".to_string(),
                        message: "You do not have permission to access analytics data".to_string(),
                    }),
                }),
            ))
        }
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
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                detail: ErrorDetailVariant::Rich(ErrorDetail {
                    code: "ANALYTICS_QUERY_FAILED".to_string(),
                    message: "Failed to retrieve user analytics".to_string(),
                }),
            }),
        )
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
        (status = 403, description = "Permission denied"),
        (status = 500, description = "Internal server error"),
    )
)]
pub async fn get_department_analytics(
    claims: Claims,
    Query(query): Query<AnalyticsQuery>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<DepartmentAnalyticsResponse>), (StatusCode, Json<ErrorResponse>)> {
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => {
            return Err((
                StatusCode::FORBIDDEN,
                Json(ErrorResponse {
                    detail: ErrorDetailVariant::Rich(ErrorDetail {
                        code: "ANALYTICS_PERMISSION_DENIED".to_string(),
                        message: "You do not have permission to access analytics data".to_string(),
                    }),
                }),
            ))
        }
    }

    let result = analytics::get_department_analytics(
        &app_state.database,
        query.start_date,
        query.end_date,
    )
    .await
    .map_err(|e| {
        eprintln!("Department analytics error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                detail: ErrorDetailVariant::Rich(ErrorDetail {
                    code: "ANALYTICS_QUERY_FAILED".to_string(),
                    message: "Failed to retrieve department analytics".to_string(),
                }),
            }),
        )
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
        (status = 403, description = "Permission denied"),
        (status = 500, description = "Internal server error"),
    )
)]
pub async fn get_timeseries_analytics(
    claims: Claims,
    Query(query): Query<TimeSeriesQuery>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<TimeSeriesResponse>), (StatusCode, Json<ErrorResponse>)> {
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => {
            return Err((
                StatusCode::FORBIDDEN,
                Json(ErrorResponse {
                    detail: ErrorDetailVariant::Rich(ErrorDetail {
                        code: "ANALYTICS_PERMISSION_DENIED".to_string(),
                        message: "You do not have permission to access analytics data".to_string(),
                    }),
                }),
            ))
        }
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
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                detail: ErrorDetailVariant::Rich(ErrorDetail {
                    code: "ANALYTICS_QUERY_FAILED".to_string(),
                    message: "Failed to retrieve timeseries analytics".to_string(),
                }),
            }),
        )
    })?;

    Ok((StatusCode::OK, Json(result)))
}

#[utoipa::path(
    post,
    path = "/admin/analytics/aggregate",
    tag = "analytics",
    responses(
        (status = 200, description = "Aggregation job completed successfully"),
        (status = 403, description = "Permission denied"),
        (status = 500, description = "Internal server error"),
    )
)]
pub async fn trigger_aggregation_job(
    claims: Claims,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<ErrorResponse>)> {
    match claims.role {
        UserRole::SuperAdmin => {}
        _ => {
            return Err((
                StatusCode::FORBIDDEN,
                Json(ErrorResponse {
                    detail: ErrorDetailVariant::Rich(ErrorDetail {
                        code: "AGGREGATION_PERMISSION_DENIED".to_string(),
                        message: "Only super admins can trigger aggregation jobs".to_string(),
                    }),
                }),
            ))
        }
    }

    let result = aggregation::run_daily_aggregation_job(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("Aggregation job error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    detail: ErrorDetailVariant::Rich(ErrorDetail {
                        code: "AGGREGATION_JOB_FAILED".to_string(),
                        message: "Failed to run aggregation job".to_string(),
                    }),
                }),
            )
        })?;

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "message": result,
            "status": "success"
        })),
    ))
}
