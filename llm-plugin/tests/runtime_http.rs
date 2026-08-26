// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use futures_util::StreamExt;
use llm_plugin::{
    ChatMessage, ChatRequest, ChatRole, ContentPart, DeclarativeProvider, EmbeddingRequest,
    ImageRequest, ModelId, ProviderError, ProviderEvent, ProviderEventStream, ProviderManifestV1,
    ProviderModelType, ProviderPlugin, ProviderRuntimeConfig, ToolCallId, ToolResult,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
};

struct CapturedRequest {
    head: String,
    body: Vec<u8>,
}

impl CapturedRequest {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

/// One scripted HTTP response. The body is written in `chunks`, separated by `gap`, so tests can
/// exercise how the runtime treats a stream that stays open across several reads.
struct MockResponse {
    status: String,
    content_type: String,
    chunks: Vec<Vec<u8>>,
    gap: Duration,
}

impl MockResponse {
    /// Answers with `body` split across two writes, which keeps every test exercising the
    /// incremental decode path rather than a single convenient chunk.
    fn new(status: &str, content_type: &str, body: impl Into<Vec<u8>>) -> Self {
        let body = body.into();
        let split = body.len() / 2;
        Self::chunked(
            status,
            content_type,
            vec![body[..split].to_vec(), body[split..].to_vec()],
            Duration::ZERO,
        )
    }

    fn json(status: &str, body: &Value) -> Self {
        Self::new(
            status,
            "application/json",
            serde_json::to_vec(body).unwrap(),
        )
    }

    fn chunked(status: &str, content_type: &str, chunks: Vec<Vec<u8>>, gap: Duration) -> Self {
        Self {
            status: status.to_string(),
            content_type: content_type.to_string(),
            chunks,
            gap,
        }
    }
}

type Requests = Arc<Mutex<Vec<CapturedRequest>>>;

/// Serves `responses` in order, one per accepted connection, recording every request received.
async fn serve(responses: Vec<MockResponse>) -> (String, Requests) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    tokio::spawn(async move {
        for response in responses {
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
            recorded.lock().await.push(CapturedRequest {
                head,
                body: request[header_end..header_end + content_length].to_vec(),
            });

            let length = response.chunks.iter().map(Vec::len).sum::<usize>();
            let response_head = format!(
                "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n",
                response.status, response.content_type
            );
            socket.write_all(response_head.as_bytes()).await.unwrap();
            for (index, chunk) in response.chunks.iter().enumerate() {
                if index > 0 && !response.gap.is_zero() {
                    tokio::time::sleep(response.gap).await;
                }
                socket.write_all(chunk).await.unwrap();
                socket.flush().await.unwrap();
                tokio::task::yield_now().await;
            }
        }
    });
    (format!("http://{address}/v1/"), requests)
}

async fn serve_once(status: &str, content_type: &str, body: Vec<u8>) -> (String, Requests) {
    serve(vec![MockResponse::new(status, content_type, body)]).await
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

fn manifest_value(base_url: &str, capabilities: Value, operations: Value) -> Value {
    json!({
        "manifestVersion": "1.0",
        "id": "mock-provider",
        "version": "1.0",
        "name": "Mock provider",
        "baseUrl": base_url,
        "credentials": [{"id": "api_key", "type": "secret", "required": true}],
        "capabilities": capabilities,
        "operations": operations
    })
}

fn manifest(base_url: &str, capabilities: Value, operations: Value) -> ProviderManifestV1 {
    let value = manifest_value(base_url, capabilities, operations);
    ProviderManifestV1::from_json(&serde_json::to_vec(&value).unwrap()).unwrap()
}

fn stub_chat_operation() -> Value {
    json!({
        "method": "POST",
        "path": "chat",
        "bodyEncoding": "json",
        "body": {},
        "response": {
            "bodyEncoding": "sse",
            "eventDataEncoding": "json",
            "doneData": "[DONE]",
            "rules": [{
                "id": "completed",
                "when": {"pointer": "/done", "exists": true},
                "emit": "completed"
            }]
        }
    })
}

fn stub_embeddings_operation() -> Value {
    json!({
        "method": "POST",
        "path": "embeddings",
        "bodyEncoding": "json",
        "body": {},
        "response": {
            "bodyEncoding": "json",
            "itemsPointer": "/data",
            "vectorPointer": "/embedding"
        }
    })
}

fn provider(base_url: String, capabilities: Value, operations: Value) -> DeclarativeProvider {
    let manifest = manifest(&base_url, capabilities, operations);
    DeclarativeProvider::new(manifest, runtime(base_url)).unwrap()
}

fn chat_request(model: &str) -> ChatRequest {
    ChatRequest {
        model: ModelId::new(model),
        messages: vec![ChatMessage {
            role: ChatRole::User,
            content: vec![ContentPart::Text {
                text: "hello".to_string(),
            }],
            tool_calls: Vec::new(),
            tool_result: None,
        }],
        temperature: None,
        max_tokens: None,
        tools: Vec::new(),
        tool_choice: None,
        web_search: false,
        options: Value::Null,
    }
}

fn embedding_request() -> EmbeddingRequest {
    EmbeddingRequest {
        model: ModelId::new("embed-1"),
        inputs: vec!["a".to_string()],
        dimensions: None,
        options: Value::Null,
    }
}

fn image_request(count: u8) -> ImageRequest {
    ImageRequest {
        model: ModelId::new("image-1"),
        prompt: "a test".to_string(),
        input_images: Vec::new(),
        count,
        size: None,
        quality: None,
        options: Value::Null,
    }
}

/// Unwraps the error from a stream that was expected to fail. `ProviderEventStream` is not
/// `Debug`, so `Result::unwrap_err` cannot be used directly.
fn stream_error(result: Result<ProviderEventStream, ProviderError>) -> ProviderError {
    match result {
        Ok(_) => panic!("expected the stream to fail"),
        Err(error) => error,
    }
}

/// Drains a chat stream, returning every event or the first error.
async fn collect_chat(provider: &DeclarativeProvider, model: &str) -> Vec<ProviderEvent> {
    let mut session = provider
        .chat()
        .unwrap()
        .start(chat_request(model))
        .await
        .unwrap();
    let mut stream = session.stream().await.unwrap();
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.unwrap());
    }
    events
}

#[tokio::test]
async fn sends_mapped_chat_request_and_decodes_sse() {
    let wire = concat!(
        "data: {\"delta\":\"hello\"}\n\n",
        "data: {\"delta\":\" world\"}\n\n",
        "data: [DONE]\n\n"
    );
    let (base_url, requests) = serve_once(
        "200 OK",
        "text/event-stream; charset=utf-8",
        wire.as_bytes().to_vec(),
    )
    .await;
    let provider = provider(
        base_url,
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
    );

    let events = collect_chat(&provider, "test model").await;
    let text = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(text, "hello world");
    assert!(matches!(
        events.last(),
        Some(ProviderEvent::Completed { .. })
    ));

    let requests = requests.lock().await;
    assert!(
        requests[0]
            .head
            .starts_with("POST /v1/chat/test%20model?stream=true HTTP/1.1"),
        "unexpected request head: {}",
        requests[0].head
    );
    assert!(
        requests[0]
            .head
            .to_ascii_lowercase()
            .contains("authorization: bearer super-secret")
    );
    assert_eq!(requests[0].json()["model"], "test model");
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
    let provider = provider(
        base_url,
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
    );
    let mut session = provider
        .chat()
        .unwrap()
        .start(chat_request("chat-1"))
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

/// An OpenAI-compatible chat operation whose response spec matches the reference manifest.
fn openai_chat_operations(extra: Value) -> Value {
    let mut operation = json!({
        "method": "POST",
        "path": "chat/completions",
        "bodyEncoding": "json",
        "body": {
            "model": {"$get": "request.model"},
            "messages": {"$get": "request.messages"}
        },
        "response": {
            "bodyEncoding": "sse",
            "eventDataEncoding": "json",
            "doneData": "[DONE]",
            "rules": [
                {
                    "id": "text",
                    "when": {"pointer": "/choices/0/delta/content", "notNull": true},
                    "emit": "textDelta",
                    "value": "/choices/0/delta/content"
                },
                {
                    "id": "tool_start",
                    "forEach": "/choices/0/delta/tool_calls",
                    "when": {"pointer": "/id", "notNull": true},
                    "emit": "toolCallStart",
                    "fields": {"id": "/id", "name": "/function/name", "index": "/index"}
                },
                {
                    "id": "tool_arguments",
                    "forEach": "/choices/0/delta/tool_calls",
                    "when": {"pointer": "/function/arguments", "notNull": true},
                    "emit": "toolArgumentsDelta",
                    "fields": {"index": "/index", "fragment": "/function/arguments"}
                },
                {
                    "id": "completed",
                    "when": {"pointer": "/choices/0/finish_reason", "notNull": true},
                    "emit": "completed",
                    "fields": {"finishReason": "/choices/0/finish_reason"}
                }
            ]
        }
    });
    if let Value::Object(extra) = extra {
        operation.as_object_mut().unwrap().extend(extra);
    }
    json!({"chatStream": operation})
}

#[tokio::test]
async fn emits_one_completion_for_a_finish_reason_followed_by_done() {
    let wire = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let (base_url, _) = serve_once("200 OK", "text/event-stream", wire.as_bytes().to_vec()).await;
    let provider = provider(
        base_url,
        json!({"chat": {"streaming": true}}),
        openai_chat_operations(Value::Null),
    );

    let events = collect_chat(&provider, "chat-1").await;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ProviderEvent::Completed { .. }))
            .count(),
        1,
        "expected exactly one completion in {events:?}"
    );
}

#[tokio::test]
async fn skips_the_null_content_that_accompanies_tool_call_chunks() {
    // Real OpenAI streams send `"content": null` alongside tool calls; reading a text delta out of
    // that null used to abort the whole stream.
    let wire = concat!(
        "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":null}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n"
    );
    let (base_url, _) = serve_once("200 OK", "text/event-stream", wire.as_bytes().to_vec()).await;
    let provider = provider(
        base_url,
        json!({"chat": {"streaming": true}}),
        openai_chat_operations(Value::Null),
    );

    let events = collect_chat(&provider, "chat-1").await;
    assert!(matches!(
        events.as_slice(),
        [
            ProviderEvent::TextDelta { text },
            ProviderEvent::Completed { .. }
        ] if text == "ok"
    ));
}

#[tokio::test]
async fn replays_tool_calls_and_results_on_continuation() {
    let first = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"lookup\",\"arguments\":\"{\\\"q\\\":\"}}]}}]}\n\n",
        // The repeated id mirrors providers that echo it on every fragment.
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"function\":{\"arguments\":\"\\\"rust\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let second =
        "data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":\"stop\"}]}\n\n";
    let (base_url, requests) = serve(vec![
        MockResponse::new("200 OK", "text/event-stream", first.as_bytes().to_vec()),
        MockResponse::new("200 OK", "text/event-stream", second.as_bytes().to_vec()),
    ])
    .await;
    let provider = provider(
        base_url,
        json!({"chat": {"streaming": true, "tools": true}}),
        openai_chat_operations(json!({"continuation": {"maxToolRounds": 2}})),
    );

    let mut session = provider
        .chat()
        .unwrap()
        .start(chat_request("chat-1"))
        .await
        .unwrap();
    let mut stream = session.stream().await.unwrap();
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.unwrap());
    }
    drop(stream);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ProviderEvent::ToolCallStart { .. }))
            .count(),
        1,
        "a repeated tool id must not restart the call: {events:?}"
    );

    let mut stream = session
        .continue_with_tools(vec![ToolResult {
            call_id: ToolCallId::new("call-1"),
            name: "lookup".to_string(),
            output: json!({"answer": 42}),
            is_error: false,
        }])
        .await
        .unwrap();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }

    let requests = requests.lock().await;
    let replayed = requests[1].json();
    let messages = replayed["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3, "unexpected replay: {replayed}");
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["toolCalls"][0]["name"], "lookup");
    // Argument fragments are reassembled and parsed, not forwarded as a raw string.
    assert_eq!(messages[1]["toolCalls"][0]["arguments"]["q"], "rust");
    assert_eq!(messages[2]["role"], "tool");
    assert_eq!(messages[2]["toolResult"]["callId"], "call-1");
}

#[tokio::test]
async fn replays_assistant_narration_alongside_the_tool_call_it_preceded() {
    let first = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Let me look that up.\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"lookup\",\"arguments\":\"{}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n"
    );
    let second =
        "data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":\"stop\"}]}\n\n";
    let (base_url, requests) = serve(vec![
        MockResponse::new("200 OK", "text/event-stream", first.as_bytes().to_vec()),
        MockResponse::new("200 OK", "text/event-stream", second.as_bytes().to_vec()),
    ])
    .await;
    let provider = provider(
        base_url,
        json!({"chat": {"streaming": true, "tools": true}}),
        openai_chat_operations(Value::Null),
    );

    let mut session = provider
        .chat()
        .unwrap()
        .start(chat_request("chat-1"))
        .await
        .unwrap();
    let mut stream = session.stream().await.unwrap();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    drop(stream);
    let mut stream = session
        .continue_with_tools(vec![ToolResult {
            call_id: ToolCallId::new("call-1"),
            name: "lookup".to_string(),
            output: json!("ok"),
            is_error: false,
        }])
        .await
        .unwrap();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }

    let requests = requests.lock().await;
    let messages = requests[1].json();
    // Providers that validate history reject an assistant turn whose text was dropped, and the
    // model loses its own reasoning-in-prose if the narration disappears.
    assert_eq!(
        messages["messages"][1]["content"][0]["text"], "Let me look that up.",
        "narration missing from replayed turn: {messages}"
    );
    assert_eq!(messages["messages"][1]["toolCalls"][0]["id"], "call-1");
}

/// End-to-end web search over the real Anthropic reference manifest: a server-side search and a
/// client tool interleaved in one content-block index space, both streaming identical
/// `input_json_delta` payloads.
#[tokio::test]
async fn maps_interleaved_web_search_and_client_tool_blocks_from_one_stream() {
    let wire = concat!(
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srvtoolu_1\",\"name\":\"web_search\"}}\n\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"query\\\":\\\"rust\\\"}\"}}\n\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"web_search_tool_result\",\"tool_use_id\":\"srvtoolu_1\",\"content\":[{\"type\":\"web_search_result\",\"title\":\"Releases\",\"url\":\"https://releases.rs/\",\"page_age\":\"June 2, 2026\",\"encrypted_content\":\"SECRETBLOB\"}]}}\n\n",
        "data: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"lookup\"}}\n\n",
        "data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"city\\\":\\\"Paris\\\"}\"}}\n\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n"
    );
    let (base_url, _) = serve_once("200 OK", "text/event-stream", wire.as_bytes().to_vec()).await;
    let mut manifest: Value =
        serde_json::from_slice(include_bytes!("../examples/anthropic.provider.json")).unwrap();
    manifest["baseUrl"] = json!(&base_url);
    let manifest = ProviderManifestV1::from_json(&serde_json::to_vec(&manifest).unwrap()).unwrap();
    let provider = DeclarativeProvider::new(manifest, runtime(base_url)).unwrap();

    let mut session = provider
        .chat()
        .unwrap()
        .start(chat_request("claude"))
        .await
        .unwrap();
    let mut stream = session.stream().await.unwrap();
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.unwrap());
    }

    // The search reports as a server tool, never as something the caller must execute.
    assert!(matches!(
        events.first(),
        Some(ProviderEvent::ServerToolStart { name, .. }) if name == "web_search"
    ));
    assert!(events.iter().any(|event| matches!(
        event,
        ProviderEvent::ServerToolQueryDelta { fragment, .. } if fragment.contains("rust")
    )));
    let results = events
        .iter()
        .find_map(|event| match event {
            ProviderEvent::ServerToolResult { results, .. } => Some(results),
            _ => None,
        })
        .expect("no grouped search result in {events:?}");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].url, "https://releases.rs/");
    assert_eq!(results[0].title, "Releases");

    // The client tool in the same index space still arrives intact and executable.
    assert!(events.iter().any(|event| matches!(
        event,
        ProviderEvent::ToolCallStart { name, .. } if name == "lookup"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        ProviderEvent::ToolArgumentsDelta { id, fragment } if id.as_str() == "toolu_1" && fragment.contains("Paris")
    )));
    // Exactly one client tool call: the search block must not leak into the client tool space.
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ProviderEvent::ToolCallStart { .. }))
            .count(),
        1,
        "{events:?}"
    );
    // Provider-internal payloads stay out of the caller's events entirely.
    let encoded = serde_json::to_string(&events).unwrap();
    assert!(!encoded.contains("SECRETBLOB"), "{encoded}");
}

#[tokio::test]
async fn completes_a_long_stream_when_gaps_stay_within_the_operation_timeout() {
    // The operation timeout has to bound silence between chunks, not the whole exchange: a model
    // that talks for longer than `timeoutMs` in total must still finish.
    let chunks = vec![
        b"data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n".to_vec(),
        b"data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n\n".to_vec(),
        b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n".to_vec(),
    ];
    let (base_url, _) = serve(vec![MockResponse::chunked(
        "200 OK",
        "text/event-stream",
        chunks,
        Duration::from_millis(300),
    )])
    .await;
    let provider = provider(
        base_url,
        json!({"chat": {"streaming": true}}),
        openai_chat_operations(json!({"timeoutMs": 500})),
    );

    let events = collect_chat(&provider, "chat-1").await;
    let text = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(text, "ab");
    assert!(matches!(
        events.last(),
        Some(ProviderEvent::Completed { .. })
    ));
}

#[tokio::test]
async fn rejects_chat_responses_that_are_not_event_streams() {
    let (base_url, _) =
        serve_once("200 OK", "application/json", br#"{"text":"hi"}"#.to_vec()).await;
    let provider = provider(
        base_url,
        json!({"chat": {"streaming": true}}),
        openai_chat_operations(Value::Null),
    );
    let mut session = provider
        .chat()
        .unwrap()
        .start(chat_request("chat-1"))
        .await
        .unwrap();
    let error = stream_error(session.stream().await);
    assert!(
        error.to_string().contains("text/event-stream"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn rejects_operation_paths_that_climb_out_of_the_base_url() {
    // Nothing is served: the traversal must be refused before a connection is attempted.
    let base_url = "http://127.0.0.1:1/v1/".to_string();
    let provider = provider(
        base_url,
        json!({"chat": {"streaming": true}}),
        openai_chat_operations(json!({"path": "${request.model}/completions"})),
    );
    let mut session = provider
        .chat()
        .unwrap()
        .start(chat_request(".."))
        .await
        .unwrap();
    assert!(matches!(
        stream_error(session.stream().await),
        ProviderError::UrlNotAllowed(_)
    ));

    // A model id that stays inside the base path is still accepted (and fails on connect instead).
    let mut session = provider
        .chat()
        .unwrap()
        .start(chat_request("gpt-4.1"))
        .await
        .unwrap();
    assert!(matches!(
        stream_error(session.stream().await),
        ProviderError::Transport(_)
    ));
}

#[tokio::test]
async fn rejects_manifest_paths_with_percent_encoded_parent_segments() {
    for path in ["chat/%2e%2e/admin", "%2E%2E/admin", "../admin", "chat#frag"] {
        let value = manifest_value(
            "https://api.example.com/v1/",
            json!({"chat": {"streaming": true}}),
            openai_chat_operations(json!({"path": path})),
        );
        assert!(
            ProviderManifestV1::from_json(&serde_json::to_vec(&value).unwrap()).is_err(),
            "{path} should be rejected"
        );
    }
}

#[tokio::test]
async fn rejects_manifests_whose_header_spec_has_an_unknown_field() {
    // A misspelled `prefix` silently sent a bare credential instead of `Bearer <key>`.
    let value = manifest_value(
        "https://api.example.com/v1/",
        json!({"chat": {"streaming": true}}),
        openai_chat_operations(json!({
            "headers": {"Authorization": {"secret": "api_key", "prefx": "Bearer "}}
        })),
    );
    assert!(ProviderManifestV1::from_json(&serde_json::to_vec(&value).unwrap()).is_err());

    let value = manifest_value(
        "https://api.example.com/v1/",
        json!({"chat": {"streaming": true}}),
        openai_chat_operations(json!({
            "headers": {"Authorization": {"secret": "api_key", "prefix": "Bearer "}}
        })),
    );
    ProviderManifestV1::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
}

#[tokio::test]
async fn decodes_and_orders_embedding_vectors() {
    let response = json!({
        "data": [
            {"index": 1, "embedding": [3.0, 4.0]},
            {"index": 0, "embedding": [1.0, 2.0]}
        ],
        "usage": {
            "input_tokens": 12,
            "output_tokens": 7,
            "total_tokens": 19,
            "cached_input_tokens": 5,
            "cache_creation_tokens": 3
        }
    });
    let (base_url, _) = serve(vec![MockResponse::json("200 OK", &response)]).await;
    let provider = provider(
        base_url,
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
                    "usage": {
                        "inputTokens": "/usage/input_tokens",
                        "outputTokens": "/usage/output_tokens",
                        "totalTokens": "/usage/total_tokens",
                        "cachedInputTokens": "/usage/cached_input_tokens",
                        "cacheCreationTokens": "/usage/cache_creation_tokens",
                        "inputTokensIncludeCached": false,
                        "inputTokensIncludeCacheCreation": false
                    }
                }
            }
        }),
    );

    let result = provider
        .embeddings()
        .unwrap()
        .embed(EmbeddingRequest {
            inputs: vec!["a".to_string(), "b".to_string()],
            ..embedding_request()
        })
        .await
        .unwrap();
    assert_eq!(result.vectors, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
    let usage = result.usage.unwrap();
    assert_eq!(usage.input_tokens, Some(20));
    assert_eq!(usage.output_tokens, Some(7));
    assert_eq!(usage.total_tokens, Some(27));
    assert_eq!(usage.cached_input_tokens, Some(5));
    assert_eq!(usage.cache_creation_tokens, Some(3));
}

#[tokio::test]
async fn sends_query_parameters_and_form_encoded_bodies() {
    let response = json!({"data": [{"embedding": [1.0]}]});
    let (base_url, requests) = serve(vec![MockResponse::json("200 OK", &response)]).await;
    let provider = provider(
        base_url,
        json!({"embeddings": true}),
        json!({
            "embeddings": {
                "method": "POST",
                "path": "embeddings",
                "query": {"tags": {"$literal": ["a", "b"]}, "limit": {"$literal": 5}},
                "bodyEncoding": "form",
                "body": {"model": {"$get": "request.model"}, "absent": {"$omitIfNull": {"$get": "request.dimensions"}}},
                "response": {"itemsPointer": "/data", "vectorPointer": "/embedding"}
            }
        }),
    );

    provider
        .embeddings()
        .unwrap()
        .embed(embedding_request())
        .await
        .unwrap();

    let requests = requests.lock().await;
    assert!(
        requests[0]
            .head
            .starts_with("POST /v1/embeddings?limit=5&tags=a&tags=b HTTP/1.1"),
        "unexpected request head: {}",
        requests[0].head
    );
    assert!(
        requests[0]
            .head
            .to_ascii_lowercase()
            .contains("content-type: application/x-www-form-urlencoded")
    );
    assert_eq!(String::from_utf8_lossy(&requests[0].body), "model=embed-1");
}

#[tokio::test]
async fn treats_the_operator_response_limit_as_a_ceiling_a_manifest_cannot_raise() {
    let response = json!({"data": [{"embedding": [1.0]}], "padding": "x".repeat(4096)});
    let (base_url, _) = serve(vec![MockResponse::json("200 OK", &response)]).await;
    let manifest = manifest(
        &base_url,
        json!({"embeddings": true}),
        json!({
            "embeddings": {
                "method": "POST",
                "path": "embeddings",
                // A manifest asking for far more than the operator allows must not win.
                "maxResponseBytes": 64 * 1024 * 1024,
                "bodyEncoding": "json",
                "body": {"input": {"$get": "request.inputs"}},
                "response": {"itemsPointer": "/data", "vectorPointer": "/embedding"}
            }
        }),
    );
    let provider = DeclarativeProvider::new(
        manifest,
        ProviderRuntimeConfig {
            max_response_bytes: 512,
            ..runtime(base_url)
        },
    )
    .unwrap();
    assert!(matches!(
        provider
            .embeddings()
            .unwrap()
            .embed(embedding_request())
            .await
            .unwrap_err(),
        ProviderError::ResponseTooLarge
    ));
}

#[tokio::test]
async fn treats_the_operator_timeout_as_a_ceiling_a_manifest_cannot_raise() {
    let (base_url, _) = serve(vec![MockResponse::chunked(
        "200 OK",
        "application/json",
        vec![b"{\"data\":".to_vec(), b"[]}".to_vec()],
        Duration::from_millis(1500),
    )])
    .await;
    let manifest = manifest(
        &base_url,
        json!({"embeddings": true}),
        json!({
            "embeddings": {
                "method": "POST",
                "path": "embeddings",
                "timeoutMs": 600000,
                "bodyEncoding": "json",
                "body": {"input": {"$get": "request.inputs"}},
                "response": {"itemsPointer": "/data", "vectorPointer": "/embedding"}
            }
        }),
    );
    let provider = DeclarativeProvider::new(
        manifest,
        ProviderRuntimeConfig {
            default_timeout_ms: 300,
            ..runtime(base_url)
        },
    )
    .unwrap();
    let error = provider
        .embeddings()
        .unwrap()
        .embed(embedding_request())
        .await
        .unwrap_err();
    assert!(
        matches!(error, ProviderError::Transport(_)),
        "expected the operator ceiling to time out the request, got {error}"
    );
    assert!(error.is_retryable(), "a timeout is worth retrying");
}

#[tokio::test]
async fn rejects_a_manifest_whose_configuration_schema_is_itself_invalid() {
    // The failure has to surface when the manifest is submitted, not on first provider use.
    let value = json!({
        "manifestVersion": "1.0",
        "id": "broken-schema",
        "version": "1.0",
        "name": "Broken schema",
        "baseUrl": "https://api.example.com/v1/",
        "capabilities": {},
        "configurationSchema": {"type": "not-a-json-schema-type"},
        "operations": {}
    });
    let error = ProviderManifestV1::from_json(&serde_json::to_vec(&value).unwrap()).unwrap_err();
    assert!(
        matches!(error, ProviderError::InvalidManifest(_)),
        "unexpected error: {error}"
    );
    assert!(error.to_string().contains("configurationSchema"), "{error}");
    assert!(error.is_configuration_fault());
}

#[tokio::test]
async fn enforces_the_operation_response_size_limit() {
    let response = json!({"data": [{"embedding": [1.0]}], "padding": "x".repeat(256)});
    let (base_url, _) = serve(vec![MockResponse::json("200 OK", &response)]).await;
    let provider = provider(
        base_url,
        json!({"embeddings": true}),
        json!({
            "embeddings": {
                "method": "POST",
                "path": "embeddings",
                "maxResponseBytes": 32,
                "bodyEncoding": "json",
                "body": {"input": {"$get": "request.inputs"}},
                "response": {"itemsPointer": "/data", "vectorPointer": "/embedding"}
            }
        }),
    );
    assert!(matches!(
        provider
            .embeddings()
            .unwrap()
            .embed(embedding_request())
            .await
            .unwrap_err(),
        ProviderError::ResponseTooLarge
    ));
}

#[tokio::test]
async fn accepts_direct_binary_image_responses() {
    let (base_url, _) = serve_once("200 OK", "image/webp", vec![1, 2, 3, 4]).await;
    let provider = provider(
        base_url,
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
    );

    let result = provider
        .images()
        .unwrap()
        .generate(image_request(1))
        .await
        .unwrap();
    assert_eq!(result.images[0].bytes, vec![1, 2, 3, 4]);
    assert_eq!(result.images[0].media_type, "image/webp");
}

fn json_image_operations() -> Value {
    json!({
        "imageGeneration": {
            "method": "POST",
            "path": "images",
            "bodyEncoding": "json",
            "body": {"prompt": {"$get": "request.prompt"}, "n": {"$get": "request.count"}},
            "response": {
                "bodyEncoding": "json",
                "imagesPointer": "/data",
                "base64Pointer": "/b64_json",
                "mediaTypePointer": "/media_type",
                "defaultMediaType": "image/png"
            }
        }
    })
}

#[tokio::test]
async fn decodes_base64_images_and_enforces_the_requested_count() {
    let response =
        json!({"data": [{"b64_json": "AQID"}, {"b64_json": "BAUG", "media_type": "image/webp"}]});
    let (base_url, _) = serve(vec![MockResponse::json("200 OK", &response)]).await;
    let provider = provider(
        base_url,
        json!({"imageGeneration": true}),
        json_image_operations(),
    );

    let result = provider
        .images()
        .unwrap()
        .generate(image_request(2))
        .await
        .unwrap();
    assert_eq!(result.images[0].bytes, vec![1, 2, 3]);
    assert_eq!(result.images[0].media_type, "image/png");
    assert_eq!(result.images[1].bytes, vec![4, 5, 6]);
    assert_eq!(result.images[1].media_type, "image/webp");
}

#[tokio::test]
async fn ignores_non_image_parts_in_mixed_provider_responses() {
    let response = json!({
        "candidates": [{
            "content": {
                "parts": [
                    {"text": "Here is the generated image."},
                    {"inlineData": {"data": "AQID", "mimeType": "image/png"}}
                ]
            }
        }],
        "usageMetadata": {
            "promptTokenCount": 7,
            "candidatesTokenCount": 11,
            "totalTokenCount": 18
        }
    });
    let (base_url, _) = serve(vec![MockResponse::json("200 OK", &response)]).await;
    let provider = provider(
        base_url,
        json!({"imageGeneration": true}),
        json!({
            "imageGeneration": {
                "method": "POST",
                "path": "models/${request.model}:generateContent",
                "bodyEncoding": "json",
                "body": {"prompt": {"$get": "request.prompt"}},
                "response": {
                    "bodyEncoding": "json",
                    "imagesPointer": "/candidates/0/content/parts",
                    "base64Pointer": "/inlineData/data",
                    "mediaTypePointer": "/inlineData/mimeType",
                    "usage": {
                        "inputTokens": "/usageMetadata/promptTokenCount",
                        "outputTokens": "/usageMetadata/candidatesTokenCount",
                        "totalTokens": "/usageMetadata/totalTokenCount"
                    }
                }
            }
        }),
    );

    let result = provider
        .images()
        .unwrap()
        .generate(image_request(1))
        .await
        .unwrap();
    assert_eq!(result.images[0].bytes, vec![1, 2, 3]);
    assert_eq!(result.images[0].media_type, "image/png");
    let usage = result.usage.unwrap();
    assert_eq!(usage.input_tokens, Some(7));
    assert_eq!(usage.output_tokens, Some(11));
    assert_eq!(usage.total_tokens, Some(18));
}

#[tokio::test]
async fn rejects_image_counts_the_provider_did_not_honour() {
    let response = json!({"data": [{"b64_json": "AQID"}]});
    let (base_url, _) = serve(vec![MockResponse::json("200 OK", &response)]).await;
    let provider = provider(
        base_url,
        json!({"imageGeneration": true}),
        json_image_operations(),
    );
    assert!(
        provider
            .images()
            .unwrap()
            .generate(image_request(3))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn rejects_image_requests_that_ask_for_no_images() {
    // Nothing is served: a zero count is refused before the request goes out.
    let provider = provider(
        "http://127.0.0.1:1/v1/".to_string(),
        json!({"imageGeneration": true}),
        json_image_operations(),
    );
    assert!(matches!(
        provider
            .images()
            .unwrap()
            .generate(image_request(0))
            .await
            .unwrap_err(),
        ProviderError::Configuration(_)
    ));
}

#[tokio::test]
async fn uploads_multipart_bodies_with_decoded_file_parts() {
    let (base_url, requests) = serve_once("200 OK", "image/png", vec![9, 9]).await;
    let provider = provider(
        base_url,
        json!({"imageGeneration": true}),
        json!({
            "imageGeneration": {
                "method": "POST",
                "path": "images/edits",
                "bodyEncoding": "multipart",
                "body": {
                    "prompt": {"$get": "request.prompt"},
                    "image": {
                        "data": {"$get": "/request/inputImages/0/data"},
                        "filename": {"$literal": "input.png"},
                        "mediaType": {"$literal": "image/png"}
                    }
                },
                "response": {"bodyEncoding": "binary"}
            }
        }),
    );

    provider
        .images()
        .unwrap()
        .generate(ImageRequest {
            input_images: vec![llm_plugin::InputImage {
                data: "AQID".to_string(),
                media_type: "image/png".to_string(),
                filename: None,
            }],
            ..image_request(1)
        })
        .await
        .unwrap();

    let requests = requests.lock().await;
    assert!(
        requests[0]
            .head
            .to_ascii_lowercase()
            .contains("content-type: multipart/form-data; boundary=")
    );
    let body = requests[0].body.clone();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("name=\"prompt\""), "{text}");
    assert!(text.contains("filename=\"input.png\""), "{text}");
    // The base64 payload is decoded into raw bytes rather than forwarded as text.
    assert!(
        body.windows(3).any(|window| window == [1, 2, 3]),
        "decoded image bytes missing from {text}"
    );
    assert!(!text.contains("AQID"), "{text}");
}

#[tokio::test]
async fn selects_image_edit_and_repeats_multipart_file_fields() {
    let (base_url, requests) = serve_once("200 OK", "image/png", vec![9, 8, 7]).await;
    let provider = provider(
        base_url,
        json!({"imageGeneration": true}),
        json!({
            "imageGeneration": {
                "method": "POST",
                "path": "must-not-run",
                "bodyEncoding": "json",
                "body": {"prompt": {"$get": "request.prompt"}},
                "response": {"bodyEncoding": "binary"}
            },
            "imageEdit": {
                "method": "POST",
                "path": "images/edits",
                "bodyEncoding": "multipart",
                "body": {
                    "n": {"$get": "request.count"},
                    "image[]": [
                        {
                            "data": {"$get": "/request/inputImages/0/data"},
                            "filename": {"$literal": "first.png"},
                            "mediaType": {"$literal": "image/png"}
                        },
                        {
                            "data": {"$get": "/request/inputImages/1/data"},
                            "filename": {"$literal": "second.webp"},
                            "mediaType": {"$literal": "image/webp"}
                        }
                    ]
                },
                "response": {"bodyEncoding": "binary"}
            }
        }),
    );

    let result = provider
        .images()
        .unwrap()
        .generate(ImageRequest {
            input_images: vec![
                llm_plugin::InputImage {
                    data: "AQID".to_string(),
                    media_type: "image/png".to_string(),
                    filename: Some("first.png".to_string()),
                },
                llm_plugin::InputImage {
                    data: "BAUG".to_string(),
                    media_type: "image/webp".to_string(),
                    filename: Some("second.webp".to_string()),
                },
            ],
            ..image_request(1)
        })
        .await
        .unwrap();
    assert_eq!(result.images[0].bytes, vec![9, 8, 7]);

    let requests = requests.lock().await;
    assert!(requests[0].head.starts_with("POST /v1/images/edits "));
    let body = String::from_utf8_lossy(&requests[0].body);
    assert_eq!(body.matches("name=\"image[]\"").count(), 2, "{body}");
    assert!(body.contains("filename=\"first.png\""), "{body}");
    assert!(body.contains("filename=\"second.webp\""), "{body}");
    assert!(body.contains("name=\"n\""), "{body}");
}

#[tokio::test]
async fn lists_models_from_a_static_manifest_without_an_endpoint() {
    let value = json!({
        "manifestVersion": "1.0",
        "id": "static-models",
        "version": "1.0",
        "name": "Static models",
        "baseUrl": "https://api.example.com/v1/",
        "capabilities": {
            "modelListing": true,
            "embeddings": true,
            "chat": {"streaming": true}
        },
        "models": [
            {
                "id": "model-a",
                "name": "Model A",
                "modelType": "text_embedder",
                "capabilities": {"embeddings": true}
            },
            {
                "id": "model-b",
                "name": "Model B",
                "modelType": "text_generator",
                "capabilities": {"chat": {"streaming": true}}
            }
        ],
        "operations": {
            "chatStream": stub_chat_operation(),
            "embeddings": stub_embeddings_operation()
        }
    });
    let manifest = ProviderManifestV1::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let provider = DeclarativeProvider::new(manifest, ProviderRuntimeConfig::default()).unwrap();

    assert!(provider.descriptor().capabilities.model_listing);
    let models = provider.models().unwrap().list_models().await.unwrap();
    assert_eq!(
        models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        vec!["model-a", "model-b"]
    );
    assert_eq!(models[0].model_type, ProviderModelType::TextEmbedder);
    assert_eq!(models[1].model_type, ProviderModelType::TextGenerator);
}

#[tokio::test]
async fn decodes_model_listings_from_the_provider() {
    let response = json!({"data": [{"id": "gpt-4.1", "display": "GPT 4.1"}, {"id": "o3"}]});
    let (base_url, requests) = serve(vec![MockResponse::json("200 OK", &response)]).await;
    let mut operations = json!({
        "listModels": {
            "method": "GET",
            "path": "models",
            "headers": {"Authorization": {"secret": "api_key", "prefix": "Bearer "}},
            "bodyEncoding": "none",
            "response": {
                "modelsPointer": "/data",
                "idPointer": "/id",
                "namePointer": "/display",
                "defaultModelType": "text_generator",
                "defaultCapabilities": {"chat": {"streaming": true}}
            }
        }
    });
    operations["chatStream"] = stub_chat_operation();
    let provider = provider(
        base_url,
        json!({"modelListing": true, "chat": {"streaming": true}}),
        operations,
    );

    let models = provider.models().unwrap().list_models().await.unwrap();
    assert_eq!(models[0].id.as_str(), "gpt-4.1");
    assert_eq!(models[0].name, "GPT 4.1");
    // Without a name pointer match the id doubles as the display name.
    assert_eq!(models[1].name, "o3");

    let requests = requests.lock().await;
    assert!(requests[0].head.starts_with("GET /v1/models HTTP/1.1"));
    assert_eq!(requests[0].body, Vec::<u8>::new());
}

#[tokio::test]
async fn maps_and_enriches_all_canonical_model_types() {
    let response = json!({
        "data": [
            {
                "id": "chat-1",
                "display": "Provider chat name",
                "kind": "text_generator",
                "capabilities": {"chat": {"streaming": true, "tools": false, "vision": false, "reasoning": false}},
                "metadata": {"inputTokenRate": 99.0, "providerField": "kept"}
            },
            {
                "id": "embed-1",
                "display": "Embedding 1",
                "kind": "text_embedder",
                "capabilities": {"embeddings": true},
                "metadata": {"inputTokenRate": 0.02, "dimensions": 1536}
            },
            {
                "id": "image-1",
                "display": "Provider image name",
                "kind": "image_generator",
                "capabilities": {"imageGeneration": true},
                "metadata": {"pricePerImage": 99.0}
            }
        ]
    });
    let (base_url, _) = serve(vec![MockResponse::json("200 OK", &response)]).await;
    let mut value: Value = serde_json::from_slice(include_bytes!(
        "../examples/openai-compatible.provider.json"
    ))
    .unwrap();
    value["baseUrl"] = json!(&base_url);
    value["capabilities"]["chat"]["reasoning"] = json!(true);
    value["models"] = json!([
        {
            "id": "chat-1",
            "name": "Catalog Chat",
            "modelType": "text_generator",
            "capabilities": {"chat": {"streaming": true, "tools": true, "vision": true, "reasoning": true}},
            "metadata": {"inputTokenRate": 1.25, "outputTokenRate": 5.0, "maxInputTokens": 128000}
        },
        {
            "id": "image-1",
            "name": "Catalog Image",
            "modelType": "image_generator",
            "capabilities": {"imageGeneration": true},
            "metadata": {"pricePerImage": 0.04, "supportsMultipleImages": true}
        }
    ]);
    value["operations"]["listModels"]["response"]["modelMapping"] = json!({
        "id": {"$get": "item.id"},
        "name": {"$get": "item.display"},
        "modelType": {"$get": "item.kind"},
        "capabilities": {"$get": "item.capabilities"},
        "metadata": {"$get": "item.metadata"}
    });
    let manifest = ProviderManifestV1::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let provider = DeclarativeProvider::new(manifest, runtime(base_url)).unwrap();

    let models = provider.models().unwrap().list_models().await.unwrap();
    assert_eq!(models.len(), 3);
    assert_eq!(models[0].model_type, ProviderModelType::TextGenerator);
    assert_eq!(models[0].name, "Catalog Chat");
    assert!(models[0].capabilities.chat.as_ref().unwrap().reasoning);
    assert_eq!(models[0].metadata["inputTokenRate"], 1.25);
    assert_eq!(models[0].metadata["providerField"], "kept");
    assert_eq!(models[1].model_type, ProviderModelType::TextEmbedder);
    assert!(models[1].capabilities.embeddings);
    assert_eq!(models[1].metadata["dimensions"], 1536);
    assert_eq!(models[2].model_type, ProviderModelType::ImageGenerator);
    assert!(models[2].capabilities.image_generation);
    assert_eq!(models[2].name, "Catalog Image");
    assert_eq!(models[2].metadata["pricePerImage"], 0.04);
}

#[tokio::test]
async fn rejects_duplicate_dynamic_model_ids() {
    let response = json!({"data": [{"id": "duplicate"}, {"id": "duplicate"}]});
    let (base_url, _) = serve(vec![MockResponse::json("200 OK", &response)]).await;
    let mut operations = json!({
        "listModels": {
            "method": "GET",
            "path": "models",
            "bodyEncoding": "none",
            "response": {
                "modelsPointer": "/data",
                "idPointer": "/id",
                "defaultModelType": "text_generator",
                "defaultCapabilities": {"chat": {"streaming": true}}
            }
        }
    });
    operations["chatStream"] = stub_chat_operation();
    let provider = provider(
        base_url,
        json!({"modelListing": true, "chat": {"streaming": true}}),
        operations,
    );

    let error = provider.models().unwrap().list_models().await.unwrap_err();
    assert!(matches!(error, ProviderError::ResponseMapping(_)));
}

#[tokio::test]
async fn rejects_mapped_model_capabilities_outside_provider_contract() {
    let response = json!({"data": [{"id": "image-1"}]});
    let (base_url, _) = serve(vec![MockResponse::json("200 OK", &response)]).await;
    let provider = provider(
        base_url,
        json!({"modelListing": true}),
        json!({
            "listModels": {
                "method": "GET",
                "path": "models",
                "bodyEncoding": "none",
                "response": {
                    "modelsPointer": "/data",
                    "modelMapping": {
                        "id": {"$get": "item.id"},
                        "name": {"$get": "item.id"},
                        "modelType": {"$literal": "image_generator"},
                        "capabilities": {"$literal": {"imageGeneration": true}},
                        "metadata": {"$literal": {}}
                    }
                }
            }
        }),
    );

    let error = provider.models().unwrap().list_models().await.unwrap_err();
    assert!(matches!(error, ProviderError::ResponseMapping(_)));
}

async fn embedding_error(status: &str, body: &[u8]) -> ProviderError {
    let (base_url, _) = serve_once(status, "application/json", body.to_vec()).await;
    let provider = provider(
        base_url,
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
    );
    provider
        .embeddings()
        .unwrap()
        .embed(embedding_request())
        .await
        .unwrap_err()
}

#[tokio::test]
async fn maps_rate_limits_without_leaking_provider_bodies() {
    let error = embedding_error(
        "429 Too Many Requests",
        br#"{"error":"secret response detail"}"#,
    )
    .await;
    assert!(matches!(error, ProviderError::QuotaExhausted));
    assert!(!error.to_string().contains("secret response detail"));
    assert!(!error.to_string().contains("super-secret"));
}

#[tokio::test]
async fn classifies_rate_limits_even_when_the_error_body_is_oversized() {
    // Callers back off on `QuotaExhausted`; a chatty 429 body must not be reported as a size error.
    let body = serde_json::to_vec(&json!({"error": "x".repeat(32 * 1024)})).unwrap();
    let error = embedding_error("429 Too Many Requests", &body).await;
    assert!(matches!(error, ProviderError::QuotaExhausted), "{error}");
}

#[tokio::test]
async fn distinguishes_payment_required_from_rate_limits() {
    let error = embedding_error(
        "402 Payment Required",
        br#"{"message":"billing account detail"}"#,
    )
    .await;
    assert!(matches!(error, ProviderError::PaymentRequired));
    assert!(!error.to_string().contains("billing account detail"));
}

#[tokio::test]
async fn keeps_other_http_errors_bounded_and_generic() {
    let unauthorized =
        embedding_error("401 Unauthorized", br#"{"error":"credential fingerprint"}"#).await;
    assert!(matches!(
        unauthorized,
        ProviderError::HttpStatus { status: 401, .. }
    ));
    assert_eq!(
        unauthorized.to_string(),
        "provider returned HTTP 401: Unauthorized"
    );

    let server_error = embedding_error(
        "503 Service Unavailable",
        br#"{"error":"internal provider trace"}"#,
    )
    .await;
    assert!(matches!(
        server_error,
        ProviderError::HttpStatus { status: 503, .. }
    ));
    assert!(!server_error.to_string().contains("internal provider trace"));
}
