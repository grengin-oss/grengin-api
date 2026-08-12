// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

//! Drives the declarative runtime against the Node mock provider in `tests/mock/`, using the
//! *shipped* reference manifests rather than purpose-built ones.
//!
//! The Rust mock in `runtime_http.rs` checks the runtime against hand-written frames. This suite
//! checks the manifests we actually ship against a server that reproduces real provider quirks, so
//! a manifest edit that breaks chat, tool calling or web search fails here.
//!
//! Skipped automatically when `node` is unavailable.

use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader},
    process::{Child, ChildStdout, Command, Stdio},
};

use futures_util::StreamExt;
use llm_plugin::{
    ChatMessage, ChatRequest, ChatRole, ContentPart, DeclarativeProvider, EmbeddingRequest,
    ImageRequest, ModelId, ProviderError, ProviderEvent, ProviderManifestV1, ProviderPlugin,
    ProviderRuntimeConfig, ToolCallId, ToolDefinition, ToolResult,
};
use serde_json::{Value, json};

const OPENAI_COMPATIBLE: &[u8] = include_bytes!("../examples/openai-compatible.provider.json");
const ANTHROPIC: &[u8] = include_bytes!("../examples/anthropic.provider.json");
const MCP_TOOL: &str = "mcp__ab12cd34__get_weather__9f3c1d02";

struct MockServer {
    child: Child,
    /// Held open so the server's stdout pipe is never closed under it.
    _stdout: BufReader<ChildStdout>,
    port: u16,
}

impl MockServer {
    /// Boots the mock server, or returns `None` when `node` is not installed.
    fn start() -> Option<Self> {
        if Command::new("node")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("skipping: node is not available");
            return None;
        }
        let script = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/mock/provider-server.mjs"
        );
        let mut child = Command::new("node")
            .arg(script)
            .stdout(Stdio::piped())
            .spawn()
            .expect("failed to spawn the mock provider server");
        let mut stdout = BufReader::new(child.stdout.take().expect("mock server stdout"));
        let mut handshake = String::new();
        stdout
            .read_line(&mut handshake)
            .expect("mock server did not report a port");
        let port = serde_json::from_str::<Value>(&handshake)
            .ok()
            .and_then(|value| value["port"].as_u64())
            .unwrap_or_else(|| panic!("unexpected handshake: {handshake:?}"))
            as u16;
        Some(Self {
            child,
            _stdout: stdout,
            port,
        })
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1/", self.port)
    }

    /// Every request the plugin has sent so far.
    async fn requests(&self) -> Vec<Value> {
        reqwest::get(format!("http://127.0.0.1:{}/__requests", self.port))
            .await
            .expect("could not read recorded requests")
            .json()
            .await
            .expect("recorded requests were not JSON")
    }

    /// Builds a provider from a shipped manifest, optionally patching it first.
    fn provider(&self, manifest: &[u8], patch: impl FnOnce(&mut Value)) -> DeclarativeProvider {
        let mut value: Value = serde_json::from_slice(manifest).unwrap();
        patch(&mut value);
        let manifest =
            ProviderManifestV1::from_json(&serde_json::to_vec(&value).unwrap()).expect("manifest");
        DeclarativeProvider::new(
            manifest,
            ProviderRuntimeConfig {
                base_url_override: Some(self.base_url()),
                credentials: BTreeMap::from([("api_key".to_string(), "mock-key".to_string())]),
                allow_insecure_http: true,
                allow_private_network: true,
                default_timeout_ms: 15_000,
                ..Default::default()
            },
        )
        .expect("provider")
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Boots a server or returns from the test when node is missing.
macro_rules! mock_server {
    () => {
        match MockServer::start() {
            Some(server) => server,
            None => return,
        }
    };
}

fn request(model: &str, text: &str, tools: Vec<ToolDefinition>) -> ChatRequest {
    ChatRequest {
        model: ModelId::new(model),
        messages: vec![ChatMessage {
            role: ChatRole::User,
            content: vec![ContentPart::Text {
                text: text.to_string(),
            }],
            tool_calls: Vec::new(),
            tool_result: None,
        }],
        temperature: Some(0.0),
        max_tokens: Some(512),
        tools,
        tool_choice: None,
        web_search: false,
        options: Value::Null,
    }
}

fn weather_tool() -> ToolDefinition {
    ToolDefinition {
        name: MCP_TOOL.to_string(),
        description: Some("Look up the temperature for a city.".to_string()),
        parameters: json!({
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": ["city"]
        }),
    }
}

async fn drain(stream: &mut llm_plugin::ProviderEventStream) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.expect("stream failed"));
    }
    events
}

fn text_of(events: &[ProviderEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn citations(events: &[ProviderEvent]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::ServerToolResult { results, .. } => Some(results),
            _ => None,
        })
        .flatten()
        .map(|result| (result.title.clone(), result.url.clone()))
        .collect()
}

// ---------------------------------------------------------------------------
// Chat text
// ---------------------------------------------------------------------------

#[tokio::test]
async fn openai_compatible_manifest_streams_chat_text() {
    let server = mock_server!();
    let provider = server.provider(OPENAI_COMPATIBLE, |_| {});
    let mut session = provider
        .chat()
        .unwrap()
        .start(request("mock-model", "hello there", Vec::new()))
        .await
        .unwrap();
    let events = drain(&mut session.stream().await.unwrap()).await;

    // Reassembled across a keepalive comment, a frame split mid-JSON, and a CRLF frame.
    assert_eq!(text_of(&events), "Hello from the mock server");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProviderEvent::MessageStart { .. })),
        "{events:?}"
    );
    let usage = events
        .iter()
        .find_map(|event| match event {
            ProviderEvent::Usage { usage } => Some(usage),
            _ => None,
        })
        .expect("no usage event");
    assert_eq!(usage.total_tokens, Some(15));
    // The mock sends both a finish reason and `[DONE]`; that is one completion, not two.
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ProviderEvent::Completed { .. }))
            .count(),
        1,
        "{events:?}"
    );
}

#[tokio::test]
async fn anthropic_manifest_streams_chat_text() {
    let server = mock_server!();
    let provider = server.provider(ANTHROPIC, |_| {});
    let mut session = provider
        .chat()
        .unwrap()
        .start(request("mock-model", "hello there", Vec::new()))
        .await
        .unwrap();
    let events = drain(&mut session.stream().await.unwrap()).await;

    assert_eq!(text_of(&events), "Hello from the mock server");
    // `ping` events carry data but match no rule, and must not disturb the stream.
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ProviderEvent::Completed { .. }))
            .count(),
        1,
        "{events:?}"
    );

    let sent = &server.requests().await[0]["body"];
    // `max_tokens` is mandatory for Anthropic; `$coalesce` supplies it from the request.
    assert_eq!(sent["max_tokens"], 512);
    assert_eq!(sent["stream"], true);
}

// ---------------------------------------------------------------------------
// Tool calling
// ---------------------------------------------------------------------------

/// Runs the whole MCP loop against a dialect and asserts the replayed request is well formed.
async fn mcp_round_trip(
    server: &MockServer,
    provider: DeclarativeProvider,
    expect_replay: fn(&Value),
) {
    let mut session = provider
        .chat()
        .unwrap()
        .start(request(
            "mock-model",
            "what is the weather in Paris?",
            vec![weather_tool()],
        ))
        .await
        .unwrap();
    let first = drain(&mut session.stream().await.unwrap()).await;

    let calls = first
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::ToolCallStart { id, name, .. } => Some((id.clone(), name.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 1, "expected one tool call in {first:?}");
    // The mock repeats the tool id in every fragment; that must not restart the call.
    assert_eq!(calls[0].1, MCP_TOOL, "MCP tool name was mangled");
    let arguments = first
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::ToolArgumentsDelta { fragment, .. } => Some(fragment.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(
        serde_json::from_str::<Value>(&arguments).unwrap(),
        json!({"city": "Paris"})
    );

    let mut stream = session
        .continue_with_tools(vec![ToolResult {
            call_id: ToolCallId::new(calls[0].0.as_str()),
            name: MCP_TOOL.to_string(),
            output: json!({
                "content": [{"type": "text", "text": "11 degrees"}],
                "isError": false
            }),
            is_error: false,
        }])
        .await
        .expect("continuation failed");
    let second = drain(&mut stream).await;
    assert_eq!(
        text_of(&second),
        "The tool said: 11 degrees",
        "the provider did not receive the tool result: {second:?}"
    );

    let requests = server.requests().await;
    expect_replay(&requests.last().unwrap()["body"]);
}

#[tokio::test]
async fn openai_compatible_manifest_completes_an_mcp_tool_round_trip() {
    let server = mock_server!();
    let provider = server.provider(OPENAI_COMPATIBLE, |_| {});
    mcp_round_trip(&server, provider, |body| {
        let messages = body["messages"].as_array().expect("messages");
        let assistant = messages
            .iter()
            .find(|message| message["role"] == "assistant")
            .expect("no assistant turn was replayed");
        assert_eq!(assistant["tool_calls"][0]["id"], "call_mock_1");
        assert_eq!(assistant["tool_calls"][0]["function"]["name"], MCP_TOOL);
        // Arguments go back as a JSON *string*, which is what the OpenAI schema requires.
        assert_eq!(
            assistant["tool_calls"][0]["function"]["arguments"],
            json!(r#"{"city":"Paris"}"#)
        );
        let tool = messages
            .iter()
            .find(|message| message["role"] == "tool")
            .expect("no tool turn was replayed");
        assert_eq!(tool["tool_call_id"], "call_mock_1");
    })
    .await;
}

#[tokio::test]
async fn anthropic_manifest_completes_an_mcp_tool_round_trip() {
    let server = mock_server!();
    let provider = server.provider(ANTHROPIC, |_| {});
    mcp_round_trip(&server, provider, |body| {
        let messages = body["messages"].as_array().expect("messages");
        let assistant = messages
            .iter()
            .find(|message| message["role"] == "assistant")
            .expect("no assistant turn was replayed");
        // Anthropic wants a `tool_use` content block, not an OpenAI-style `tool_calls` array.
        let block = assistant["content"]
            .as_array()
            .expect("assistant content blocks")
            .iter()
            .find(|block| block["type"] == "tool_use")
            .expect("no tool_use block");
        assert_eq!(block["name"], MCP_TOOL);
        assert_eq!(block["input"], json!({"city": "Paris"}));
        // The result comes back as a user turn holding a `tool_result` block.
        let result = messages
            .iter()
            .filter(|message| message["role"] == "user")
            .flat_map(|message| message["content"].as_array().cloned().unwrap_or_default())
            .find(|block| block["type"] == "tool_result")
            .expect("no tool_result block");
        assert_eq!(result["tool_use_id"], "toolu_mock");
    })
    .await;
}

// ---------------------------------------------------------------------------
// Web search
// ---------------------------------------------------------------------------

#[tokio::test]
async fn openai_compatible_manifest_surfaces_web_search_citations() {
    let server = mock_server!();
    let provider = server.provider(OPENAI_COMPATIBLE, |_| {});
    let mut session = provider
        .chat()
        .unwrap()
        .start(request(
            "mock-model",
            "search the web for the rust version",
            Vec::new(),
        ))
        .await
        .unwrap();
    let events = drain(&mut session.stream().await.unwrap()).await;

    assert_eq!(
        citations(&events),
        vec![
            (
                "Rust Releases".to_string(),
                "https://releases.rs/".to_string()
            ),
            (
                "Rust Blog".to_string(),
                "https://blog.rust-lang.org/".to_string()
            ),
        ],
        "the citation without a url should be skipped, not fail the stream: {events:?}"
    );
    assert_eq!(text_of(&events), "Rust 1.90");
}

#[tokio::test]
async fn anthropic_manifest_routes_web_search_and_a_client_tool_from_one_index_space() {
    let server = mock_server!();
    let provider = server.provider(ANTHROPIC, |_| {});
    let mut chat = request(
        "mock-model",
        "search for the weather in Paris",
        vec![weather_tool()],
    );
    chat.web_search = true;
    let mut session = provider.chat().unwrap().start(chat).await.unwrap();
    let events = drain(&mut session.stream().await.unwrap()).await;

    // `$concat` put both tool kinds in the one array the provider accepts.
    let sent = &server.requests().await[0]["body"];
    let tools = sent["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 2, "unexpected tools payload: {sent}");
    assert!(tools.iter().any(|tool| tool["name"] == MCP_TOOL));
    assert!(
        tools
            .iter()
            .any(|tool| tool["type"] == "web_search_20250305")
    );

    // The provider-run search reports as a server tool.
    assert!(
        events.iter().any(|event| matches!(
            event,
            ProviderEvent::ServerToolStart { name, .. } if name == "web_search"
        )),
        "{events:?}"
    );
    let query = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::ServerToolQueryDelta { fragment, .. } => Some(fragment.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(
        serde_json::from_str::<Value>(&query).unwrap(),
        json!({"query": "rust version"}),
        "query fragments did not reassemble"
    );
    assert_eq!(
        citations(&events),
        vec![
            (
                "Rust Releases".to_string(),
                "https://releases.rs/".to_string()
            ),
            // Title falls back to the url when the provider omits it.
            (
                "https://blog.rust-lang.org/".to_string(),
                "https://blog.rust-lang.org/".to_string()
            ),
        ],
        "{events:?}"
    );

    // The client tool shares the index space and still arrives intact and executable.
    let calls = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::ToolCallStart { id, name, .. } => Some((id.as_str(), name.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        calls,
        vec![("toolu_mock", MCP_TOOL)],
        "the search block must not leak into the client tool space: {events:?}"
    );
    let arguments = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::ToolArgumentsDelta { id, fragment } if id.as_str() == "toolu_mock" => {
                Some(fragment.as_str())
            }
            _ => None,
        })
        .collect::<String>();
    assert_eq!(
        serde_json::from_str::<Value>(&arguments).unwrap(),
        json!({"city": "Paris"})
    );

    // The mock attaches 1.2KB of `encrypted_content` per result; none of it reaches the caller.
    let encoded = serde_json::to_string(&events).unwrap();
    assert!(
        !encoded.contains("MOCK_ENCRYPTED_BLOB"),
        "provider blob leaked"
    );
    assert!(
        encoded.len() < 4096,
        "event payload is bloated: {} bytes",
        encoded.len()
    );
}

// ---------------------------------------------------------------------------
// Non-streaming operations and error mapping
// ---------------------------------------------------------------------------

#[tokio::test]
async fn openai_compatible_manifest_handles_buffered_operations() {
    let server = mock_server!();
    let provider = server.provider(OPENAI_COMPATIBLE, |_| {});

    // The mock returns vectors in reverse index order; the runtime must reorder them.
    let embeddings = provider
        .embeddings()
        .unwrap()
        .embed(EmbeddingRequest {
            model: ModelId::new("mock-embed"),
            inputs: vec!["a".to_string(), "b".to_string()],
            dimensions: None,
            options: Value::Null,
        })
        .await
        .unwrap();
    assert_eq!(
        embeddings.vectors,
        vec![vec![0.5, 1.5, 2.5], vec![1.5, 2.5, 3.5]]
    );
    assert_eq!(embeddings.usage.unwrap().total_tokens, Some(6));

    let images = provider
        .images()
        .unwrap()
        .generate(ImageRequest {
            model: ModelId::new("mock-image"),
            prompt: "a cat".to_string(),
            input_images: Vec::new(),
            count: 2,
            size: None,
            quality: None,
            options: Value::Null,
        })
        .await
        .unwrap();
    assert_eq!(images.images.len(), 2);
    assert_eq!(images.images[0].bytes, b"mock-png");
    assert_eq!(images.images[0].media_type, "image/png");

    let models = provider.models().unwrap().list_models().await.unwrap();
    assert_eq!(
        models
            .iter()
            .map(|model| (model.id.as_str(), model.name.as_str()))
            .collect::<Vec<_>>(),
        // Real `/v1/models` has no display-name field, so the shipped manifest declares no
        // `namePointer` and the id doubles as the name even though the mock offers one.
        vec![
            ("mock-model", "mock-model"),
            ("mock-model-mini", "mock-model-mini")
        ]
    );
    assert_eq!(models[0].metadata["display_name"], "Mock Model");
}

/// Points the embeddings operation at one of the mock's failure routes.
async fn chaos_error(server: &MockServer, route: &str) -> ProviderError {
    let provider = server.provider(OPENAI_COMPATIBLE, |manifest| {
        manifest["operations"]["embeddings"]["path"] = json!(format!("chaos/{route}"));
    });
    provider
        .embeddings()
        .unwrap()
        .embed(EmbeddingRequest {
            model: ModelId::new("mock-embed"),
            inputs: vec!["a".to_string()],
            dimensions: None,
            options: Value::Null,
        })
        .await
        .expect_err("expected the chaos route to fail")
}

#[tokio::test]
async fn classifies_provider_failures_from_the_mock() {
    let server = mock_server!();

    // The mock's 429 body is ~45KB, far above the error-body cap: the status must still win.
    let rate_limit = chaos_error(&server, "rate-limit").await;
    assert!(
        matches!(rate_limit, ProviderError::QuotaExhausted),
        "{rate_limit}"
    );
    assert!(rate_limit.is_retryable());
    assert!(!rate_limit.to_string().contains("slow down"), "body leaked");

    let payment = chaos_error(&server, "payment").await;
    assert!(matches!(payment, ProviderError::PaymentRequired));
    assert!(
        !payment.is_retryable(),
        "billing failures must not be retried"
    );

    let server_error = chaos_error(&server, "server").await;
    assert!(matches!(
        server_error,
        ProviderError::HttpStatus { status: 503, .. }
    ));
    assert!(server_error.is_retryable());
    assert!(!server_error.to_string().contains("upstream unavailable"));
}

#[tokio::test]
async fn rejects_a_chat_response_that_is_not_an_event_stream() {
    let server = mock_server!();
    let provider = server.provider(OPENAI_COMPATIBLE, |manifest| {
        manifest["operations"]["chatStream"]["path"] = json!("chaos/not-sse");
    });
    let mut session = provider
        .chat()
        .unwrap()
        .start(request("mock-model", "hello", Vec::new()))
        .await
        .unwrap();
    let error = match session.stream().await {
        Ok(_) => panic!("a JSON body must not be accepted as a stream"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("text/event-stream"), "{error}");
    assert!(
        !error.is_retryable(),
        "a wrong content type is a config fault"
    );
}

#[tokio::test]
async fn reports_a_stream_that_ends_without_completing() {
    let server = mock_server!();
    let provider = server.provider(OPENAI_COMPATIBLE, |manifest| {
        manifest["operations"]["chatStream"]["path"] = json!("chaos/truncated");
    });
    let mut session = provider
        .chat()
        .unwrap()
        .start(request("mock-model", "hello", Vec::new()))
        .await
        .unwrap();
    let mut stream = session.stream().await.unwrap();
    let mut last = None;
    while let Some(event) = stream.next().await {
        match event {
            // The chunk carries an id, so a message start precedes the text delta.
            Ok(event) => assert!(
                matches!(
                    event,
                    ProviderEvent::MessageStart { .. } | ProviderEvent::TextDelta { .. }
                ),
                "unexpected event before the truncation: {event:?}"
            ),
            Err(error) => {
                last = Some(error);
                break;
            }
        }
    }
    let error = last.expect("a truncated stream must surface an error");
    assert!(matches!(error, ProviderError::StreamEnded), "{error}");
    assert!(error.is_retryable(), "a truncated stream is worth retrying");
}
