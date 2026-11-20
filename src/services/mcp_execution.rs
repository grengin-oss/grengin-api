// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::future::Future;

use crate::state::StreamCancel;

pub async fn cancellable_tool_call<T>(
    cancel: &StreamCancel,
    call: impl Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => None,
        result = call => Some(result),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::messages::StopReason;
    use std::sync::Arc;
    use tokio::sync::{Mutex, Notify};
    use uuid::Uuid;

    #[tokio::test]
    async fn stop_drops_the_active_call_and_releases_its_lease() {
        let cancel = Arc::new(StreamCancel::new(Uuid::new_v4()));
        let lease = Arc::new(Mutex::new(()));
        let entered = Arc::new(Notify::new());
        let task = tokio::spawn({
            let cancel = cancel.clone();
            let lease = lease.clone();
            let entered = entered.clone();
            async move {
                cancellable_tool_call(&cancel, async {
                    let _lease = lease.lock().await;
                    entered.notify_one();
                    std::future::pending::<()>().await;
                })
                .await
            }
        });
        entered.notified().await;
        cancel.cancel(StopReason::UserCancelled);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(200), task)
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        assert!(lease.try_lock().is_ok());
    }

    #[tokio::test]
    async fn an_already_cancelled_chat_never_dispatches_the_tool() {
        let cancel = StreamCancel::new(Uuid::new_v4());
        cancel.cancel(StopReason::UserCancelled);
        let mut dispatched = false;
        assert!(
            cancellable_tool_call(&cancel, async {
                dispatched = true;
            })
            .await
            .is_none()
        );
        assert!(!dispatched);
    }

    #[tokio::test]
    async fn stop_cancels_a_real_remote_call_without_starting_the_next_tool_round() {
        use crate::{
            models::mcp_servers::McpTransportType,
            services::{
                chat_stream_helpers::{ToolRoundOutcome, next_tool_round},
                mcp_client::McpServerClient,
            },
        };
        use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
        use serde_json::{Value, json};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        let calls = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(Notify::new());
        let cancelled_at_server = Arc::new(Notify::new());
        let router = Router::new().route("/mcp", post({
            let calls = calls.clone();
            let entered = entered.clone();
            let cancelled_at_server = cancelled_at_server.clone();
            move |Json(message): Json<Value>| {
                let calls = calls.clone();
                let entered = entered.clone();
                let cancelled_at_server = cancelled_at_server.clone();
                async move {
                    if message.get("id").is_none() {
                        if message["method"] == "notifications/cancelled" {
                            cancelled_at_server.notify_one();
                        }
                        return StatusCode::ACCEPTED.into_response();
                    }
                    let result = match message["method"].as_str().unwrap() {
                        "initialize" => json!({"protocolVersion":message["params"]["protocolVersion"],
                            "capabilities":{"tools":{}},"serverInfo":{"name":"stop", "version":"1"}}),
                        "tools/call" => {
                            calls.fetch_add(1, Ordering::SeqCst);
                            if message["params"]["name"] == "hang" {
                                entered.notify_one();
                                std::future::pending::<()>().await;
                            }
                            json!({"content":[]})
                        }
                        _ => return StatusCode::BAD_REQUEST.into_response(),
                    };
                    Json(json!({"jsonrpc":"2.0","id":message["id"],"result":result})).into_response()
                }
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = Arc::new(McpServerClient::new(
            Uuid::new_v4(),
            "stop".into(),
            McpTransportType::Http,
            Some(url),
            json!({}),
            None,
        ));
        let user = Uuid::new_v4();
        let cancel = Arc::new(StreamCancel::new(Uuid::new_v4()));
        let task = tokio::spawn({
            let cancel = cancel.clone();
            let client = client.clone();
            async move {
                for name in ["hang", "must-not-run"] {
                    let result = cancellable_tool_call(
                        &cancel,
                        client.call_tool(name, json!({}), Some("fixture".into()), Some(user)),
                    )
                    .await;
                    if result.is_none() {
                        break;
                    }
                    result.unwrap().unwrap();
                }
                next_tool_round(cancel.is_cancelled(), false, true)
            }
        });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        cancel.cancel(StopReason::UserCancelled);
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap(),
            ToolRoundOutcome::ReportCancel
        ));
        tokio::time::timeout(Duration::from_secs(1), cancelled_at_server.notified())
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::timeout(
            Duration::from_secs(1),
            client.call_tool("probe", json!({}), Some("fixture".into()), Some(user)),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        server.abort();
    }
}
