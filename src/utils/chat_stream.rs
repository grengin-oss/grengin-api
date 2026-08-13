// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    dto::chat_stream::{
        ChatStreamToolInput, ChatStreamWebSearchResult, ChatStreamWebSearchResultItem,
    },
    handlers::llm::{StreamWebSearchState, ToolInput},
};
use rust_decimal::prelude::{Decimal, FromPrimitive};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
pub struct LlmErrorObject {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub code: Option<Value>,
    pub message: Option<String>,
    pub status: Option<String>,
}

#[derive(Deserialize)]
pub struct LlmErrorEnvelope {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub error: Option<LlmErrorObject>,
    pub message: Option<String>,
}

pub fn extract_llm_error_message(body: &str) -> Option<String> {
    let parsed = serde_json::from_str::<LlmErrorEnvelope>(body).ok()?;
    if let Some(msg) = parsed.error.as_ref().and_then(|e| e.message.as_deref()) {
        if !msg.is_empty() {
            return Some(msg.to_string());
        }
    }
    if let Some(msg) = parsed.message.as_deref() {
        if !msg.is_empty() {
            return Some(msg.to_string());
        }
    }
    None
}

pub fn is_rate_limit_error(body: &str) -> bool {
    let Ok(parsed) = serde_json::from_str::<LlmErrorEnvelope>(body) else {
        return false;
    };
    if parsed.kind.as_deref() == Some("rate_limit_error") {
        return true;
    }
    if let Some(error) = parsed.error {
        if error.kind.as_deref() == Some("rate_limit_error") {
            return true;
        }
        if error.code.as_ref().and_then(|c| c.as_str()) == Some("rate_limit_error") {
            return true;
        }
        if error.status.as_deref() == Some("RESOURCE_EXHAUSTED") {
            return true;
        }
    }
    false
}

pub fn calculate_cost_decimal(
    input_tokens: i32,
    output_tokens: i32,
    input_rate: Option<f64>,
    output_rate: Option<f64>,
) -> Decimal {
    let input_rate = input_rate.unwrap_or(0.0);
    let output_rate = output_rate.unwrap_or(0.0);
    if input_rate == 0.0 && output_rate == 0.0 {
        return Decimal::from(0);
    }
    let cost =
        (input_tokens as f64 * input_rate + output_tokens as f64 * output_rate) / 1_000_000.0;
    Decimal::from_f64(cost).unwrap_or_else(|| Decimal::from(0))
}

/// Cost calculation for LLM streams that carry prompt-caching token buckets.
///
/// `input_tokens` is the total from the provider (includes cached + creation subsets).
/// Fallback rule: if a cache rate is None, use `input_rate` to avoid underestimation.
pub fn calculate_llm_cost(
    input_tokens: i32,
    cached_input_tokens: i32,
    cache_creation_tokens: i32,
    output_tokens: i32,
    input_rate: Option<f64>,
    cached_input_rate: Option<f64>,
    cache_creation_rate: Option<f64>,
    output_rate: Option<f64>,
) -> Decimal {
    let input_rate_val = input_rate.unwrap_or(0.0);
    let output_rate_val = output_rate.unwrap_or(0.0);
    // Fall back to input_rate when cache rates are absent to avoid underestimating.
    let cached_rate_val = cached_input_rate.unwrap_or(input_rate_val);
    let creation_rate_val = cache_creation_rate.unwrap_or(input_rate_val);

    if input_rate_val == 0.0 && output_rate_val == 0.0 {
        return Decimal::from(0);
    }

    let regular = (input_tokens - cached_input_tokens - cache_creation_tokens).max(0);
    let cost = (regular as f64 * input_rate_val
        + cached_input_tokens as f64 * cached_rate_val
        + cache_creation_tokens as f64 * creation_rate_val
        + output_tokens as f64 * output_rate_val)
        / 1_000_000.0;
    Decimal::from_f64(cost).unwrap_or_else(|| Decimal::from(0))
}

pub fn to_chat_web_search_result(state: &StreamWebSearchState) -> ChatStreamWebSearchResult {
    ChatStreamWebSearchResult {
        query: state.query.clone(),
        queries: state.queries.clone(),
        results: state
            .results
            .iter()
            .map(|result| ChatStreamWebSearchResultItem {
                title: result.title.clone(),
                url: result.url.clone(),
                source: result.source.clone(),
                page_age: result.page_age.clone(),
                snippet: result.snippet.clone(),
            })
            .collect(),
    }
}

pub fn to_chat_tool_input(input: &ToolInput) -> ChatStreamToolInput {
    match input {
        ToolInput::Text(text) => ChatStreamToolInput::Text { text: text.clone() },
        ToolInput::Json(value) => ChatStreamToolInput::Json {
            value: value.clone(),
        },
    }
}

pub fn tool_input_to_value(input: Option<&ToolInput>) -> Value {
    match input {
        Some(ToolInput::Json(value)) => value.clone(),
        Some(ToolInput::Text(text)) => {
            serde_json::from_str::<Value>(text).unwrap_or(Value::String(text.clone()))
        }
        None => Value::Null,
    }
}

pub fn output_indicates_error(output: &Value) -> bool {
    match output {
        Value::Object(map) => {
            if map
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return true;
            }
            if map.get("isError").and_then(Value::as_bool).unwrap_or(false) {
                return true;
            }
            if map.get("success").and_then(Value::as_bool) == Some(false) {
                return true;
            }
            if map.get("ok").and_then(Value::as_bool) == Some(false) {
                return true;
            }
            if let Some(status) = map.get("status").and_then(Value::as_str) {
                let status = status.to_ascii_lowercase();
                if status == "error" || status == "failed" {
                    return true;
                }
            }
            if let Some(err) = map.get("error") {
                if !err.is_null() {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

pub fn tool_result_status_from_output(output: &Option<Value>) -> Option<String> {
    let is_error = output.as_ref().map(output_indicates_error).unwrap_or(false);
    Some(if is_error { "error" } else { "success" }.to_string())
}

#[cfg(test)]
mod tests {
    use super::calculate_llm_cost;
    use rust_decimal::Decimal;

    #[test]
    fn llm_cost_prices_regular_cached_created_and_output_tokens_separately() {
        let cost = calculate_llm_cost(
            1_000,
            300,
            200,
            100,
            Some(2.0),
            Some(0.2),
            Some(2.5),
            Some(8.0),
        );

        assert_eq!(cost, Decimal::from_str_exact("0.00236").unwrap());
    }

    #[test]
    fn llm_cost_falls_back_to_regular_input_rate_for_missing_cache_rates() {
        let cost = calculate_llm_cost(1_000, 300, 200, 100, Some(2.0), None, None, Some(8.0));

        assert_eq!(cost, Decimal::from_str_exact("0.0028").unwrap());
    }

    #[test]
    fn llm_cost_is_zero_when_model_pricing_is_not_configured() {
        let cost = calculate_llm_cost(1_000, 300, 200, 100, None, None, None, None);

        assert_eq!(cost, Decimal::ZERO);
    }
}
