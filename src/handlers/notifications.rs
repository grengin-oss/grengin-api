// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::convert::Infallible;

use axum::{
    Json,
    extract::{Path, Query, State},
    response::{
        Sse,
        sse::{Event, KeepAlive},
    },
};
use chrono::Utc;
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect};
use uuid::Uuid;

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
    },
    dto::notifications::{NotificationDto, NotificationsListQuery, NotificationsListResponse},
    error::AppError,
    models::notifications,
    services::notifications::to_notification_dto,
    state::SharedState,
};

#[utoipa::path(
    get,
    path = "/me/notifications",
    tag = "me",
    params(
        ("limit" = Option<u64>, Query, description = "Max number of notifications"),
        ("offset" = Option<u64>, Query, description = "Offset for pagination"),
        ("unread_only" = Option<bool>, Query, description = "Only unread notifications"),
        ("created_from" = Option<DateTime<Utc>>, Query, description = "Filter by created_at >= (RFC3339)"),
        ("created_to" = Option<DateTime<Utc>>, Query, description = "Filter by created_at <= (RFC3339)"),
    ),
    responses(
        (status = 200, body = NotificationsListResponse),
        (status = 401, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn list_my_notifications(
    claims: Claims,
    State(state): State<SharedState>,
    Query(query): Query<NotificationsListQuery>,
) -> Result<Json<NotificationsListResponse>, AuthError> {
    let limit = query.limit.unwrap_or(20).min(100) as u64;
    let offset = query.offset.unwrap_or(0) as u64;

    let mut base = notifications::Entity::find()
        .filter(notifications::Column::UserId.eq(claims.user_id))
        .order_by_desc(notifications::Column::CreatedAt);
    if query.unread_only.unwrap_or(false) {
        base = base.filter(notifications::Column::ReadAt.is_null());
    }
    if let Some(from) = query.created_from {
        base = base.filter(notifications::Column::CreatedAt.gte(from));
    }
    if let Some(to) = query.created_to {
        base = base.filter(notifications::Column::CreatedAt.lte(to));
    }

    let total = base.clone().count(&state.database).await.map_err(|e| {
        eprintln!("notifications count error: {e}");
        AuthError::DbTimeout
    })?;

    let rows = base
        .offset(offset)
        .limit(limit)
        .all(&state.database)
        .await
        .map_err(|e| {
            eprintln!("notifications list error: {e}");
            AuthError::DbTimeout
        })?;

    let notifications = rows.iter().map(to_notification_dto).collect();

    Ok(Json(NotificationsListResponse {
        notifications,
        total: total as i64,
    }))
}

#[utoipa::path(
    post,
    path = "/me/notifications/{notification_id}/read",
    tag = "me",
    params(
        ("notification_id" = Uuid, Path, description = "Notification id")
    ),
    responses(
        (status = 204, description = "Marked as read"),
        (status = 401, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn mark_notification_read(
    claims: Claims,
    State(state): State<SharedState>,
    Path(notification_id): Path<Uuid>,
) -> Result<axum::http::StatusCode, AuthError> {
    let result = notifications::Entity::update_many()
        .col_expr(notifications::Column::ReadAt, Expr::value(Utc::now()))
        .filter(notifications::Column::Id.eq(notification_id))
        .filter(notifications::Column::UserId.eq(claims.user_id))
        .exec(&state.database)
        .await
        .map_err(|e| {
            eprintln!("notification mark read error: {e}");
            AuthError::DbTimeout
        })?;

    if result.rows_affected == 0 {
        return Err(AuthError::ResourceNotFound);
    }

    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/me/notifications/stream",
    tag = "me",
    responses(
        (status = 200, content_type = "text/event-stream", body = NotificationDto),
        (status = 401, content_type = "application/json", body = Error)
    )
)]
pub async fn stream_my_notifications(
    claims: Claims,
    State(state): State<SharedState>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, AppError> {
    let mut rx = state.notification_hub.subscribe();
    let user_id = claims.user_id;

    let sse_stream = async_stream::try_stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if event.user_id == user_id {
                        let data = serde_json::to_string(&event.notification)
                            .unwrap_or_else(|_| "{}".to_string());
                        yield Event::default().event("notification").data(data);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Ok(Sse::new(sse_stream).keep_alive(KeepAlive::new()))
}
