// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use axum::{
    Router,
    body::Body,
    http::{Response, StatusCode, header},
};
use connector_server_plugin::{
    CallContext, CompiledConnector, ConnectorError, ConnectorPluginConfig,
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, sync::Mutex, task::JoinHandle};

static TESTS: Mutex<()> = Mutex::const_new(());

fn manifest(base: &str, response: Value) -> Value {
    json!({
        "schema_version":1,"id":"content-test","name":"Content test","version":"1.0.0",
        "base_url":base,"auth":"oauth2",
        "tools":[{"name":"read","description":"Read bounded content","read_only":true,
            "input_schema":{"type":"object","additionalProperties":false},
            "request":{"method":"GET","path":"/content"},"response":response}]
    })
}

fn compile(base: &str, response: Value) -> Result<CompiledConnector, ConnectorError> {
    CompiledConnector::compile(ConnectorPluginConfig {
        manifest: manifest(base, response),
        allow_insecure_http: true,
        allow_private_network: true,
        ..Default::default()
    })
}

async fn serve(mime: &str, body: Vec<u8>, status: StatusCode) -> (String, JoinHandle<()>) {
    let mime = mime.to_owned();
    let router = Router::new().fallback(move || {
        let body = body.clone();
        let mime = mime.clone();
        async move {
            Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, mime)
                .body(Body::from(body))
                .unwrap()
        }
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (origin, task)
}

async fn call(base: &str, policy: Value) -> Result<Value, ConnectorError> {
    compile(base, policy)?
        .call(
            "read",
            json!({}),
            CallContext {
                user_id: "alice",
                access_token: Some("private-token-canary"),
            },
        )
        .await
}

#[tokio::test]
async fn plain_text_csv_markdown_and_vtt_are_readable_and_clipped_on_utf8_boundaries() {
    let _guard = TESTS.lock().await;
    for mime in [
        "text/plain; charset=utf-8",
        "text/csv",
        "text/markdown",
        "text/vtt",
    ] {
        let body = "Water 水🙂\n".repeat(1000);
        let (base, task) = serve(mime, body.into_bytes(), StatusCode::OK).await;
        let result = call(
            &base,
            json!({"format":"text","overflow":"truncate","max_result_bytes":512}),
        )
        .await
        .unwrap();
        task.abort();
        let _ = task.await;
        assert_eq!(result["truncated"], true);
        assert_eq!(result["source_complete"], false);
        assert!(result["content"].as_str().unwrap().starts_with("Water"));
        assert!(serde_json::to_vec(&result).unwrap().len() <= 512);
    }
}

#[tokio::test]
async fn large_json_is_parsed_completely_then_truncated_as_valid_structured_content() {
    let _guard = TESTS.lock().await;
    let body =
        serde_json::to_vec(&json!({"body":"context ".repeat(200000),"more":[1,2,3]})).unwrap();
    assert!(body.len() > 1024 * 1024);
    let (base, task) = serve("application/json", body, StatusCode::OK).await;
    let result = call(
        &base,
        json!({"overflow":"truncate","max_source_bytes":4*1024*1024,"max_result_bytes":1024,
                                   "result":{"$get":"session.response"}}),
    )
    .await
    .unwrap();
    task.abort();
    let _ = task.await;
    assert_eq!(result["truncated"], true);
    assert_eq!(result["source_complete"], true);
    assert!(result["content"].is_object());
    assert!(serde_json::to_vec(&result).unwrap().len() <= 1024);
}

#[tokio::test]
async fn json_is_never_byte_sliced_and_legacy_json_tools_remain_strict() {
    let _guard = TESTS.lock().await;
    for (mime, body, policy, too_large) in [
        ("text/plain", b"not-json".to_vec(), json!({}), false),
        (
            "application/json",
            b"{\"incomplete\":".to_vec(),
            json!({"overflow":"truncate"}),
            false,
        ),
        (
            "application/json",
            serde_json::to_vec(&"x".repeat(1024 * 1024 + 1)).unwrap(),
            json!({}),
            true,
        ),
        (
            "application/json",
            serde_json::to_vec(&"x".repeat(3000)).unwrap(),
            json!({"overflow":"truncate","max_source_bytes":2048}),
            true,
        ),
    ] {
        let (base, task) = serve(mime, body, StatusCode::OK).await;
        let error = call(&base, policy).await.unwrap_err();
        task.abort();
        let _ = task.await;
        assert!(if too_large {
            matches!(error, ConnectorError::ResponseTooLarge)
        } else {
            matches!(error, ConnectorError::Response)
        });
    }
}

#[tokio::test]
async fn content_mode_rejects_binary_invalid_utf8_and_does_not_hide_provider_errors() {
    let _guard = TESTS.lock().await;
    for (mime, body, status) in [
        ("application/pdf", b"%PDF-ascii".to_vec(), StatusCode::OK),
        (
            "application/octet-stream",
            b"binary".to_vec(),
            StatusCode::OK,
        ),
        ("text/plain", vec![0xff, 0xfe], StatusCode::OK),
        (
            "text/plain",
            b"private provider canary".to_vec(),
            StatusCode::UNAUTHORIZED,
        ),
    ] {
        let (base, task) = serve(mime, body, status).await;
        let error = call(&base, json!({"format":"text","overflow":"truncate"}))
            .await
            .unwrap_err();
        task.abort();
        let _ = task.await;
        assert!(!error.to_string().contains("private"));
        assert!(matches!(
            error,
            ConnectorError::Response | ConnectorError::Http { status: 401, .. }
        ));
    }
}

#[tokio::test]
async fn content_projection_decodes_provider_base64_without_exposing_encoded_data() {
    let _guard = TESTS.lock().await;
    let (base, task) = serve(
        "application/json",
        br#"{"content":"SGVsbG8sIGNhZsOp"}"#.to_vec(),
        StatusCode::OK,
    )
    .await;
    let result = call(
        &base,
        json!({"overflow":"truncate",
            "result":{"text":{"$base64Decode":{"$get":"session.response.content"}}}
        }),
    )
    .await
    .unwrap();
    task.abort();
    let _ = task.await;
    assert_eq!(result["content"]["text"], "Hello, café");
    assert_eq!(result["truncated"], false);
}

#[tokio::test]
async fn path_arrays_encode_each_segment_and_query_arrays_preserve_repeated_keys() {
    use axum::extract::OriginalUri;
    let _guard = TESTS.lock().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().fallback(|OriginalUri(uri): OriginalUri| async move {
                axum::Json(json!({"path":uri.path(),"query":uri.query()}))
            }),
        )
        .await
        .unwrap()
    });
    let mut document = manifest(&base, json!({}));
    document["tools"][0]["input_schema"] = json!({"type":"object","required":["path","ranges"],
        "properties":{"path":{"type":"array","items":{"type":"string"}},
                      "ranges":{"type":"array","items":{"type":"string"}}},"additionalProperties":false});
    document["tools"][0]["request"] = json!({"method":"GET","path":"/repos/{path}/",
        "path_parameters":{"path":{"$get":"request.path"}},
        "query":{"ranges":{"$get":"request.ranges"}}});
    let plugin = CompiledConnector::compile(ConnectorPluginConfig {
        manifest: document,
        allow_insecure_http: true,
        allow_private_network: true,
        ..Default::default()
    })
    .unwrap();
    let context = CallContext {
        user_id: "alice",
        access_token: Some("fixture"),
    };
    let result = plugin
        .call(
            "read",
            json!({"path":["src","my file.rs"],"ranges":["A1:B2","Other!A1:C3","A1:B2"]}),
            context,
        )
        .await
        .unwrap();
    assert_eq!(result["path"], "/repos/src/my%20file.rs/");
    let pairs: Vec<_> = url::form_urlencoded::parse(result["query"].as_str().unwrap().as_bytes())
        .into_owned()
        .collect();
    assert_eq!(
        pairs,
        [
            ("ranges".to_owned(), "A1:B2".to_owned()),
            ("ranges".to_owned(), "Other!A1:C3".to_owned()),
            ("ranges".to_owned(), "A1:B2".to_owned())
        ]
    );
    for path in [
        json!([]),
        json!(vec!["a"; 33]),
        json!([".."]),
        json!(["."]),
        json!([""]),
        json!(["a/b"]),
        json!(["a\\b"]),
        json!(["%2e%2e"]),
        json!(["?token=value"]),
        json!(["#fragment"]),
        json!(["\n"]),
    ] {
        assert!(matches!(
            plugin
                .call(
                    "read",
                    json!({"path":path,"ranges":["A1"]}),
                    CallContext {
                        user_id: "alice",
                        access_token: Some("fixture")
                    }
                )
                .await,
            Err(ConnectorError::Arguments)
        ));
    }
    assert!(matches!(
        plugin
            .call(
                "read",
                json!({"path":["src"],"ranges":vec!["A1";101]}),
                CallContext {
                    user_id: "alice",
                    access_token: Some("fixture")
                }
            )
            .await,
        Err(ConnectorError::Arguments)
    ));
    server.abort();
    let _ = server.await;
}

#[test]
fn content_policies_cannot_disable_resource_bounds_or_mix_text_with_json_pagination() {
    for policy in [
        json!({"format":"binary"}),
        json!({"max_source_bytes":9*1024*1024}),
        json!({"max_result_bytes":2*1024*1024}),
        json!({"max_result_bytes":127}),
        json!({"format":"text","error_if":true}),
        json!({"format":"text","pagination":{"items_pointer":"/items","next_pointer":"/next","max_pages":2}}),
    ] {
        assert!(compile("https://example.com", policy).is_err());
    }
}

#[tokio::test]
async fn unknown_length_chunked_responses_obey_the_same_source_and_text_limits() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _guard = TESTS.lock().await;
    for text in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 8192];
            let mut read = 0;
            while read < request.len() {
                let size = socket.read(&mut request[read..]).await.unwrap();
                if size == 0 {
                    return;
                }
                read += size;
                if request[..read].windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    break;
                }
            }
            let mime = if text {
                "text/plain"
            } else {
                "application/json"
            };
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            for chunk in [
                format!("\"{}", "x".repeat(400)),
                "y".repeat(400),
                "\"".into(),
            ] {
                if socket
                    .write_all(format!("{:x}\r\n{chunk}\r\n", chunk.len()).as_bytes())
                    .await
                    .is_err()
                {
                    return;
                }
            }
            let _ = socket.write_all(b"0\r\n\r\n").await;
        });
        let result = call(&base, json!({"format":if text {"text"}else{"json"}, "overflow":"truncate","max_source_bytes":512,"max_result_bytes":512})).await;
        task.await.unwrap();
        if text {
            let result = result.unwrap();
            assert_eq!(result["truncated"], true);
            assert_eq!(result["source_complete"], false);
        } else {
            assert!(matches!(result, Err(ConnectorError::ResponseTooLarge)));
        }
    }
}

#[tokio::test]
async fn text_prefix_does_not_wait_for_the_next_chunk_after_filling_its_budget() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _guard = TESTS.lock().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 8192];
        let _ = socket.read(&mut request).await.unwrap();
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\r\n200\r\n").await.unwrap();
        socket.write_all(&vec![b'x'; 512]).await.unwrap();
        socket.write_all(b"\r\n").await.unwrap();
        std::future::pending::<()>().await;
    });
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        call(
            &base,
            json!({"format":"text","overflow":"truncate","max_result_bytes":512}),
        ),
    )
    .await;
    server.abort();
    let _ = server.await;
    let result = result
        .expect("a full prefix must not wait for the server to close")
        .unwrap();
    assert_eq!(result["source_complete"], false);
    assert_eq!(result["truncated"], true);
    assert!(serde_json::to_vec(&result).unwrap().len() <= 512);
}

#[tokio::test]
async fn content_concurrency_is_bounded_across_connector_instances_and_releases_permits() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::{Notify, Semaphore};
    let _guard = TESTS.lock().await;
    let entered = Arc::new(AtomicUsize::new(0));
    let changed = Arc::new(Notify::new());
    let release = Arc::new(Semaphore::new(0));
    let router = Router::new().fallback({
        let entered = entered.clone();
        let changed = changed.clone();
        let release = release.clone();
        move || {
            let entered = entered.clone();
            let changed = changed.clone();
            let release = release.clone();
            async move {
                entered.fetch_add(1, Ordering::SeqCst);
                changed.notify_waiters();
                let _permit = release.acquire().await.unwrap();
                ([(header::CONTENT_TYPE, "text/plain")], "complete")
            }
        }
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let plugin = compile(&base, json!({"format":"text"})).unwrap();
        tasks.push(tokio::spawn(async move {
            plugin
                .call(
                    "read",
                    json!({}),
                    CallContext {
                        user_id: "alice",
                        access_token: Some("fixture"),
                    },
                )
                .await
        }));
    }
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let next = changed.notified();
            if entered.load(Ordering::SeqCst) == 2 {
                break;
            }
            next.await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        call(&base, json!({"format":"text"})).await,
        Err(ConnectorError::Busy)
    ));
    release.add_permits(2);
    for task in tasks {
        assert_eq!(task.await.unwrap().unwrap(), "complete");
    }
    assert_eq!(
        call(&base, json!({"format":"text"})).await.unwrap(),
        "complete"
    );
    server.abort();
    let _ = server.await;
}
