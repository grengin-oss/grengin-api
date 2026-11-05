// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{collections::BTreeMap, env, fs, path::Path};

use futures_util::StreamExt;
use llm_plugin::{
    ChatMessage, ChatRequest, ChatRole, ContentPart, DeclarativeProvider, ModelId, ProviderEvent,
    ProviderManifestV1, ProviderPlugin, ProviderRuntimeConfig, ToolDefinition, ToolResult,
};
use serde_json::{Value, json};

#[tokio::test]
#[ignore = "requires explicit GEMINI_API_KEY and GEMINI_TEST_MODEL"]
async fn gemini_flash_mcp_json_schema_round_trip() {
    let key = env::var("GEMINI_API_KEY").expect("export GEMINI_API_KEY");
    let model = env::var("GEMINI_TEST_MODEL").expect("choose GEMINI_TEST_MODEL explicitly");
    let manifest = if let Some(catalog) = env::var("GRENGIN_PROVIDER_CATALOG_DIR")
        .ok()
        .filter(|path| !path.trim().is_empty())
    {
        fs::read(Path::new(&catalog).join("gemini/plugin.json")).unwrap()
    } else {
        include_bytes!("../examples/gemini.provider.json").to_vec()
    };
    let provider = DeclarativeProvider::new(
        ProviderManifestV1::from_json(&manifest).unwrap(),
        ProviderRuntimeConfig {
            credentials: BTreeMap::from([("api_key".into(), key)]),
            default_timeout_ms: 45_000,
            ..Default::default()
        },
    )
    .unwrap();
    let mut session = provider.chat().unwrap().start(ChatRequest {
        model: ModelId::new(&model),
        messages: vec![ChatMessage {
            role: ChatRole::User,
            content: vec![ContentPart::Text { text: "You must call lookup_fixture with query=fixture and limit=1 to learn a private fixture code. Then answer with exactly the returned code. Do not guess the code.".into() }],
            tool_calls: vec![], tool_result: None,
        }],
        temperature: Some(0.0), max_tokens: Some(2048),
        tools: vec![ToolDefinition {
            name: "lookup_fixture".into(), description: Some("Read a synthetic fixture; no external effects.".into()),
            parameters: json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":{"query":{"type":"string"},"limit":{"anyOf":[{"type":"integer"},{"type":"null"}]}},"required":["query"],"additionalProperties":false}),
        }],
        tool_choice: None, web_search: false, options: Value::Null,
    }).await.unwrap();
    let mut first = session.stream().await.unwrap();
    let mut calls = Vec::new();
    while let Some(event) = first.next().await {
        if let ProviderEvent::ToolCallStart { id, name, .. } = event.unwrap() {
            assert_eq!(name, "lookup_fixture");
            calls.push(id);
        }
    }
    assert_eq!(calls.len(), 1, "model must request one fixture tool");
    let mut continued = session
        .continue_with_tools(vec![ToolResult {
            call_id: calls.remove(0),
            name: "lookup_fixture".into(),
            output: json!({"code":"FIXTURE_OK_7391"}),
            is_error: false,
        }])
        .await
        .unwrap();
    let mut text = String::new();
    let mut completed = false;
    while let Some(event) = continued.next().await {
        match event.unwrap() {
            ProviderEvent::TextDelta { text: delta } => text.push_str(&delta),
            ProviderEvent::Completed { .. } => completed = true,
            _ => {}
        }
    }
    assert!(completed);
    assert!(text.contains("FIXTURE_OK_7391"), "tool result was not used");
    eprintln!("PASS {model}: native plugin stream, JSON Schema tool and continuation");
}
