// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use rust_decimal::Decimal;
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

    fn input_priced_as(self, cached_input: i32, cache_creation: i32) -> Self {
        Self {
            cached_input,
            cache_creation,
            ..self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEnd {
    Completed,
    Interrupted,
    Refused,
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
    ended: bool,
}

impl RoundUsage {
    // Reported counts only, so a mid-stream save never charges more than the final count.
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

    fn totals(&self, end: TurnEnd) -> UsageTotals {
        let nothing_back =
            self.input.is_none() && self.output.is_none() && self.streamed.is_empty();
        if !self.started || (end == TurnEnd::Refused && nothing_back) {
            return UsageTotals::default();
        }
        let streamed_estimate = estimate_text_tokens(&self.streamed);
        let input = self.input.unwrap_or(self.estimated_input);
        // A stream cut before its last usage update may hold a stale output count (Anthropic
        // reports one token at message start), so streamed text sets a floor under it.
        let cut_short = end != TurnEnd::Completed && !self.ended;
        let (output, output_estimated) = match self.output {
            Some(output) if cut_short && streamed_estimate > output => (streamed_estimate, true),
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

// Usage of one chat turn: one provider request per tool round, each reporting running totals.
// The spend trigger never refunds, so the cost saved for a message only ever goes up.
#[derive(Debug, Clone)]
pub struct TurnUsage {
    finished: UsageTotals,
    round: RoundUsage,
    charged: Decimal,
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
            charged: Decimal::ZERO,
        }
    }

    // Running totals never shrink within a request, so a lower or zero count is a partial update.
    pub fn record(
        &mut self,
        input: Option<u32>,
        output: Option<u32>,
        total: Option<u32>,
        cached_input: Option<u32>,
        cache_creation: Option<u32>,
    ) {
        let tokens = |value: u32| i32::try_from(value).unwrap_or(i32::MAX);
        let highest =
            |reported: Option<u32>, known: Option<i32>| match (reported.map(tokens), known) {
                (Some(reported), Some(known)) => Some(reported.max(known)),
                (reported, known) => reported.or(known),
            };
        let round = &mut self.round;
        round.input = highest(input, round.input);
        round.output = highest(output, round.output);
        round.total = match total {
            Some(_) => highest(total, round.total),
            None if input.is_some() || output.is_some() => None,
            None => round.total,
        };
        round.cached_input = highest(cached_input, round.cached_input);
        round.cache_creation = highest(cache_creation, round.cache_creation);
    }

    pub fn streamed_output(&mut self, text: &str) {
        self.round.streamed.push_str(text);
    }

    pub fn round_ended(&mut self) {
        self.round.ended = true;
    }

    // The next request resends this round's prompt with the reply and the tool results appended.
    pub fn next_round(&mut self, tool_results: &[llm_plugin::ToolResult]) {
        let round = self.round.totals(TurnEnd::Completed);
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
            started: true,
            ..RoundUsage::default()
        };
    }

    // A continuation the provider refused to open costs nothing.
    pub fn round_refused(&mut self) {
        self.round.started = false;
    }

    pub fn reported_totals(&self) -> UsageTotals {
        self.finished.add(self.round.reported())
    }

    pub fn totals(&self) -> UsageTotals {
        self.ended_as(TurnEnd::Completed)
    }

    pub fn ended_as(&self, end: TurnEnd) -> UsageTotals {
        self.finished.add(self.round.totals(end))
    }

    // Prices the open round's input at the cheapest input rate: a later update may still move
    // tokens into a cheaper cache bucket.
    pub fn checkpoint_cost(&mut self, cost_of: impl Fn(&UsageTotals) -> Decimal) -> Decimal {
        let open = self.round.reported();
        let floor = [
            open.input_priced_as(0, 0),
            open.input_priced_as(open.input, 0),
            open.input_priced_as(0, open.input),
        ]
        .iter()
        .map(&cost_of)
        .min()
        .unwrap_or_default();
        self.charge(cost_of(&self.finished) + floor)
    }

    pub fn charge(&mut self, cost: Decimal) -> Decimal {
        self.charged = self.charged.max(cost);
        self.charged
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
        let totals = usage.ended_as(TurnEnd::Interrupted);
        assert_eq!((totals.input, totals.output), (1_200, 100));
        assert!(totals.estimated);
    }

    #[test]
    fn a_stale_output_count_does_not_hide_streamed_text_on_cancel() {
        let mut usage = TurnUsage::new(0);
        usage.record(Some(900), Some(1), None, None, None);
        usage.streamed_output(&"x".repeat(400));
        assert_eq!(usage.totals().output, 1);
        let interrupted = usage.ended_as(TurnEnd::Interrupted);
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
        let totals = usage.ended_as(TurnEnd::Interrupted);
        assert_eq!((totals.input, totals.output), (1_000 + 1_100 + 100, 100));
        assert!(totals.estimated);
        usage.round_refused();
        assert_eq!(usage.ended_as(TurnEnd::Interrupted).input, 1_000);
    }

    #[test]
    fn checkpoints_never_exceed_the_final_charge() {
        let mut usage = TurnUsage::new(5_000);
        usage.record(Some(900), Some(1), None, None, None);
        usage.streamed_output(&"x".repeat(4_000));
        let checkpoint = usage.reported_totals();
        assert_eq!((checkpoint.input, checkpoint.output), (900, 1));
        for end in [usage.totals(), usage.ended_as(TurnEnd::Interrupted)] {
            assert!(end.input >= checkpoint.input && end.output >= checkpoint.output);
        }
        let fresh = TurnUsage::new(5_000);
        assert_eq!(fresh.reported_totals(), UsageTotals::default());
        assert_eq!(fresh.totals().input, 5_000);
    }

    fn priced(totals: &UsageTotals) -> Decimal {
        let regular = totals.input - totals.cached_input - totals.cache_creation;
        Decimal::from(
            regular * 10 + totals.cached_input + totals.cache_creation * 12 + totals.output * 40,
        )
    }

    #[test]
    fn a_late_cache_count_never_lowers_a_saved_cost() {
        let mut usage = TurnUsage::new(0);
        usage.record(Some(1_000), Some(1), None, None, None);
        let early = usage.checkpoint_cost(priced);
        usage.record(Some(1_000), Some(50), Some(1_050), Some(900), None);
        let late = usage.checkpoint_cost(priced);
        let fin = usage.charge(priced(&usage.totals()));
        assert_eq!((early, late), (Decimal::from(1_040), Decimal::from(3_000)));
        assert_eq!(fin, Decimal::from(1_000 + 900 + 2_000));
        assert!(early <= late && late <= fin);
    }

    #[test]
    fn the_saved_cost_only_ratchets_up() {
        let mut usage = TurnUsage::new(0);
        assert_eq!(usage.charge(Decimal::from(5)), Decimal::from(5));
        assert_eq!(usage.charge(Decimal::from(3)), Decimal::from(5));
        assert_eq!(usage.charge(Decimal::from(8)), Decimal::from(8));
    }

    #[test]
    fn a_total_is_dropped_when_later_counts_arrive_without_one() {
        let mut usage = TurnUsage::new(0);
        usage.record(Some(1_000), Some(1), Some(1_001), None, None);
        usage.record(None, Some(350), None, None, None);
        assert_eq!(usage.totals().total, 1_350);
    }

    #[test]
    fn a_refused_request_with_nothing_back_costs_nothing() {
        let usage = TurnUsage::new(1_200);
        assert_eq!(usage.ended_as(TurnEnd::Refused), UsageTotals::default());
        assert_eq!(usage.ended_as(TurnEnd::Interrupted).input, 1_200);
        let mut partial = TurnUsage::new(1_200);
        partial.streamed_output("partial reply");
        assert_eq!(partial.ended_as(TurnEnd::Refused).input, 1_200);
    }

    #[test]
    fn a_round_that_ended_keeps_its_reported_output_when_the_turn_is_stopped() {
        let mut usage = TurnUsage::new(0);
        usage.record(Some(900), Some(5), None, None, None);
        usage.streamed_output(&" ".repeat(400));
        usage.round_ended();
        let totals = usage.ended_as(TurnEnd::Interrupted);
        assert_eq!((totals.output, totals.estimated), (5, false));
    }

    #[test]
    fn a_partial_update_never_lowers_a_running_count() {
        let mut usage = TurnUsage::new(0);
        usage.record(Some(1_000), Some(40), None, Some(300), None);
        usage.record(Some(0), Some(60), None, Some(0), None);
        let totals = usage.totals();
        assert_eq!(
            (totals.input, totals.output, totals.cached_input),
            (1_000, 60, 300)
        );
    }

    #[test]
    fn a_reported_zero_stands() {
        let mut usage = TurnUsage::new(500);
        usage.record(Some(0), Some(0), Some(0), None, None);
        assert_eq!(usage.totals(), UsageTotals::default());
    }
}
