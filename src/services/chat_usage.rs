// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use serde_json::Value;

use crate::services::llm_usage_gate::estimate_text_tokens;

const ESTIMATED_KEY: &str = "usageEstimated";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageTotals {
    pub input: i32,
    pub output: i32,
    pub total: i32,
    pub cached_input: i32,
    pub cache_creation: i32,
    pub estimated: bool,
}

impl UsageTotals {
    fn add(self, other: Self) -> Self {
        Self {
            input: self.input.saturating_add(other.input),
            output: self.output.saturating_add(other.output),
            total: self.total.saturating_add(other.total),
            cached_input: self.cached_input.saturating_add(other.cached_input),
            cache_creation: self.cache_creation.saturating_add(other.cache_creation),
            estimated: self.estimated || other.estimated,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct RoundUsage {
    input: Option<i32>,
    output: Option<i32>,
    total: Option<i32>,
    cached_input: Option<i32>,
    cache_creation: Option<i32>,
    estimated_input: i32,
    streamed: String,
    started: bool,
}

impl RoundUsage {
    // What the provider has reported so far; nothing is estimated, so a checkpoint never charges
    // more than the final count (the spend trigger charges increases and never refunds).
    fn reported(&self) -> UsageTotals {
        let input = self.input.unwrap_or_default();
        let output = self.output.unwrap_or_default();
        UsageTotals {
            input,
            output,
            total: input.saturating_add(output),
            cached_input: self.cached_input.unwrap_or_default(),
            cache_creation: self.cache_creation.unwrap_or_default(),
            estimated: false,
        }
    }

    // An interrupted round may stop before its final usage update, so streamed output (reply,
    // reasoning, tool arguments) sets a floor under a stale output count; Anthropic reports one
    // output token at message start.
    fn totals(&self, interrupted: bool) -> UsageTotals {
        if !self.started {
            return UsageTotals::default();
        }
        let streamed_estimate = estimate_text_tokens(&self.streamed);
        let input = self.input.unwrap_or(self.estimated_input);
        let (output, output_estimated) = match self.output {
            Some(output) if interrupted && streamed_estimate > output => (streamed_estimate, true),
            Some(output) => (output, false),
            None => (streamed_estimate, true),
        };
        let estimated = self.input.is_none() || output_estimated;
        let total = match (self.total, estimated) {
            (Some(total), false) => total,
            _ => input.saturating_add(output),
        };
        UsageTotals {
            input,
            output,
            total,
            cached_input: self.cached_input.unwrap_or_default(),
            cache_creation: self.cache_creation.unwrap_or_default(),
            estimated,
        }
    }
}

// Token usage of one chat turn, which spans one provider request per tool round. Providers report
// running totals for the current request, so within a round the latest value wins; rounds add up.
// A count the provider never reported is estimated, never charged as zero.
#[derive(Debug, Clone)]
pub struct TurnUsage {
    finished: UsageTotals,
    round: RoundUsage,
}

impl TurnUsage {
    pub fn new(estimated_input: i32) -> Self {
        Self {
            finished: UsageTotals::default(),
            round: RoundUsage {
                estimated_input,
                started: true,
                ..RoundUsage::default()
            },
        }
    }

    pub fn record(
        &mut self,
        input: Option<u32>,
        output: Option<u32>,
        total: Option<u32>,
        cached_input: Option<u32>,
        cache_creation: Option<u32>,
    ) {
        let tokens = |value: u32| i32::try_from(value).unwrap_or(i32::MAX);
        let round = &mut self.round;
        round.input = input.map(tokens).or(round.input);
        round.output = output.map(tokens).or(round.output);
        round.total = total.map(tokens).or(round.total);
        round.cached_input = cached_input.map(tokens).or(round.cached_input);
        round.cache_creation = cache_creation.map(tokens).or(round.cache_creation);
    }

    pub fn streamed_output(&mut self, text: &str) {
        self.round.streamed.push_str(text);
    }

    // The next request resends this round's prompt with the reply and the tool results appended.
    // It counts only once its stream opens; a continuation the provider refused costs nothing.
    pub fn next_round(&mut self, tool_results: &[llm_plugin::ToolResult]) {
        let round = self.round.totals(false);
        let results = tool_results
            .iter()
            .map(|result| estimate_text_tokens(&result.output.to_string()))
            .fold(0i32, i32::saturating_add);
        self.finished = self.finished.add(round);
        self.round = RoundUsage {
            estimated_input: round
                .input
                .saturating_add(round.output)
                .saturating_add(results),
            ..RoundUsage::default()
        };
    }

    pub fn round_opened(&mut self) {
        self.round.started = true;
    }

    pub fn reported_totals(&self) -> UsageTotals {
        self.finished.add(self.round.reported())
    }

    pub fn totals(&self) -> UsageTotals {
        self.finished.add(self.round.totals(false))
    }

    pub fn interrupted_totals(&self) -> UsageTotals {
        self.finished.add(self.round.totals(true))
    }
}

pub fn with_usage_estimated(metadata: Option<Value>) -> Value {
    let mut map = match metadata {
        Some(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    map.insert(ESTIMATED_KEY.to_string(), Value::Bool(true));
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_totals_within_a_round_are_not_added_twice() {
        let mut usage = TurnUsage::new(0);
        usage.record(Some(1_000), Some(1), None, Some(200), None);
        usage.record(Some(1_000), Some(350), Some(1_350), None, None);
        usage.next_round(&[]);
        usage.round_opened();
        usage.record(Some(1_500), Some(1), None, None, Some(40));
        usage.record(Some(1_500), Some(80), None, None, None);
        let totals = usage.totals();
        assert_eq!(
            (
                totals.input,
                totals.output,
                totals.total,
                totals.cached_input,
                totals.cache_creation
            ),
            (2_500, 430, 2_930, 200, 40)
        );
        assert!(!totals.estimated);
    }

    #[test]
    fn a_cancel_before_any_usage_is_estimated_not_free() {
        let mut usage = TurnUsage::new(1_200);
        usage.streamed_output(&"word ".repeat(80));
        let totals = usage.interrupted_totals();
        assert_eq!((totals.input, totals.output), (1_200, 100));
        assert!(totals.estimated);
    }

    #[test]
    fn a_stale_output_count_does_not_hide_streamed_text_on_cancel() {
        let mut usage = TurnUsage::new(0);
        usage.record(Some(900), Some(1), None, None, None);
        usage.streamed_output(&"x".repeat(400));
        assert_eq!(usage.totals().output, 1);
        let interrupted = usage.interrupted_totals();
        assert_eq!((interrupted.input, interrupted.output), (900, 100));
        assert!(interrupted.estimated);
    }

    fn tool_result(text: &str) -> llm_plugin::ToolResult {
        llm_plugin::ToolResult {
            call_id: llm_plugin::ToolCallId::new("call-1"),
            name: "search".to_string(),
            output: Value::String(text.to_string()),
            is_error: false,
        }
    }

    #[test]
    fn a_later_round_without_usage_is_estimated_from_the_previous_one() {
        let mut usage = TurnUsage::new(0);
        usage.record(Some(1_000), Some(100), None, None, None);
        usage.next_round(&[tool_result(&"r".repeat(398))]);
        assert_eq!(usage.interrupted_totals().input, 1_000);
        usage.round_opened();
        let totals = usage.interrupted_totals();
        assert_eq!((totals.input, totals.output), (1_000 + 1_100 + 100, 100));
        assert!(totals.estimated);
    }

    #[test]
    fn checkpoints_never_exceed_the_final_charge() {
        let mut usage = TurnUsage::new(5_000);
        usage.record(Some(900), Some(1), None, None, None);
        usage.streamed_output(&"x".repeat(4_000));
        let checkpoint = usage.reported_totals();
        assert_eq!((checkpoint.input, checkpoint.output), (900, 1));
        for end in [usage.totals(), usage.interrupted_totals()] {
            assert!(end.input >= checkpoint.input && end.output >= checkpoint.output);
        }
        let fresh = TurnUsage::new(5_000);
        assert_eq!(fresh.reported_totals(), UsageTotals::default());
        assert_eq!(fresh.totals().input, 5_000);
    }

    #[test]
    fn a_reported_zero_stands() {
        let mut usage = TurnUsage::new(500);
        usage.record(Some(0), Some(0), Some(0), None, None);
        assert_eq!(usage.totals(), UsageTotals::default());
    }
}
