// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{auth::claims::Claims, state::SharedState};
use axum::{
    Json,
    extract::{FromRequestParts, Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum ShareOperation {
    Read,
    Download,
    Fork,
    Change,
    Revoke,
}

impl ShareOperation {
    fn limit(self) -> u32 {
        match self {
            Self::Read => 120,
            Self::Download => 600,
            Self::Fork => 10,
            Self::Change => 60,
            Self::Revoke => 60,
        }
    }

    fn for_request(method: &Method, path: &str) -> Self {
        if *method == Method::DELETE {
            Self::Revoke
        } else if path.ends_with("/fork") {
            Self::Fork
        } else if *method == Method::POST || *method == Method::PUT {
            Self::Change
        } else if path.starts_with("/share/") && path.contains("/files/") {
            Self::Download
        } else {
            Self::Read
        }
    }
}

struct Window {
    started: Instant,
    touched: Instant,
    count: u32,
}

#[derive(Default)]
pub struct ShareRateLimiter {
    windows: Mutex<HashMap<(Uuid, ShareOperation), Window>>,
}

impl ShareRateLimiter {
    fn admit(&self, user: Uuid, operation: ShareOperation, now: Instant) -> Result<(), u64> {
        let mut windows = self
            .windows
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        windows.retain(|_, window| {
            now.saturating_duration_since(window.started) < Duration::from_secs(60)
        });
        let key = (user, operation);
        // Evict an old authenticated bucket rather than denying every new user at capacity.
        if windows.len() >= 16384
            && !windows.contains_key(&key)
            && let Some(oldest) = windows
                .iter()
                .min_by_key(|(_, window)| window.touched)
                .map(|(key, _)| *key)
        {
            windows.remove(&oldest);
        }
        let window = windows.entry(key).or_insert(Window {
            started: now,
            touched: now,
            count: 0,
        });
        window.touched = now;
        if window.count >= operation.limit() {
            return Err(60u64
                .saturating_sub(now.saturating_duration_since(window.started).as_secs())
                .max(1));
        }
        window.count += 1;
        Ok(())
    }
}

pub async fn share_middleware(
    State(state): State<SharedState>,
    req: Request,
    next: Next,
) -> Response {
    let (mut parts, body) = req.into_parts();
    let viewer = match Claims::from_request_parts(&mut parts, &state).await {
        Ok(viewer) => viewer,
        Err(error) => return protect_share_response(error.into_response()),
    };
    let operation = ShareOperation::for_request(&parts.method, parts.uri.path());
    let response = match state
        .share_rate_limiter
        .admit(viewer.user_id, operation, Instant::now())
    {
        Ok(()) => next.run(Request::from_parts(parts, body)).await,
        Err(retry_after) => {
            let mut response = (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({"error":"share_rate_limited","retry_after":retry_after})),
            )
                .into_response();
            if let Ok(value) = HeaderValue::from_str(&retry_after.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
            response
        }
    };
    protect_share_response(response)
}

fn protect_share_response(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
        .headers_mut()
        .insert(header::VARY, HeaderValue::from_static("Authorization"));
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow, noarchive"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

pub fn fork_busy_response() -> Response {
    protect_share_response(
        (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, "1")],
            Json(serde_json::json!({"error":"share_fork_busy","retry_after":1})),
        )
            .into_response(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fork_capacity_exhaustion_is_retryable_and_private() {
        let response = fork_busy_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[header::RETRY_AFTER], "1");
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"error":"share_fork_busy","retry_after":1})
        );
    }
    #[test]
    fn limits_are_per_user_operation_and_window() {
        let limiter = ShareRateLimiter::default();
        let now = Instant::now();
        let user = Uuid::new_v4();
        for operation in [
            ShareOperation::Read,
            ShareOperation::Download,
            ShareOperation::Fork,
            ShareOperation::Change,
            ShareOperation::Revoke,
        ] {
            for _ in 0..operation.limit() {
                assert!(limiter.admit(user, operation, now).is_ok());
            }
            assert_eq!(limiter.admit(user, operation, now), Err(60));
            assert!(limiter.admit(Uuid::new_v4(), operation, now).is_ok());
            assert!(
                limiter
                    .admit(user, operation, now + Duration::from_secs(60))
                    .is_ok()
            );
        }
    }
    #[test]
    fn full_cache_does_not_block_new_users_and_memory_stays_bounded() {
        let limiter = ShareRateLimiter::default();
        let now = Instant::now();
        for _ in 0..16385 {
            assert!(
                limiter
                    .admit(Uuid::new_v4(), ShareOperation::Read, now)
                    .is_ok()
            );
        }
        assert_eq!(limiter.windows.lock().unwrap().len(), 16384);
        assert!(
            limiter
                .admit(
                    Uuid::new_v4(),
                    ShareOperation::Read,
                    now + Duration::from_secs(60)
                )
                .is_ok()
        );
        assert_eq!(limiter.windows.lock().unwrap().len(), 1);
    }

    #[test]
    fn routes_have_separate_read_download_change_and_revoke_budgets() {
        for (method, path, expected) in [
            (Method::GET, "/share/token", ShareOperation::Read),
            (
                Method::GET,
                "/share/token/files/id",
                ShareOperation::Download,
            ),
            (Method::POST, "/share/token/fork", ShareOperation::Fork),
            (Method::PUT, "/chat/id/shares/id", ShareOperation::Change),
            (Method::POST, "/chat/id/shares", ShareOperation::Change),
            (Method::DELETE, "/chat/id/shares/id", ShareOperation::Revoke),
        ] {
            assert_eq!(ShareOperation::for_request(&method, path), expected);
        }
    }
}
