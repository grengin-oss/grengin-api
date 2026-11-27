// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    dto::chat_stream::{ChatStreamWebSearchResult, ChatStreamWebSearchResultItem},
    services::provider_stream::{StreamWebSearchState, ToolInput},
};
use rust_decimal::prelude::{Decimal, FromPrimitive};
use serde_json::Value;

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

pub fn tool_input_to_value(input: Option<&ToolInput>) -> Value {
    match input {
        Some(ToolInput::Json(value)) => value.clone(),
        Some(ToolInput::Text(text)) => {
            serde_json::from_str::<Value>(text).unwrap_or(Value::String(text.clone()))
        }
        None => Value::Null,
    }
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

    #[test]
    fn cache_tokens_are_charged_when_only_cache_rates_are_priced() {
        let cost = calculate_llm_cost(1_000, 300, 200, 0, Some(0.0), Some(1.0), Some(5.0), None);

        assert_eq!(cost, Decimal::from_str_exact("0.0013").unwrap());
    }
}
