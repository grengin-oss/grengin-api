// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use llm_plugin::{ChatResponseSpec, DecodedSseEvent, ProviderEvent, SseEventMapper, TokenUsage};
use serde_json::{Value, json};

fn map_usage(rule: Value, payload: Value) -> Result<TokenUsage, llm_plugin::ProviderError> {
    let response = serde_json::from_value::<ChatResponseSpec>(json!({
        "rules": [rule]
    }))
    .unwrap();
    let event = DecodedSseEvent {
        event: None,
        data: payload.to_string(),
        id: None,
    };
    SseEventMapper::new(response)
        .map(&event)?
        .into_iter()
        .find_map(|event| match event {
            ProviderEvent::Usage { usage } => Some(usage),
            _ => None,
        })
        .ok_or_else(|| llm_plugin::ProviderError::ResponseMapping("usage missing".to_string()))
}

fn usage_rule() -> Value {
    json!({
        "id": "usage",
        "emit": "usage",
        "fields": {
            "inputTokens": "/input",
            "outputTokens": "/output",
            "totalTokens": "/total",
            "cachedInputTokens": "/cached",
            "cacheCreationTokens": "/created"
        }
    })
}

#[test]
fn inclusive_provider_keeps_cache_as_input_subsets() {
    let usage = map_usage(
        usage_rule(),
        json!({"input": 20, "output": 7, "total": 27, "cached": 5, "created": 3}),
    )
    .unwrap();

    assert_eq!(usage.input_tokens, Some(20));
    assert_eq!(usage.output_tokens, Some(7));
    assert_eq!(usage.total_tokens, Some(27));
    assert_eq!(usage.cached_input_tokens, Some(5));
    assert_eq!(usage.cache_creation_tokens, Some(3));
}

#[test]
fn exclusive_provider_normalizes_cache_into_input_and_total() {
    let mut rule = usage_rule();
    rule["inputTokensIncludeCached"] = json!(false);
    rule["inputTokensIncludeCacheCreation"] = json!(false);
    let usage = map_usage(
        rule,
        json!({"input": 12, "output": 7, "total": 19, "cached": 5, "created": 3}),
    )
    .unwrap();

    assert_eq!(usage.input_tokens, Some(20));
    assert_eq!(usage.output_tokens, Some(7));
    assert_eq!(usage.total_tokens, Some(27));
    assert_eq!(usage.cached_input_tokens, Some(5));
    assert_eq!(usage.cache_creation_tokens, Some(3));
}

#[test]
fn cache_read_and_creation_inclusion_are_independent() {
    let mut rule = usage_rule();
    rule["inputTokensIncludeCached"] = json!(false);
    let usage = map_usage(
        rule,
        json!({"input": 15, "output": 7, "total": 22, "cached": 5, "created": 3}),
    )
    .unwrap();

    assert_eq!(usage.input_tokens, Some(20));
    assert_eq!(usage.total_tokens, Some(27));
}

#[test]
fn rejects_cache_buckets_larger_than_canonical_input() {
    let error = map_usage(
        usage_rule(),
        json!({"input": 4, "output": 1, "total": 5, "cached": 5, "created": 3}),
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("cache token usage exceeds total input token usage"),
        "{error}"
    );
}

#[test]
fn rejects_overflow_while_normalizing_exclusive_cache_tokens() {
    let mut rule = usage_rule();
    rule["inputTokensIncludeCached"] = json!(false);
    let error = map_usage(
        rule,
        json!({"input": u32::MAX, "output": 0, "total": u32::MAX, "cached": 1, "created": 0}),
    )
    .unwrap_err();

    assert!(error.to_string().contains("supported range"), "{error}");
}
