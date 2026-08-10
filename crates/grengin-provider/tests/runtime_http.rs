// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;

use futures_util::StreamExt;
use grengin_provider::{
    ChatRequest, DeclarativeProvider, EmbeddingRequest, ImageRequest, ModelId, ProviderError,
    ProviderEvent, ProviderManifestV1, ProviderPlugin, ProviderRuntimeConfig,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
};

struct CapturedRequest {
    head: String,
    body: Vec<u8>,
}

async fn serve_once(
    status: &str,
    content_type: &str,
    body: Vec<u8>,
) -> (String, oneshot::Receiver<CapturedRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = oneshot::channel();
    let status = status.to_string();
    let content_type = content_type.to_string();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let header_end = loop {
            let mut buffer = [0_u8; 1024];
            let read = socket.read(&mut buffer).await.unwrap();
            assert!(read > 0, "client closed before sending request headers");
            request.extend_from_slice(&buffer[..read]);
            if let Some(position) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let head = String::from_utf8(request[..header_end].to_vec()).unwrap();
        let content_length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        while request.len() - header_end < content_length {
            let mut buffer = [0_u8; 1024];
            let read = socket.read(&mut buffer).await.unwrap();
            assert!(read > 0, "client closed before sending request body");
            request.extend_from_slice(&buffer[..read]);
        }
        sender
            .send(CapturedRequest {
                head,
                body: request[header_end..header_end + content_length].to_vec(),
            })
            .ok();

        let response_head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        socket.write_all(response_head.as_bytes()).await.unwrap();
        let split = body.len() / 2;
        socket.write_all(&body[..split]).await.unwrap();
        tokio::task::yield_now().await;
        socket.write_all(&body[split..]).await.unwrap();
    });
    (format!("http://{address}/v1/"), receiver)
}

fn runtime(base_url: String) -> ProviderRuntimeConfig {
    ProviderRuntimeConfig {
        base_url_override: Some(base_url),
        credentials: BTreeMap::from([("api_key".to_string(), "super-secret".to_string())]),
        allow_insecure_http: true,
        allow_private_network: true,
        ..Default::default()
    }
}

fn manifest(base_url: &str, capabilities: Value, operations: Value) -> ProviderManifestV1 {
    let value = json!({
        "manifestVersion": "1.0",
        "id": "mock-provider",
        "version": "1.0.0",
        "name": "Mock provider",
        "baseUrl": base_url,
        "credentials": [{"id": "api_key", "type": "secret", "required": true}],
        "capabilities": capabilities,
        "operations": operations
    });
    ProviderManifestV1::from_json(&serde_json::to_vec(&value).unwrap()).unwrap()
}

#[tokio::test]
async fn sends_mapped_chat_request_and_decodes_sse() {
    let wire = concat!(
        "data: {\"delta\":\"hello\"}\n\n",
        "data: {\"delta\":\" world\"}\n\n",
        "data: [DONE]\n\n"
    );
    let (base_url, captured) = serve_once(
        "200 OK",
        "text/event-stream; charset=utf-8",
        wire.as_bytes().to_vec(),
    )
    .await;
    let provider = DeclarativeProvider::new(
        manifest(
            &base_url,
            json!({"chat": {"streaming": true}}),
            json!({
                "chatStream": {
                    "method": "POST",
                    "path": "chat/${request.model}",
                    "headers": {
                        "Authorization": {"secret": "api_key", "prefix": "Bearer "}
                    },
                    "query": {"stream": {"$literal": true}},
                    "bodyEncoding": "json",
                    "body": {
                        "model": {"$get": "request.model"},
                        "messages": {"$get": "request.messages"},
                        "stream": {"$literal": true}
                    },
                    "response": {
                        "bodyEncoding": "sse",
                        "eventDataEncoding": "json",
                        "doneData": "[DONE]",
                        "rules": [{"id": "text", "emit": "textDelta", "value": "/delta"}]
                    }
                }
            }),
        ),
        runtime(base_url),
    )
    .unwrap();

    let mut session = provider
        .chat()
        .unwrap()
        .start(ChatRequest {
            model: ModelId::new("test model"),
            messages: Vec::new(),
            temperature: None,
            max_tokens: None,
            tools: Vec::new(),
            tool_choice: None,
            options: Value::Null,
        })
        .await
        .unwrap();
    let mut stream = session.stream().await.unwrap();
    let mut text = String::new();
    let mut completed = false;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            ProviderEvent::TextDelta { text: delta } => text.push_str(&delta),
            ProviderEvent::Completed { .. } => completed = true,
            _ => {}
        }
    }

    assert_eq!(text, "hello world");
    assert!(completed);
    let captured = captured.await.unwrap();
    assert!(
        captured
            .head
            .starts_with("POST /v1/chat/test%20model?stream=true HTTP/1.1"),
        "unexpected request head: {}",
        captured.head
    );
    assert!(
        captured
            .head
            .to_ascii_lowercase()
            .contains("authorization: bearer super-secret")
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&captured.body).unwrap()["model"],
        "test model"
    );
    assert!(!format!("{:?}", provider_config_for_debug()).contains("debug-secret"));
}

#[tokio::test]
async fn rejects_chat_stream_without_completion_event() {
    let (base_url, _) = serve_once(
        "200 OK",
        "text/event-stream",
        b"data: {\"delta\":\"partial\"}\n\n".to_vec(),
    )
    .await;
    let provider = DeclarativeProvider::new(
        manifest(
            &base_url,
            json!({"chat": {"streaming": true}}),
            json!({
                "chatStream": {
                    "method": "POST",
                    "path": "chat",
                    "bodyEncoding": "json",
                    "body": {"messages": {"$get": "request.messages"}},
                    "response": {
                        "bodyEncoding": "sse",
                        "eventDataEncoding": "json",
                        "doneData": "[DONE]",
                        "rules": [{"id": "text", "emit": "textDelta", "value": "/delta"}]
                    }
                }
            }),
        ),
        runtime(base_url),
    )
    .unwrap();
    let mut session = provider
        .chat()
        .unwrap()
        .start(ChatRequest {
            model: ModelId::new("chat-1"),
            messages: Vec::new(),
            temperature: None,
            max_tokens: None,
            tools: Vec::new(),
            tool_choice: None,
            options: Value::Null,
        })
        .await
        .unwrap();
    let mut stream = session.stream().await.unwrap();
    assert!(matches!(
        stream.next().await.unwrap().unwrap(),
        ProviderEvent::TextDelta { .. }
    ));
    assert!(matches!(
        stream.next().await.unwrap().unwrap_err(),
        ProviderError::StreamEnded
    ));
}

fn provider_config_for_debug() -> ProviderRuntimeConfig {
    ProviderRuntimeConfig {
        credentials: BTreeMap::from([("api_key".to_string(), "debug-secret".to_string())]),
        ..Default::default()
    }
}

#[tokio::test]
async fn decodes_and_orders_embedding_vectors() {
    let response = json!({
        "data": [
            {"index": 1, "embedding": [3.0, 4.0]},
            {"index": 0, "embedding": [1.0, 2.0]}
        ],
        "usage": {"total_tokens": 9}
    });
    let (base_url, _) = serve_once(
        "200 OK",
        "application/json",
        serde_json::to_vec(&response).unwrap(),
    )
    .await;
    let provider = DeclarativeProvider::new(
        manifest(
            &base_url,
            json!({"embeddings": true}),
            json!({
                "embeddings": {
                    "method": "POST",
                    "path": "embeddings",
                    "bodyEncoding": "json",
                    "body": {
                        "model": {"$get": "request.model"},
                        "input": {"$get": "request.inputs"}
                    },
                    "response": {
                        "bodyEncoding": "json",
                        "itemsPointer": "/data",
                        "vectorPointer": "/embedding",
                        "indexPointer": "/index",
                        "usage": {"totalTokens": "/usage/total_tokens"}
                    }
                }
            }),
        ),
        runtime(base_url),
    )
    .unwrap();

    let result = provider
        .embeddings()
        .unwrap()
        .embed(EmbeddingRequest {
            model: ModelId::new("embed-1"),
            inputs: vec!["a".to_string(), "b".to_string()],
            dimensions: None,
            options: Value::Null,
        })
        .await
        .unwrap();
    assert_eq!(result.vectors, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
    assert_eq!(result.usage.unwrap().total_tokens, Some(9));
}

#[tokio::test]
async fn accepts_direct_binary_image_responses() {
    let (base_url, _) = serve_once("200 OK", "image/webp", vec![1, 2, 3, 4]).await;
    let provider = DeclarativeProvider::new(
        manifest(
            &base_url,
            json!({"imageGeneration": true}),
            json!({
                "imageGeneration": {
                    "method": "POST",
                    "path": "images",
                    "bodyEncoding": "json",
                    "body": {"prompt": {"$get": "request.prompt"}},
                    "response": {"bodyEncoding": "binary"}
                }
            }),
        ),
        runtime(base_url),
    )
    .unwrap();

    let result = provider
        .images()
        .unwrap()
        .generate(ImageRequest {
            model: ModelId::new("image-1"),
            prompt: "a test".to_string(),
            input_images: Vec::new(),
            count: 1,
            size: None,
            quality: None,
            options: Value::Null,
        })
        .await
        .unwrap();
    assert_eq!(result.images[0].bytes, vec![1, 2, 3, 4]);
    assert_eq!(result.images[0].media_type, "image/webp");
}

#[tokio::test]
async fn maps_rate_limits_without_leaking_credentials() {
    let (base_url, _) = serve_once(
        "429 Too Many Requests",
        "application/json",
        br#"{"error":"slow down"}"#.to_vec(),
    )
    .await;
    let provider = DeclarativeProvider::new(
        manifest(
            &base_url,
            json!({"embeddings": true}),
            json!({
                "embeddings": {
                    "method": "POST",
                    "path": "embeddings",
                    "headers": {"Authorization": {"secret": "api_key"}},
                    "bodyEncoding": "json",
                    "body": {"input": {"$get": "request.inputs"}},
                    "response": {"itemsPointer": "/data", "vectorPointer": "/embedding"}
                }
            }),
        ),
        runtime(base_url),
    )
    .unwrap();
    let error = provider
        .embeddings()
        .unwrap()
        .embed(EmbeddingRequest {
            model: ModelId::new("embed-1"),
            inputs: vec!["a".to_string()],
            dimensions: None,
            options: Value::Null,
        })
        .await
        .unwrap_err();
    assert!(matches!(error, ProviderError::QuotaExhausted(_)));
    assert!(!error.to_string().contains("super-secret"));
}
