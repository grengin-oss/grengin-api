// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

//! Live probes for the two capabilities the API layer needs beyond plain chat: MCP-style function
//! tool calling with a result round trip, and provider-native web search.
//!
//! These are `#[ignore]`d like the other live tests; run them with
//! `GRENGIN_LIVE_PROVIDER_TESTS=1 cargo test -p grengin-provider --test live_tooling -- --ignored`.

use std::{collections::BTreeMap, env};

use futures_util::StreamExt;
use grengin_provider::{
    ChatMessage, ChatRequest, ChatRole, ContentPart, DeclarativeProvider, ModelId, ProviderEvent,
    ProviderManifestV1, ProviderPlugin, ProviderRuntimeConfig, ToolCallId, ToolDefinition,
    ToolResult,
};
use serde_json::{Value, json};

const OPENAI_COMPATIBLE: &[u8] = include_bytes!("../examples/openai-compatible.provider.json");
const ANTHROPIC: &[u8] = include_bytes!("../examples/anthropic.provider.json");

/// A tool name in the shape `crate::llm::tooling::mcp_openai_tool_name` produces, so the probe
/// exercises the real prefixing and length rules rather than a friendly name.
const MCP_TOOL: &str = "mcp__ab12cd34__get_weather__9f3c1d02";

fn credential(name: &str) -> Option<String> {
    if env::var("GRENGIN_LIVE_PROVIDER_TESTS").as_deref() != Ok("1") {
        eprintln!("skipping: GRENGIN_LIVE_PROVIDER_TESTS is not enabled");
        return None;
    }
    match env::var(name).ok().filter(|value| !value.trim().is_empty()) {
        Some(value) => Some(value),
        None => {
            eprintln!("skipping: {name} is not configured");
            None
        }
    }
}

fn build(manifest: Value, api_key: String) -> DeclarativeProvider {
    let manifest = ProviderManifestV1::from_json(&serde_json::to_vec(&manifest).unwrap()).unwrap();
    DeclarativeProvider::new(
        manifest,
        ProviderRuntimeConfig {
            credentials: BTreeMap::from([("api_key".to_string(), api_key)]),
            default_timeout_ms: 60_000,
            ..Default::default()
        },
    )
    .unwrap()
}

fn user(text: &str) -> ChatMessage {
    ChatMessage {
        role: ChatRole::User,
        content: vec![ContentPart::Text {
            text: text.to_string(),
        }],
        tool_calls: Vec::new(),
        tool_result: None,
    }
}

fn request(model: &str, text: &str, tools: Vec<ToolDefinition>) -> ChatRequest {
    ChatRequest {
        model: ModelId::new(model),
        messages: vec![user(text)],
        temperature: Some(0.0),
        max_tokens: Some(1024),
        tools,
        tool_choice: None,
        options: Value::Null,
    }
}

fn weather_tool() -> ToolDefinition {
    ToolDefinition {
        name: MCP_TOOL.to_string(),
        description: Some("Look up the current temperature for a city.".to_string()),
        parameters: json!({
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": ["city"],
            "additionalProperties": false
        }),
    }
}

/// Collects a whole stream, panicking with the error class on failure.
async fn drain(
    stream: &mut grengin_provider::ProviderEventStream,
    label: &str,
) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.unwrap_or_else(|error| panic!("{label} stream failed: {error}")));
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

fn tool_arguments(events: &[ProviderEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::ToolArgumentsDelta { fragment, .. } => Some(fragment.as_str()),
            _ => None,
        })
        .collect()
}

/// Runs the full MCP loop: advertise an MCP-named tool, receive the call, hand back an MCP-shaped
/// result, and confirm the model answers from it.
async fn mcp_round_trip(display: &str, key_env: &str, base_url: &str, model: &str) {
    let Some(api_key) = credential(key_env) else {
        return;
    };
    let mut manifest: Value = serde_json::from_slice(OPENAI_COMPATIBLE).unwrap();
    manifest["baseUrl"] = json!(base_url);
    let provider = build(manifest, api_key);

    let mut session = provider
        .chat()
        .unwrap()
        .start(request(
            model,
            "What is the current temperature in Paris? You must call the provided tool to find out.",
            vec![weather_tool()],
        ))
        .await
        .unwrap();

    let mut stream = session.stream().await.unwrap();
    let first = drain(&mut stream, display).await;
    drop(stream);

    let calls = first
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::ToolCallStart { id, name, .. } => Some((id.clone(), name.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        calls.len(),
        1,
        "{display} did not request exactly one tool call: {first:?}"
    );
    let (call_id, call_name) = calls.into_iter().next().unwrap();
    assert_eq!(
        call_name, MCP_TOOL,
        "{display} mangled the MCP tool name; the 64-char prefixed form must survive the round trip"
    );
    assert!(
        first
            .iter()
            .any(|event| matches!(event, ProviderEvent::ToolCallEnd { .. })),
        "{display} never closed the tool call: {first:?}"
    );
    let arguments: Value = serde_json::from_str(&tool_arguments(&first))
        .unwrap_or_else(|error| panic!("{display} sent unparseable tool arguments: {error}"));
    assert!(
        arguments["city"]
            .as_str()
            .is_some_and(|city| city.to_lowercase().contains("paris")),
        "{display} sent unexpected arguments: {arguments}"
    );

    // MCP results are `{content: [...], isError}`; the manifest JSON-encodes the whole value.
    let mut stream = session
        .continue_with_tools(vec![ToolResult {
            call_id: ToolCallId::new(call_id.as_str()),
            name: MCP_TOOL.to_string(),
            output: json!({
                "content": [{"type": "text", "text": "It is 11 degrees Celsius in Paris."}],
                "isError": false
            }),
            is_error: false,
        }])
        .await
        .unwrap_or_else(|error| panic!("{display} continuation failed: {error}"));
    let second = drain(&mut stream, display).await;
    let answer = text_of(&second);
    assert!(
        answer.contains("11"),
        "{display} did not answer from the tool result: {answer:?} ({second:?})"
    );
    eprintln!("{display} MCP round trip OK -> {answer:?}");
}

#[tokio::test]
#[ignore = "requires GRENGIN_LIVE_PROVIDER_TESTS=1 and a provider credential"]
async fn groq_mcp_tool_round_trip() {
    mcp_round_trip(
        "Groq",
        "GROQ_API_KEY",
        "https://api.groq.com/openai/v1/",
        "llama-3.3-70b-versatile",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires GRENGIN_LIVE_PROVIDER_TESTS=1 and a provider credential"]
async fn mistral_mcp_tool_round_trip() {
    mcp_round_trip(
        "Mistral",
        "MISTRAL_API_KEY",
        "https://api.mistral.ai/v1/",
        "mistral-small-latest",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires GRENGIN_LIVE_PROVIDER_TESTS=1 and a provider credential"]
async fn openrouter_mcp_tool_round_trip() {
    mcp_round_trip(
        "OpenRouter",
        "OPEN_ROUTER_API_KEY",
        "https://openrouter.ai/api/v1/",
        "openai/gpt-4o-mini",
    )
    .await;
}

/// Anthropic's server-side web search through the shipped reference manifest, which now carries
/// `serverTool*` rules. Only `options.nativeTools` is supplied per request.
fn anthropic_search_provider(api_key: String) -> DeclarativeProvider {
    let mut manifest: Value = serde_json::from_slice(ANTHROPIC).unwrap();
    manifest["baseUrl"] = json!("https://api.anthropic.com/v1/");
    build(manifest, api_key)
}

fn native_web_search() -> Value {
    json!([{"type": "web_search_20250305", "name": "web_search", "max_uses": 3}])
}

#[tokio::test]
#[ignore = "requires GRENGIN_LIVE_PROVIDER_TESTS=1 and a provider credential"]
async fn anthropic_web_search_reaches_the_caller_as_structured_citations() {
    let Some(api_key) = credential("ANTHROPIC_API_KEY") else {
        return;
    };
    let provider = anthropic_search_provider(api_key);
    let mut request = request(
        "claude-haiku-4-5-20251001",
        "Search the web for the current stable Rust compiler version and cite your sources.",
        Vec::new(),
    );
    request.options = json!({"nativeTools": native_web_search()});

    let mut session = provider.chat().unwrap().start(request).await.unwrap();
    let mut stream = session.stream().await.unwrap();
    let events = drain(&mut stream, "Anthropic").await;

    let started = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::ServerToolStart { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        started,
        ["web_search"],
        "no server tool started: {events:?}"
    );

    // The query arrives as raw JSON fragments the caller concatenates, matching how it already
    // accumulates client tool input.
    let query: String = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::ServerToolQueryDelta { fragment, .. } => Some(fragment.as_str()),
            _ => None,
        })
        .collect();
    let query: Value = serde_json::from_str(&query)
        .unwrap_or_else(|error| panic!("query fragments did not reassemble: {error} ({query:?})"));
    eprintln!("Anthropic search query: {query}");
    assert!(query["query"].is_string(), "no query in {query}");

    let results = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::ServerToolResult { results, .. } => Some(results.clone()),
            _ => None,
        })
        .flatten()
        .collect::<Vec<_>>();
    assert!(!results.is_empty(), "no citations: {events:?}");
    for result in &results {
        assert!(result.url.starts_with("http"), "bad url: {result:?}");
        assert!(!result.title.is_empty(), "empty title: {result:?}");
    }
    for result in results.iter().take(3) {
        eprintln!("  citation: {} <{}>", result.title, result.url);
    }

    // No client tool call: the caller must never be asked to run the provider's own search.
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ProviderEvent::ToolCallStart { .. })),
        "server search leaked into the client tool space: {events:?}"
    );
    // Provider-internal payloads are gone: the whole event stream stays small.
    let encoded = serde_json::to_string(&events).unwrap();
    assert!(
        !encoded.contains("encrypted_content"),
        "provider blob forwarded to the caller"
    );
    eprintln!(
        "Anthropic web search: {} citations, {} bytes of events total",
        results.len(),
        encoded.len()
    );
}

/// Web search and MCP client tools in one request, which `$concat` now assembles from the canonical
/// `request.tools` plus `options.nativeTools`.
#[tokio::test]
#[ignore = "requires GRENGIN_LIVE_PROVIDER_TESTS=1 and a provider credential"]
async fn anthropic_combines_web_search_with_mcp_client_tools() {
    let Some(api_key) = credential("ANTHROPIC_API_KEY") else {
        return;
    };
    let provider = anthropic_search_provider(api_key);
    let mut request = request(
        "claude-haiku-4-5-20251001",
        "Use the weather tool to get the temperature in Paris. Do not search the web.",
        vec![weather_tool()],
    );
    request.options = json!({"nativeTools": native_web_search()});

    let mut session = provider.chat().unwrap().start(request).await.unwrap();
    let mut stream = session.stream().await.unwrap();
    let events = drain(&mut stream, "Anthropic").await;

    let calls = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::ToolCallStart { id, name, .. } => Some((id.clone(), name.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    eprintln!("Anthropic client tool calls alongside web search: {calls:?}");
    assert_eq!(
        calls.len(),
        1,
        "expected exactly the MCP tool to be called: {events:?}"
    );
    assert_eq!(calls[0].1, MCP_TOOL);

    // And the result round trip still closes on Anthropic's content-block format.
    let mut stream = session
        .continue_with_tools(vec![ToolResult {
            call_id: ToolCallId::new(calls[0].0.as_str()),
            name: MCP_TOOL.to_string(),
            output: json!({
                "content": [{"type": "text", "text": "It is 11 degrees Celsius in Paris."}],
                "isError": false
            }),
            is_error: false,
        }])
        .await
        .unwrap_or_else(|error| panic!("Anthropic continuation failed: {error}"));
    let answer = text_of(&drain(&mut stream, "Anthropic").await);
    assert!(answer.contains("11"), "unexpected answer: {answer:?}");
    eprintln!("Anthropic MCP round trip with web search enabled OK -> {answer:?}");
}
