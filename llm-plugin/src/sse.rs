// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;

use serde_json::Value;

use crate::{
    ChatResponseSpec, EventDataEncoding, EventKind, FinishReason, ProviderError, ProviderEvent,
    ProviderEventErrorKind, RequestId, ResponseRule, ServerToolResultItem, TokenUsage, ToolCallId,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedSseEvent {
    pub event: Option<String>,
    pub data: String,
    pub id: Option<String>,
}

pub struct SseDecoder {
    buffer: Vec<u8>,
    event_name: Option<String>,
    event_id: Option<String>,
    data_lines: Vec<String>,
    current_event_bytes: usize,
    max_event_bytes: usize,
}

impl SseDecoder {
    pub fn new(max_event_bytes: usize) -> Self {
        Self {
            buffer: Vec::new(),
            event_name: None,
            event_id: None,
            data_lines: Vec::new(),
            current_event_bytes: 0,
            max_event_bytes,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<DecodedSseEvent>, ProviderError> {
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line = self.buffer.drain(..=newline).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.process_line(&line, &mut events)?;
        }
        if self.buffer.len().saturating_add(self.current_event_bytes) > self.max_event_bytes {
            return Err(ProviderError::ResponseTooLarge);
        }
        Ok(events)
    }

    pub fn finish(&mut self) -> Result<Vec<DecodedSseEvent>, ProviderError> {
        let mut events = Vec::new();
        if !self.buffer.is_empty() {
            let mut line = std::mem::take(&mut self.buffer);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.process_line(&line, &mut events)?;
        }
        if let Some(event) = self.dispatch() {
            events.push(event);
        }
        Ok(events)
    }

    fn process_line(
        &mut self,
        line: &[u8],
        events: &mut Vec<DecodedSseEvent>,
    ) -> Result<(), ProviderError> {
        self.current_event_bytes = self.current_event_bytes.saturating_add(line.len() + 1);
        if self.current_event_bytes > self.max_event_bytes {
            return Err(ProviderError::ResponseTooLarge);
        }
        let line = std::str::from_utf8(line).map_err(|error| {
            ProviderError::ResponseMapping(format!("SSE event is not valid UTF-8: {error}"))
        })?;
        if line.is_empty() {
            if let Some(event) = self.dispatch() {
                events.push(event);
            }
            return Ok(());
        }
        if line.starts_with(':') {
            return Ok(());
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.event_name = Some(value.to_string()),
            "data" => self.data_lines.push(value.to_string()),
            "id" if !value.contains('\0') => self.event_id = Some(value.to_string()),
            "retry" => {}
            _ => {}
        }
        Ok(())
    }

    fn dispatch(&mut self) -> Option<DecodedSseEvent> {
        self.current_event_bytes = 0;
        if self.data_lines.is_empty() {
            self.event_name = None;
            return None;
        }
        Some(DecodedSseEvent {
            event: self.event_name.take(),
            data: self.data_lines.drain(..).collect::<Vec<_>>().join("\n"),
            id: self.event_id.clone(),
        })
    }
}

#[derive(Clone)]
pub struct SseEventMapper {
    response: ChatResponseSpec,
    state: MapperState,
}

impl SseEventMapper {
    pub fn new(response: ChatResponseSpec) -> Self {
        Self {
            response,
            state: MapperState::default(),
        }
    }

    /// True once a completion event has been emitted; every later event is ignored.
    pub fn is_completed(&self) -> bool {
        self.state.completed
    }

    pub fn map(&mut self, event: &DecodedSseEvent) -> Result<Vec<ProviderEvent>, ProviderError> {
        if self.response.done_data.as_deref() == Some(event.data.trim()) {
            if self.state.completed {
                return Ok(Vec::new());
            }
            return Ok(self.state.complete(None));
        }

        let value = self.decode_data(event)?;

        let mut output = Vec::new();
        for rule in &self.response.rules {
            if self.state.completed && !survives_completion(rule.emit) {
                continue;
            }
            if let Some(pointer) = &rule.for_each {
                let Some(selected) = value.pointer(pointer) else {
                    continue;
                };
                let items = selected.as_array().ok_or_else(|| {
                    ProviderError::ResponseMapping(format!(
                        "response rule {} forEach pointer is not an array",
                        rule.id
                    ))
                })?;
                for item in items {
                    if rule_matches(rule, item) {
                        output.extend(self.state.map_rule(rule, item)?);
                    }
                }
            } else if rule_matches(rule, &value) {
                output.extend(self.state.map_rule(rule, &value)?);
            }
        }
        Ok(output)
    }

    pub fn decode_data(&self, event: &DecodedSseEvent) -> Result<Value, ProviderError> {
        Ok(match self.response.event_data_encoding {
            EventDataEncoding::Json => {
                serde_json::from_str::<Value>(&event.data).map_err(|error| {
                    ProviderError::ResponseMapping(format!("SSE data is not valid JSON: {error}"))
                })?
            }
            EventDataEncoding::Text => Value::String(event.data.clone()),
        })
    }
}

/// Cross-event mapper state, kept separate from the immutable spec so mapping a single event
/// borrows the rules without cloning them.
#[derive(Clone, Default)]
struct MapperState {
    tool_ids_by_index: BTreeMap<u32, ToolCallId>,
    /// Server-executed tools, tracked separately because providers such as Anthropic number them
    /// in the same content-block index space as client tools while streaming an identical delta
    /// shape. Knowing which space an index belongs to is what lets one rule per kind coexist.
    server_tools_by_index: BTreeMap<u32, (Option<ToolCallId>, String)>,
    message_started: bool,
    completed: bool,
}

/// Outcome of resolving a rule's target when the rule and the event may disagree about which tool
/// space an index belongs to.
enum Resolved<T> {
    Found(T),
    /// The index belongs to the other tool space, so this rule simply does not apply.
    OtherSpace,
}

impl MapperState {
    fn map_rule(
        &mut self,
        rule: &ResponseRule,
        value: &Value,
    ) -> Result<Vec<ProviderEvent>, ProviderError> {
        let events = match rule.emit {
            EventKind::MessageStart => {
                if self.message_started {
                    return Ok(Vec::new());
                }
                self.message_started = true;
                vec![ProviderEvent::MessageStart {
                    request_id: optional_string(rule, value, "requestId")?.map(RequestId::new),
                }]
            }
            EventKind::TextDelta => vec![ProviderEvent::TextDelta {
                text: required_rule_string(rule, value, "value")?,
            }],
            EventKind::ReasoningDelta => vec![ProviderEvent::ReasoningDelta {
                text: required_rule_string(rule, value, "value")?,
            }],
            EventKind::ToolCallStart => {
                let id = ToolCallId::new(required_string(rule, value, "id")?);
                let index = optional_u32(rule, value, "index")?.unwrap_or(0);
                match self.tool_ids_by_index.get(&index) {
                    // Several OpenAI-compatible providers repeat the tool id in every argument
                    // chunk, so re-announcing the same call is a no-op rather than an error.
                    Some(existing) if existing == &id => return Ok(Vec::new()),
                    Some(existing) => {
                        return Err(ProviderError::ResponseMapping(format!(
                            "tool index {index} was already assigned to {existing}"
                        )));
                    }
                    None => {}
                }
                let name = required_string(rule, value, "name")?;
                self.tool_ids_by_index.insert(index, id.clone());
                vec![ProviderEvent::ToolCallStart { id, name, index }]
            }
            EventKind::ToolArgumentsDelta => {
                let Resolved::Found(id) = self.resolve_tool_id(rule, value)? else {
                    return Ok(Vec::new());
                };
                vec![ProviderEvent::ToolArgumentsDelta {
                    id,
                    fragment: required_string(rule, value, "fragment")?,
                }]
            }
            EventKind::ToolCallEnd => {
                let Resolved::Found(id) = self.resolve_tool_id(rule, value)? else {
                    return Ok(Vec::new());
                };
                self.tool_ids_by_index.retain(|_, active| active != &id);
                vec![ProviderEvent::ToolCallEnd { id }]
            }
            EventKind::ServerToolStart => {
                let id = optional_string(rule, value, "id")?.map(ToolCallId::new);
                let name = required_string(rule, value, "name")?;
                if let Some(index) = optional_u32(rule, value, "index")? {
                    // Re-announcing the same block is a no-op, matching client-tool behaviour.
                    if self.server_tools_by_index.contains_key(&index) {
                        return Ok(Vec::new());
                    }
                    self.server_tools_by_index
                        .insert(index, (id.clone(), name.clone()));
                }
                vec![ProviderEvent::ServerToolStart {
                    id,
                    name,
                    query: optional_string(rule, value, "query")?,
                    queries: optional_string_list(rule, value, "queries")?,
                }]
            }
            EventKind::ServerToolQueryDelta => {
                let Resolved::Found((id, name)) = self.resolve_server_tool(rule, value)? else {
                    return Ok(Vec::new());
                };
                vec![ProviderEvent::ServerToolQueryDelta {
                    id,
                    name,
                    fragment: required_string(rule, value, "fragment")?,
                }]
            }
            EventKind::ServerToolResult => {
                let Resolved::Found((id, name)) = self.resolve_server_tool(rule, value)? else {
                    return Ok(Vec::new());
                };
                let pointer = rule.collect.as_deref().ok_or_else(|| {
                    ProviderError::ResponseMapping(format!(
                        "response rule {} requires collect to gather results",
                        rule.id
                    ))
                })?;
                vec![ProviderEvent::ServerToolResult {
                    id,
                    name,
                    results: collect_results(rule, value, pointer)?,
                }]
            }
            EventKind::Usage => vec![ProviderEvent::Usage {
                usage: TokenUsage {
                    input_tokens: optional_u32(rule, value, "inputTokens")?,
                    output_tokens: optional_u32(rule, value, "outputTokens")?,
                    total_tokens: optional_u32(rule, value, "totalTokens")?,
                    cached_input_tokens: optional_u32(rule, value, "cachedInputTokens")?,
                    cache_creation_tokens: optional_u32(rule, value, "cacheCreationTokens")?,
                },
            }],
            EventKind::ProviderEvent => vec![ProviderEvent::ProviderEvent {
                kind: required_string(rule, value, "kind")?,
                data: rule
                    .fields
                    .get("data")
                    .and_then(|pointer| value.pointer(pointer))
                    .cloned()
                    .unwrap_or_else(|| value.clone()),
            }],
            EventKind::Error => {
                let kind =
                    optional_string(rule, value, "kind")?.unwrap_or_else(|| "provider".to_string());
                vec![ProviderEvent::Error {
                    kind: if matches!(kind.as_str(), "quota" | "rate_limit" | "quota_exhausted") {
                        ProviderEventErrorKind::QuotaExhausted
                    } else {
                        ProviderEventErrorKind::Provider
                    },
                    message: required_string(rule, value, "message")?,
                }]
            }
            EventKind::Completed => {
                let finish_reason = optional_string(rule, value, "finishReason")?
                    .as_deref()
                    .map(parse_finish_reason);
                self.complete(finish_reason)
            }
        };
        Ok(events)
    }

    fn resolve_tool_id(
        &self,
        rule: &ResponseRule,
        value: &Value,
    ) -> Result<Resolved<ToolCallId>, ProviderError> {
        if let Some(id) = optional_string(rule, value, "id")? {
            return Ok(Resolved::Found(ToolCallId::new(id)));
        }
        let index = self.required_index(rule, value)?;
        if let Some(id) = self.tool_ids_by_index.get(&index) {
            return Ok(Resolved::Found(id.clone()));
        }
        if self.server_tools_by_index.contains_key(&index) {
            return Ok(Resolved::OtherSpace);
        }
        Err(ProviderError::ResponseMapping(format!(
            "response rule {} references unknown tool index {index}",
            rule.id
        )))
    }

    fn resolve_server_tool(
        &self,
        rule: &ResponseRule,
        value: &Value,
    ) -> Result<Resolved<(Option<ToolCallId>, String)>, ProviderError> {
        let id = optional_string(rule, value, "id")?.map(ToolCallId::new);
        // An explicit name plus an id (Anthropic's `web_search_tool_result` carries `tool_use_id`)
        // is enough on its own; index lookup is only the fallback.
        if let Some(name) = optional_string(rule, value, "name")?
            && (id.is_some() || optional_u32(rule, value, "index")?.is_none())
        {
            return Ok(Resolved::Found((id, name)));
        }
        let index = self.required_index(rule, value)?;
        if let Some((known_id, name)) = self.server_tools_by_index.get(&index) {
            return Ok(Resolved::Found((
                id.or_else(|| known_id.clone()),
                name.clone(),
            )));
        }
        if self.tool_ids_by_index.contains_key(&index) {
            return Ok(Resolved::OtherSpace);
        }
        Err(ProviderError::ResponseMapping(format!(
            "response rule {} references unknown server tool index {index}",
            rule.id
        )))
    }

    fn required_index(&self, rule: &ResponseRule, value: &Value) -> Result<u32, ProviderError> {
        optional_u32(rule, value, "index")?.ok_or_else(|| {
            ProviderError::ResponseMapping(format!(
                "response rule {} requires a tool id or index",
                rule.id
            ))
        })
    }

    fn complete(&mut self, finish_reason: Option<FinishReason>) -> Vec<ProviderEvent> {
        let mut events = std::mem::take(&mut self.tool_ids_by_index)
            .into_values()
            .map(|id| ProviderEvent::ToolCallEnd { id })
            .collect::<Vec<_>>();
        self.completed = true;
        events.push(ProviderEvent::Completed { finish_reason });
        events
    }
}

/// Which rule kinds still apply once the completion event has been emitted.
///
/// Content and tool events after completion are trailing noise and are dropped, but accounting and
/// diagnostics are not: OpenAI-compatible providers send the `stream_options.include_usage` totals
/// in a chunk *after* the finish-reason chunk, so sealing the mapper outright would silently
/// discard the token counts callers bill on.
fn survives_completion(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::Usage | EventKind::Error | EventKind::ProviderEvent
    )
}

fn rule_matches(rule: &ResponseRule, value: &Value) -> bool {
    let Some(condition) = &rule.when else {
        return true;
    };
    let selected = value.pointer(&condition.pointer);
    if let Some(exists) = condition.exists
        && selected.is_some() != exists
    {
        return false;
    }
    if let Some(expected) = &condition.equals
        && selected != Some(expected)
    {
        return false;
    }
    if let Some(not_null) = condition.not_null
        && selected.is_some_and(|selected| !selected.is_null()) != not_null
    {
        return false;
    }
    true
}

fn required_rule_string(
    rule: &ResponseRule,
    value: &Value,
    field: &str,
) -> Result<String, ProviderError> {
    if let Some(pointer) = &rule.value {
        return value
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| missing_field(rule, field));
    }
    required_string(rule, value, field)
}

fn required_string(
    rule: &ResponseRule,
    value: &Value,
    field: &str,
) -> Result<String, ProviderError> {
    optional_string(rule, value, field)?.ok_or_else(|| missing_field(rule, field))
}

fn optional_string(
    rule: &ResponseRule,
    value: &Value,
    field: &str,
) -> Result<Option<String>, ProviderError> {
    if let Some(constant) = rule.constants.get(field) {
        return constant
            .as_str()
            .map(|value| Some(value.to_string()))
            .ok_or_else(|| missing_field(rule, field));
    }
    let Some(pointer) = rule.fields.get(field) else {
        return Ok(None);
    };
    match value.pointer(pointer) {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(Value::Number(value)) => Ok(Some(value.to_string())),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(missing_field(rule, field)),
    }
}

/// Reads a field that may be a single string or an array of strings, used for the search-query
/// lists providers such as Gemini report in one shot.
fn optional_string_list(
    rule: &ResponseRule,
    value: &Value,
    field: &str,
) -> Result<Vec<String>, ProviderError> {
    let selected = match rule.constants.get(field) {
        Some(constant) => constant,
        None => match rule.fields.get(field).and_then(|pointer| {
            value
                .pointer(pointer)
                .filter(|selected| !selected.is_null())
        }) {
            Some(selected) => selected,
            None => return Ok(Vec::new()),
        },
    };
    match selected {
        Value::String(value) => Ok(vec![value.clone()]),
        Value::Array(values) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| missing_field(rule, field))
            })
            .collect(),
        _ => Err(missing_field(rule, field)),
    }
}

/// Maps each entry of a `collect` array through the rule's `itemFields`.
///
/// A `url` is mandatory because a citation without one is not actionable; entries missing it are
/// skipped rather than failing the stream, so one malformed result cannot abort an answer.
fn collect_results(
    rule: &ResponseRule,
    value: &Value,
    pointer: &str,
) -> Result<Vec<ServerToolResultItem>, ProviderError> {
    let Some(selected) = value.pointer(pointer) else {
        return Ok(Vec::new());
    };
    let items = selected.as_array().ok_or_else(|| {
        ProviderError::ResponseMapping(format!(
            "response rule {} collect pointer is not an array",
            rule.id
        ))
    })?;
    let item_string = |item: &Value, field: &str| {
        rule.item_fields
            .get(field)
            .and_then(|pointer| item.pointer(pointer))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    Ok(items
        .iter()
        .filter_map(|item| {
            let url = item_string(item, "url")?;
            Some(ServerToolResultItem {
                title: item_string(item, "title").unwrap_or_else(|| url.clone()),
                url,
                source: item_string(item, "source"),
                page_age: item_string(item, "pageAge"),
                snippet: item_string(item, "snippet"),
            })
        })
        .collect())
}

fn optional_u32(
    rule: &ResponseRule,
    value: &Value,
    field: &str,
) -> Result<Option<u32>, ProviderError> {
    if let Some(constant) = rule.constants.get(field) {
        return value_to_u32(constant)
            .map(Some)
            .ok_or_else(|| missing_field(rule, field));
    }
    let Some(pointer) = rule.fields.get(field) else {
        return Ok(None);
    };
    match value.pointer(pointer) {
        Some(Value::Null) | None => Ok(None),
        Some(value) => value_to_u32(value)
            .map(Some)
            .ok_or_else(|| missing_field(rule, field)),
    }
}

/// Reads a token counter that providers report as an integer, a whole float, or a numeric string.
pub(crate) fn value_to_u32(value: &Value) -> Option<u32> {
    let integer = match value {
        Value::Number(number) => number.as_u64().or_else(|| {
            let float = number.as_f64()?;
            (float.is_finite() && float >= 0.0 && float.fract() == 0.0).then_some(float as u64)
        })?,
        Value::String(value) => value.trim().parse::<u64>().ok()?,
        _ => return None,
    };
    u32::try_from(integer).ok()
}

fn missing_field(rule: &ResponseRule, field: &str) -> ProviderError {
    ProviderError::ResponseMapping(format!(
        "response rule {} did not produce required field {field}",
        rule.id
    ))
}

fn parse_finish_reason(value: &str) -> FinishReason {
    match value {
        "stop" | "end_turn" | "completed" => FinishReason::Stop,
        "length" | "max_tokens" => FinishReason::Length,
        "tool_calls" | "tool_use" => FinishReason::ToolCalls,
        "content_filter" | "safety" => FinishReason::ContentFilter,
        _ => FinishReason::Other,
    }
}

pub fn capture_values(
    captures: &BTreeMap<String, String>,
    event_data: &Value,
) -> BTreeMap<String, Value> {
    captures
        .iter()
        .filter_map(|(name, pointer)| {
            event_data
                .pointer(pointer)
                .cloned()
                .map(|value| (name.clone(), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use crate::{
        ChatBodyEncoding, ChatResponseSpec, EventDataEncoding, EventKind, FinishReason,
        MatchCondition, ProviderEvent, ResponseRule,
    };

    use super::{DecodedSseEvent, SseDecoder, SseEventMapper};

    #[test]
    fn decodes_fragmented_crlf_multiline_and_comments() {
        let wire = b": heartbeat\r\nevent: message\r\nid: 7\r\ndata: {\"text\":\"hel\r\ndata: lo\"}\r\n\r\n";
        let mut decoder = SseDecoder::new(4096);
        let mut events = Vec::new();
        for byte in wire {
            events.extend(decoder.push(&[*byte]).unwrap());
        }
        events.extend(decoder.finish().unwrap());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("message"));
        assert_eq!(events[0].id.as_deref(), Some("7"));
        assert_eq!(events[0].data, "{\"text\":\"hel\nlo\"}");
    }

    #[test]
    fn rejects_oversized_and_invalid_utf8_events() {
        let mut decoder = SseDecoder::new(8);
        assert!(decoder.push(b"data: 123456789\n").is_err());

        let mut decoder = SseDecoder::new(128);
        assert!(
            decoder
                .push(&[b'd', b'a', b't', b'a', b':', b' ', 0xff, b'\n'])
                .is_err()
        );
    }

    #[test]
    fn maps_text_usage_and_done_events() {
        let mut mapper = SseEventMapper::new(ChatResponseSpec {
            body_encoding: ChatBodyEncoding::Sse,
            event_data_encoding: EventDataEncoding::Json,
            done_data: Some("[DONE]".to_string()),
            rules: vec![
                ResponseRule {
                    id: "text".to_string(),
                    for_each: None,
                    when: Some(MatchCondition {
                        pointer: "/type".to_string(),
                        equals: Some(json!("delta")),
                        exists: None,
                        not_null: None,
                    }),
                    emit: EventKind::TextDelta,
                    value: Some("/text".to_string()),
                    fields: BTreeMap::new(),
                    constants: BTreeMap::new(),
                    collect: None,
                    item_fields: BTreeMap::new(),
                },
                ResponseRule {
                    id: "usage".to_string(),
                    for_each: None,
                    when: Some(MatchCondition {
                        pointer: "/type".to_string(),
                        equals: Some(json!("usage")),
                        exists: None,
                        not_null: None,
                    }),
                    emit: EventKind::Usage,
                    value: None,
                    fields: BTreeMap::from([
                        ("inputTokens".to_string(), "/input".to_string()),
                        ("outputTokens".to_string(), "/output".to_string()),
                    ]),
                    constants: BTreeMap::new(),
                    collect: None,
                    item_fields: BTreeMap::new(),
                },
            ],
        });

        assert_eq!(
            mapper
                .map(&DecodedSseEvent {
                    event: None,
                    data: r#"{"type":"delta","text":"hello"}"#.to_string(),
                    id: None,
                })
                .unwrap(),
            vec![ProviderEvent::TextDelta {
                text: "hello".to_string()
            }]
        );
        assert!(matches!(
            mapper
                .map(&DecodedSseEvent {
                    event: None,
                    data: r#"{"type":"usage","input":3,"output":5}"#.to_string(),
                    id: None,
                })
                .unwrap()
                .as_slice(),
            [ProviderEvent::Usage { .. }]
        ));
        assert!(matches!(
            mapper
                .map(&DecodedSseEvent {
                    event: None,
                    data: "[DONE]".to_string(),
                    id: None,
                })
                .unwrap()
                .as_slice(),
            [ProviderEvent::Completed { .. }]
        ));
    }

    #[test]
    fn decoder_handles_valueless_fields_and_dataless_records() {
        let mut decoder = SseDecoder::new(4096);
        let events = decoder
            .push(
                concat!(
                    "retry: 500\n",
                    "unknown: ignored\n",
                    "event: ping\n",
                    "\n",
                    "data\n",
                    "\n",
                    "event: named\n",
                    "data:tight\n",
                    "\n"
                )
                .as_bytes(),
            )
            .unwrap();
        // The `event: ping` record carries no data, so it never dispatches and must not leak its
        // name onto the following record.
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event, None);
        assert_eq!(events[0].data, "");
        assert_eq!(events[1].event.as_deref(), Some("named"));
        assert_eq!(events[1].data, "tight");
    }

    #[test]
    fn decoder_keeps_the_last_event_id_and_rejects_null_bytes() {
        let mut decoder = SseDecoder::new(4096);
        let events = decoder
            .push("id: 1\ndata: a\n\nid: \0bad\ndata: b\n\n".as_bytes())
            .unwrap();
        assert_eq!(events[0].id.as_deref(), Some("1"));
        assert_eq!(events[1].id.as_deref(), Some("1"));
    }

    #[test]
    fn decoder_accounts_for_size_per_event_rather_than_per_stream() {
        let mut decoder = SseDecoder::new(32);
        for _ in 0..50 {
            assert!(decoder.push(b"data: 0123456789\n\n").is_ok());
        }
        // A single record larger than the budget still fails, even split across pushes.
        assert!(decoder.push(b"data: 0123456789").is_ok());
        assert!(decoder.push(b"0123456789012345678901234").is_err());
    }

    #[test]
    fn emits_one_completion_when_a_rule_and_the_done_sentinel_both_fire() {
        let mut mapper = SseEventMapper::new(openai_style_response());
        assert_eq!(
            mapper
                .map(&event(r#"{"choices":[{"delta":{"content":"hi"}}]}"#))
                .unwrap(),
            vec![ProviderEvent::TextDelta {
                text: "hi".to_string()
            }]
        );
        assert!(matches!(
            mapper
                .map(&event(
                    r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#
                ))
                .unwrap()
                .as_slice(),
            [ProviderEvent::Completed {
                finish_reason: Some(FinishReason::Stop)
            }]
        ));
        assert!(mapper.is_completed());
        // OpenAI-compatible providers send both a finish_reason chunk and `[DONE]`; the second one
        // must not produce a duplicate completion, and trailing chunks must be ignored.
        assert!(mapper.map(&event("[DONE]")).unwrap().is_empty());
        assert!(
            mapper
                .map(&event(r#"{"choices":[{"delta":{"content":"late"}}]}"#))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn not_null_conditions_skip_the_explicit_nulls_openai_sends_with_tool_calls() {
        let mut mapper = SseEventMapper::new(openai_style_response());
        // `content: null` accompanies every tool-call chunk. A `notNull` guard skips the rule; an
        // `exists` guard would match and then fail to read a string out of the null.
        assert!(
            mapper
                .map(&event(
                    r#"{"choices":[{"delta":{"content":null,"role":"assistant"}}]}"#
                ))
                .unwrap()
                .is_empty()
        );

        let mut exists_mapper = SseEventMapper::new(
            serde_json::from_value(json!({
                "rules": [{
                    "id": "text",
                    "when": {"pointer": "/choices/0/delta/content", "exists": true},
                    "emit": "textDelta",
                    "value": "/choices/0/delta/content"
                }]
            }))
            .unwrap(),
        );
        assert!(
            exists_mapper
                .map(&event(r#"{"choices":[{"delta":{"content":null}}]}"#))
                .is_err()
        );
    }

    #[test]
    fn tolerates_repeated_tool_ids_but_rejects_reusing_an_index() {
        let response: ChatResponseSpec = serde_json::from_value(json!({
            "rules": [{
                "id": "tool_start",
                "when": {"pointer": "/id", "notNull": true},
                "emit": "toolCallStart",
                "fields": {"id": "/id", "name": "/name", "index": "/index"}
            }]
        }))
        .unwrap();
        let mut mapper = SseEventMapper::new(response);
        assert_eq!(
            mapper
                .map(&event(r#"{"id":"call-a","name":"lookup","index":0}"#))
                .unwrap()
                .len(),
            1
        );
        // Providers that repeat the id on every fragment must not restart the call.
        assert!(
            mapper
                .map(&event(r#"{"id":"call-a","name":"lookup","index":0}"#))
                .unwrap()
                .is_empty()
        );
        assert!(
            mapper
                .map(&event(r#"{"id":"call-b","name":"lookup","index":0}"#))
                .is_err()
        );
    }

    #[test]
    fn reads_usage_counters_reported_as_floats_or_strings() {
        let response: ChatResponseSpec = serde_json::from_value(json!({
            "rules": [{
                "id": "usage",
                "emit": "usage",
                "fields": {"inputTokens": "/input", "outputTokens": "/output", "totalTokens": "/total"}
            }]
        }))
        .unwrap();
        let mut mapper = SseEventMapper::new(response);
        assert_eq!(
            mapper
                .map(&event(r#"{"input":12.0,"output":"7","total":19}"#))
                .unwrap(),
            vec![ProviderEvent::Usage {
                usage: crate::TokenUsage {
                    input_tokens: Some(12),
                    output_tokens: Some(7),
                    total_tokens: Some(19),
                    ..Default::default()
                }
            }]
        );
        // Fractional or negative counters are still rejected rather than silently truncated.
        assert!(mapper.map(&event(r#"{"input":1.5}"#)).is_err());
        assert!(mapper.map(&event(r#"{"input":-3}"#)).is_err());
    }

    /// The Anthropic content-block rules from the reference manifest, where client and server tools
    /// share one index space and stream an identical `input_json_delta`.
    fn anthropic_block_response() -> ChatResponseSpec {
        serde_json::from_value(json!({
            "rules": [
                {
                    "id": "tool_start",
                    "when": {"pointer": "/content_block/type", "equals": "tool_use"},
                    "emit": "toolCallStart",
                    "fields": {"id": "/content_block/id", "name": "/content_block/name", "index": "/index"}
                },
                {
                    "id": "search_start",
                    "when": {"pointer": "/content_block/type", "equals": "server_tool_use"},
                    "emit": "serverToolStart",
                    "fields": {"id": "/content_block/id", "name": "/content_block/name", "index": "/index"}
                },
                {
                    "id": "tool_arguments",
                    "when": {"pointer": "/delta/type", "equals": "input_json_delta"},
                    "emit": "toolArgumentsDelta",
                    "fields": {"index": "/index", "fragment": "/delta/partial_json"}
                },
                {
                    "id": "search_query",
                    "when": {"pointer": "/delta/type", "equals": "input_json_delta"},
                    "emit": "serverToolQueryDelta",
                    "fields": {"index": "/index", "fragment": "/delta/partial_json"}
                },
                {
                    "id": "search_results",
                    "when": {"pointer": "/content_block/type", "equals": "web_search_tool_result"},
                    "emit": "serverToolResult",
                    "fields": {"id": "/content_block/tool_use_id"},
                    "constants": {"name": "web_search"},
                    "collect": "/content_block/content",
                    "itemFields": {"title": "/title", "url": "/url", "pageAge": "/page_age"}
                }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn routes_input_deltas_to_the_tool_space_their_block_belongs_to() {
        let mut mapper = SseEventMapper::new(anthropic_block_response());
        // Block 0 is a server tool, block 1 a client tool. Both then stream the same delta shape,
        // and the only discriminator was in these start events.
        assert!(matches!(
            mapper
                .map(&event(
                    r#"{"index":0,"content_block":{"type":"server_tool_use","id":"srvtoolu_1","name":"web_search"}}"#
                ))
                .unwrap()
                .as_slice(),
            [ProviderEvent::ServerToolStart { name, .. }] if name == "web_search"
        ));
        assert!(matches!(
            mapper
                .map(&event(
                    r#"{"index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"lookup"}}"#
                ))
                .unwrap()
                .as_slice(),
            [ProviderEvent::ToolCallStart { .. }]
        ));

        // The server block's delta must reach only the server rule...
        assert!(matches!(
            mapper
                .map(&event(
                    r#"{"index":0,"delta":{"type":"input_json_delta","partial_json":"{\"query\":"}}"#
                ))
                .unwrap()
                .as_slice(),
            [ProviderEvent::ServerToolQueryDelta { name, fragment, .. }]
                if name == "web_search" && fragment == "{\"query\":"
        ));
        // ...and the client block's delta only the client rule.
        assert!(matches!(
            mapper
                .map(&event(
                    r#"{"index":1,"delta":{"type":"input_json_delta","partial_json":"{}"}}"#
                ))
                .unwrap()
                .as_slice(),
            [ProviderEvent::ToolArgumentsDelta { id, .. }] if id.as_str() == "toolu_1"
        ));

        // An index belonging to neither space is still a hard error.
        assert!(
            mapper
                .map(&event(
                    r#"{"index":9,"delta":{"type":"input_json_delta","partial_json":"{}"}}"#
                ))
                .is_err()
        );
    }

    #[test]
    fn groups_search_results_into_one_event_and_drops_unmapped_provider_fields() {
        let mut mapper = SseEventMapper::new(anthropic_block_response());
        let events = mapper
            .map(&event(
                &json!({
                    "index": 2,
                    "content_block": {
                        "type": "web_search_tool_result",
                        "tool_use_id": "srvtoolu_1",
                        "content": [
                            {"type": "web_search_result", "title": "Releases", "url": "https://releases.rs/", "page_age": "June 2, 2026", "encrypted_content": "AAAA"},
                            {"type": "web_search_result", "url": "https://doc.rust-lang.org/", "encrypted_content": "BBBB"},
                            {"type": "web_search_result", "title": "no url", "encrypted_content": "CCCC"}
                        ]
                    }
                })
                .to_string(),
            ))
            .unwrap();
        let [
            ProviderEvent::ServerToolResult {
                id, name, results, ..
            },
        ] = events.as_slice()
        else {
            panic!("expected one grouped result event, got {events:?}");
        };
        assert_eq!(id.as_ref().unwrap().as_str(), "srvtoolu_1");
        assert_eq!(name, "web_search");
        // Two mappable citations; the entry without a url is skipped rather than failing the stream.
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Releases");
        assert_eq!(results[0].url, "https://releases.rs/");
        assert_eq!(results[0].page_age.as_deref(), Some("June 2, 2026"));
        // A missing title falls back to the url so the caller always has something to render.
        assert_eq!(results[1].title, "https://doc.rust-lang.org/");
        // Nothing the manifest did not map survives, so provider blobs never reach the caller.
        let encoded = serde_json::to_string(&events).unwrap();
        assert!(!encoded.contains("encrypted_content"), "{encoded}");
        assert!(!encoded.contains("AAAA"), "{encoded}");
    }

    #[test]
    fn reads_grounding_metadata_as_a_query_list_plus_grouped_citations() {
        // Gemini reports the queries and the citations in one payload, so a start rule and a result
        // rule both fire on the same event.
        let response: ChatResponseSpec = serde_json::from_value(json!({
            "rules": [
                {
                    "id": "grounding_start",
                    "when": {"pointer": "/groundingMetadata/webSearchQueries", "notNull": true},
                    "emit": "serverToolStart",
                    "constants": {"name": "google_search"},
                    "fields": {"queries": "/groundingMetadata/webSearchQueries"}
                },
                {
                    "id": "grounding_results",
                    "when": {"pointer": "/groundingMetadata/groundingChunks", "notNull": true},
                    "emit": "serverToolResult",
                    "constants": {"name": "google_search"},
                    "collect": "/groundingMetadata/groundingChunks",
                    "itemFields": {"title": "/web/title", "url": "/web/uri"}
                }
            ]
        }))
        .unwrap();
        let mut mapper = SseEventMapper::new(response);
        let events = mapper
            .map(&event(
                &json!({
                    "groundingMetadata": {
                        "webSearchQueries": ["rust version", "rust release"],
                        "groundingChunks": [{"web": {"uri": "https://releases.rs/", "title": "Releases"}}]
                    }
                })
                .to_string(),
            ))
            .unwrap();
        let [
            ProviderEvent::ServerToolStart { queries, .. },
            ProviderEvent::ServerToolResult { results, .. },
        ] = events.as_slice()
        else {
            panic!("unexpected events: {events:?}");
        };
        assert_eq!(
            queries,
            &["rust version".to_string(), "rust release".to_string()]
        );
        assert_eq!(results[0].url, "https://releases.rs/");
    }

    #[test]
    fn server_tool_events_never_look_like_client_tool_calls() {
        let mut mapper = SseEventMapper::new(anthropic_block_response());
        let events = mapper
            .map(&event(
                r#"{"index":0,"content_block":{"type":"server_tool_use","id":"srvtoolu_1","name":"web_search"}}"#,
            ))
            .unwrap();
        // A caller must not be asked to execute a tool the provider already ran.
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, ProviderEvent::ToolCallStart { .. })),
            "{events:?}"
        );
    }

    fn event(data: &str) -> DecodedSseEvent {
        DecodedSseEvent {
            event: None,
            data: data.to_string(),
            id: None,
        }
    }

    fn openai_style_response() -> ChatResponseSpec {
        serde_json::from_value(json!({
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
                    "id": "completed",
                    "when": {"pointer": "/choices/0/finish_reason", "notNull": true},
                    "emit": "completed",
                    "fields": {"finishReason": "/choices/0/finish_reason"}
                }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn maps_parallel_tool_fragments_by_stable_index() {
        let response: ChatResponseSpec = serde_json::from_value(json!({
            "bodyEncoding": "sse",
            "eventDataEncoding": "json",
            "rules": [
                {
                    "id": "tool_start",
                    "forEach": "/choices/0/delta/tool_calls",
                    "when": {"pointer": "/id", "exists": true},
                    "emit": "toolCallStart",
                    "fields": {"id": "/id", "name": "/function/name", "index": "/index"}
                },
                {
                    "id": "tool_args",
                    "forEach": "/choices/0/delta/tool_calls",
                    "when": {"pointer": "/function/arguments", "exists": true},
                    "emit": "toolArgumentsDelta",
                    "fields": {"index": "/index", "fragment": "/function/arguments"}
                },
                {
                    "id": "completed",
                    "when": {"pointer": "/choices/0/finish_reason", "exists": true},
                    "emit": "completed",
                    "fields": {"finishReason": "/choices/0/finish_reason"}
                }
            ]
        }))
        .unwrap();
        let mut mapper = SseEventMapper::new(response);
        let started = mapper
            .map(&DecodedSseEvent {
                event: None,
                data: json!({
                    "choices": [{"delta": {"tool_calls": [
                        {"index": 0, "id": "call-a", "function": {"name": "a", "arguments": "{"}},
                        {"index": 1, "id": "call-b", "function": {"name": "b", "arguments": "{\"x\":"}}
                    ]}}]
                })
                .to_string(),
                id: None,
            })
            .unwrap();
        assert_eq!(started.len(), 4);

        let continued = mapper
            .map(&DecodedSseEvent {
                event: None,
                data: json!({
                    "choices": [{"delta": {"tool_calls": [
                        {"index": 0, "function": {"arguments": "}"}},
                        {"index": 1, "function": {"arguments": "1}"}}
                    ]}}]
                })
                .to_string(),
                id: None,
            })
            .unwrap();
        assert!(matches!(
            continued.as_slice(),
            [
                ProviderEvent::ToolArgumentsDelta { id: first, .. },
                ProviderEvent::ToolArgumentsDelta { id: second, .. }
            ] if first.as_str() == "call-a" && second.as_str() == "call-b"
        ));

        let completed = mapper
            .map(&DecodedSseEvent {
                event: None,
                data: json!({"choices": [{"finish_reason": "tool_calls", "delta": {}}]})
                    .to_string(),
                id: None,
            })
            .unwrap();
        assert!(matches!(
            completed.as_slice(),
            [
                ProviderEvent::ToolCallEnd { .. },
                ProviderEvent::ToolCallEnd { .. },
                ProviderEvent::Completed { .. }
            ]
        ));
    }
}
