// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{borrow::Cow, collections::HashMap};

use serde_json::{Map, Value};

use crate::dto::chat_stream::{ConnectorCallSummary, ConnectorResultSummary};

const MAX_ITEMS: usize = 5;
const MAX_ITEM_CHARS: usize = 120;
const MAX_TARGET_CHARS: usize = 80;
// Only the backend's own error markers pick a message; error text itself never reaches the browser,
// since a vendor error can quote a message body or a credential.
const ERRORS: [(&str, &str); 9] = [
    ("oauth connection required", "Sign-in required"),
    ("reconnect_required", "Sign-in required"),
    (
        "oauth token endpoint unreachable",
        "Connector unavailable, try again",
    ),
    ("oauth", "Sign-in failed"),
    ("access denied", "Access denied"),
    ("read_only", "Access denied"),
    ("not connected", "Connector not connected"),
    ("deadline exceeded", "Timed out"),
    ("did not complete", "Did not finish"),
];
const UNKNOWN_CONNECTOR: &str = "Connector";
// Server-side search tools as the providers and the stream name them.
const NATIVE_WEB_SEARCH: [&str; 6] = [
    "web_search",
    "web_search_call",
    "web_search_20250305",
    "web_search_preview",
    "google_search",
    "browser_search",
];

// Arguments worth showing; bodies, message text and IDs are deliberately absent.
const TARGET_KEYS: [&str; 16] = [
    "query",
    "q",
    "search",
    "keywords",
    "keyword",
    "jql",
    "cql",
    "subject",
    "title",
    "name",
    "filename",
    "file_name",
    "fileName",
    "path",
    "channel",
    "to",
];
// Fields that name an item without exposing its content.
const TITLE_KEYS: [&str; 9] = [
    "subject",
    "title",
    "summary",
    "name",
    "displayName",
    "display_name",
    "fileName",
    "filename",
    "file_name",
];
const LIST_KEYS: [&str; 22] = [
    "value",
    "items",
    "results",
    "messages",
    "threads",
    "files",
    "events",
    "issues",
    "pages",
    "documents",
    "contacts",
    "tickets",
    "records",
    "entries",
    "rows",
    "data",
    "drafts",
    "labels",
    "channels",
    "users",
    "tasks",
    "notes",
];
const GENERIC_LISTS: [&str; 6] = ["value", "data", "results", "records", "entries", "rows"];

pub fn call_summary(
    connector: &str,
    tool_name: &str,
    arguments: Option<&Value>,
) -> ConnectorCallSummary {
    ConnectorCallSummary {
        connector: connector.to_string(),
        action: humanize(tool_name),
        target: arguments.and_then(target),
    }
}

pub fn result_summary(output: &Value, is_error: bool) -> ConnectorResultSummary {
    let output = decoded(output);
    if is_error {
        return summary(error_text(&output));
    }
    if let Some((noun, list)) = known_listing(&output) {
        return listing(noun, list);
    }
    if is_empty(&output) {
        return summary("No results".to_string());
    }
    if let Some(title) = title(&output) {
        return summary(title);
    }
    match generic_listing(&output) {
        Some((noun, list)) => listing(noun, list),
        None => summary("Done".to_string()),
    }
}

// Chat history stored before summaries existed still holds raw connector arguments and output.
pub fn redact_history(calls: Vec<Value>, results: Vec<Value>) -> (Vec<Value>, Vec<Value>) {
    let mut connectors: HashMap<String, (String, String)> = HashMap::new();
    let results = results
        .into_iter()
        .map(|mut result| {
            if let Some(object) = result.as_object_mut() {
                redact_result(object, &mut connectors);
            }
            result
        })
        .collect();
    let calls = calls
        .into_iter()
        .map(|mut call| {
            if let Some(object) = call.as_object_mut() {
                redact_call(object, &connectors);
            }
            call
        })
        .collect();
    (calls, results)
}

// Fails closed: every stored call and result except web search is treated as a connector's, since
// the runtime accepts several aliases for a connector tool (plain, unhashed, request-local) and
// older replies have no marker; web search is the only other tool a reply records.
fn redact_result(
    result: &mut Map<String, Value>,
    connectors: &mut HashMap<String, (String, String)>,
) {
    if is_web_search(result) {
        return;
    }
    let mcp = result
        .get_mut("mcp")
        .and_then(Value::as_object_mut)
        .map(|mcp| {
            mcp.remove("preview");
            (
                text_field(mcp, "server_name").unwrap_or_else(|| UNKNOWN_CONNECTOR.to_string()),
                text_field(mcp, "tool_name"),
            )
        });
    if let Some(id) = text_field(result, "tool_id") {
        let (server, tool) = mcp.unwrap_or_else(|| (UNKNOWN_CONNECTOR.to_string(), None));
        let tool = tool
            .unwrap_or_else(|| tool_label(&text_field(result, "tool_name").unwrap_or_default()));
        connectors.insert(id, (server, tool));
    }
    let is_error = result.get("status").and_then(Value::as_str) == Some("error");
    result.remove("web_search");
    result.insert("kind".into(), Value::from("other"));
    let output = result.remove("output");
    if !result.contains_key("summary") {
        let summary = result_summary(&output.unwrap_or_default(), is_error);
        result.insert("summary".into(), to_value(&summary));
    }
}

fn redact_call(call: &mut Map<String, Value>, connectors: &HashMap<String, (String, String)>) {
    if is_web_search(call) {
        return;
    }
    let (server, tool) = text_field(call, "tool_id")
        .and_then(|id| connectors.get(&id).cloned())
        .unwrap_or_else(|| {
            let name = text_field(call, "tool_name").unwrap_or_default();
            (UNKNOWN_CONNECTOR.to_string(), tool_label(&name))
        });
    call.remove("input_text");
    call.remove("web_search");
    call.insert("kind".into(), Value::from("other"));
    let input = call.remove("input");
    if !call.contains_key("summary") {
        let arguments = input.as_ref().and_then(|input| input.get("value"));
        call.insert(
            "summary".into(),
            to_value(&call_summary(&server, &tool, arguments)),
        );
    }
}

// Only the providers' own web search survives redaction, recognised by its exact tool name; an
// `mcp` marker wins, since a connector may be named like web search.
fn is_web_search(entry: &Map<String, Value>) -> bool {
    !entry.contains_key("mcp")
        && text_field(entry, "tool_name")
            .is_some_and(|name| NATIVE_WEB_SEARCH.contains(&name.as_str()))
}

// The tool's own name inside `mcp__<server>__<tool>[__<hash>]` or the gateway's `tool_<n>_<tool>`.
fn tool_label(name: &str) -> String {
    if let Some(rest) = name.strip_prefix("mcp__") {
        let mut parts: Vec<&str> = rest.split("__").collect();
        if parts.len() >= 3
            && parts
                .last()
                .is_some_and(|hash| hash.chars().all(|character| character.is_ascii_hexdigit()))
        {
            parts.pop();
        }
        if parts.len() >= 2 {
            return parts[1..].join("__");
        }
    }
    if let Some(rest) = name.strip_prefix("tool_") {
        let digits = rest
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(rest.len());
        if let Some(tool) = rest[digits..].strip_prefix('_')
            && digits > 0
            && !tool.is_empty()
        {
            return tool.to_string();
        }
    }
    name.to_string()
}

fn summary(text: String) -> ConnectorResultSummary {
    ConnectorResultSummary {
        text,
        item_count: None,
        items: Vec::new(),
    }
}

fn listing(noun: &str, list: &[Value]) -> ConnectorResultSummary {
    ConnectorResultSummary {
        text: count_text(list.len(), noun),
        item_count: Some(list.len()),
        items: list.iter().filter_map(title).take(MAX_ITEMS).collect(),
    }
}

// A tool without structured content answers in MCP text parts, usually holding JSON.
fn decoded(output: &Value) -> Cow<'_, Value> {
    let Some(parts) = output.get("content").and_then(Value::as_array) else {
        return Cow::Borrowed(output);
    };
    match parts
        .iter()
        .find_map(|part| part.get("text").and_then(Value::as_str))
    {
        Some(text) => Cow::Owned(
            serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string())),
        ),
        None => Cow::Owned(Value::Null),
    }
}

fn error_text(output: &Value) -> String {
    if output.get("authorization_url").is_some() {
        return "Sign-in required".to_string();
    }
    let message = output
        .get("error")
        .and_then(|error| {
            error
                .as_str()
                .or_else(|| error.get("message").and_then(Value::as_str))
        })
        .or_else(|| output.get("message").and_then(Value::as_str))
        .or_else(|| output.as_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    ERRORS
        .iter()
        .find(|(marker, _)| message.contains(marker))
        .map_or("The tool returned an error", |(_, text)| text)
        .to_string()
}

fn known_listing(output: &Value) -> Option<(&'static str, &[Value])> {
    if let Some(list) = output.as_array() {
        return Some(("results", list));
    }
    let object = output.as_object()?;
    LIST_KEYS.iter().find_map(|key| {
        object
            .get(*key)
            .and_then(Value::as_array)
            .map(|list| (noun(key), list.as_slice()))
    })
}

fn generic_listing(output: &Value) -> Option<(&'static str, &[Value])> {
    output.as_object()?.values().find_map(|value| {
        value
            .as_array()
            .filter(|list| list.first().is_some_and(Value::is_object))
            .map(|list| ("results", list.as_slice()))
    })
}

fn noun(key: &'static str) -> &'static str {
    if GENERIC_LISTS.contains(&key) {
        "results"
    } else {
        key
    }
}

fn count_text(count: usize, noun: &str) -> String {
    match (count, noun.strip_suffix('s')) {
        (0, _) => "No results".to_string(),
        (1, Some(singular)) => format!("1 {singular}"),
        _ => format!("{count} {noun}"),
    }
}

fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Object(object) => object.is_empty(),
        Value::Array(list) => list.is_empty(),
        Value::String(text) => text.trim().is_empty(),
        _ => false,
    }
}

fn title(item: &Value) -> Option<String> {
    let object = item.as_object()?;
    TITLE_KEYS
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .filter(|text| !text.trim().is_empty())
        .map(str::to_string)
        .or_else(|| subject_header(object))
        .or_else(|| object.get("fields").and_then(title))
        .map(|text| clip(&text, MAX_ITEM_CHARS))
}

// Gmail keeps the subject among the message headers.
fn subject_header(object: &Map<String, Value>) -> Option<String> {
    let headers = object.get("headers").or_else(|| {
        object
            .get("payload")
            .and_then(|payload| payload.get("headers"))
    })?;
    headers.as_array()?.iter().find_map(|header| {
        header
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| name.eq_ignore_ascii_case("subject"))
            .and_then(|_| header.get("value").and_then(Value::as_str))
            .map(str::to_string)
    })
}

fn target(arguments: &Value) -> Option<String> {
    let object = arguments.as_object()?;
    TARGET_KEYS.iter().find_map(|key| {
        let text = match object.get(*key)? {
            Value::String(text) => text.clone(),
            Value::Array(list) => list
                .iter()
                .filter_map(Value::as_str)
                .take(3)
                .collect::<Vec<_>>()
                .join(", "),
            _ => return None,
        };
        let text = clip(&text, MAX_TARGET_CHARS);
        (!text.is_empty()).then_some(text)
    })
}

fn humanize(tool_name: &str) -> String {
    let mut words = String::new();
    let mut previous_lower = false;
    for character in tool_name.chars() {
        if character == '_' || character == '-' || character == '.' {
            words.push(' ');
            previous_lower = false;
            continue;
        }
        if character.is_uppercase() && previous_lower {
            words.push(' ');
        }
        previous_lower = character.is_lowercase() || character.is_ascii_digit();
        words.extend(character.to_lowercase());
    }
    let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut characters = words.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => tool_name.to_string(),
    }
}

fn clip(text: &str, limit: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= limit {
        return text;
    }
    let mut clipped: String = text.chars().take(limit - 1).collect();
    clipped.push('…');
    clipped
}

fn text_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    object.get(key).and_then(Value::as_str).map(str::to_string)
}

fn to_value<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn email_results_show_subjects_never_bodies() {
        let output = json!({"value": [
            {"subject": "Invoice #123", "body": {"content": "secret body"}, "bodyPreview": "secret"},
            {"subject": "Re: Payment", "body": {"content": "another body"}},
        ]});
        let summary = result_summary(&output, false);
        assert_eq!(summary.text, "2 results");
        assert_eq!(summary.item_count, Some(2));
        assert_eq!(summary.items, ["Invoice #123", "Re: Payment"]);
        assert!(!serde_json::to_string(&summary).unwrap().contains("secret"));
    }

    #[test]
    fn gmail_subjects_come_from_headers_and_snippets_stay_hidden() {
        let message = json!({"id": "m1", "snippet": "secret snippet", "payload": {"headers": [
            {"name": "From", "value": "a@example.com"},
            {"name": "Subject", "value": "Quarterly report"},
        ]}});
        assert_eq!(result_summary(&message, false).text, "Quarterly report");
        let thread = json!({"id": "t1", "messages": [message]});
        let summary = result_summary(&thread, false);
        assert_eq!(
            (summary.text.as_str(), summary.items.as_slice()),
            ("1 message", ["Quarterly report".to_string()].as_slice())
        );
        assert!(!serde_json::to_string(&summary).unwrap().contains("secret"));
    }

    #[test]
    fn listings_without_titles_only_count() {
        let summary = result_summary(
            &json!({"messages": [{"id": "1"}, {"id": "2"}], "nextPageToken": "x"}),
            false,
        );
        assert_eq!(summary.text, "2 messages");
        assert!(summary.items.is_empty());
        assert_eq!(
            result_summary(
                &json!({"issues": [{"key": "P-1", "fields": {"summary": "Fix login"}}]}),
                false
            )
            .items,
            ["Fix login"]
        );
        assert_eq!(
            result_summary(&json!({"files": []}), false).text,
            "No results"
        );
    }

    #[test]
    fn opaque_text_and_mcp_parts_never_leak_content() {
        let text = json!({"content": [{"type": "text", "text": "the whole document body"}]});
        assert_eq!(result_summary(&text, false).text, "Done");
        let json_text =
            json!({"content": [{"type": "text", "text": "{\"files\":[{\"name\":\"plan.pdf\"}]}"}]});
        assert_eq!(result_summary(&json_text, false).items, ["plan.pdf"]);
        assert_eq!(
            result_summary(&json!("a document body"), false).text,
            "Done"
        );
        assert_eq!(result_summary(&Value::Null, false).text, "No results");
    }

    #[test]
    fn errors_are_short_and_sign_in_is_named() {
        let oauth = json!({"error": "mcp oauth connection required", "is_error": true, "authorization_url": "https://x"});
        assert_eq!(result_summary(&oauth, true).text, "Sign-in required");
        assert_eq!(
            result_summary(&json!({}), true).text,
            "The tool returned an error"
        );
        assert_eq!(
            result_summary(
                &json!({"error": "mcp tool execution deadline exceeded"}),
                true
            )
            .text,
            "Timed out"
        );
        assert_eq!(
            result_summary(
                &json!({"error": "mcp access denied or configuration changed"}),
                true
            )
            .text,
            "Access denied"
        );
        assert_eq!(
            result_summary(&json!({"error": "Tool execution did not complete"}), true).text,
            "Did not finish"
        );
    }

    #[test]
    fn error_text_never_reaches_the_browser() {
        for output in [
            json!({"error": "Rejected message body: CONFIDENTIAL_BODY"}),
            json!({"error": {"message": "invalid token sk-SECRET123"}}),
            json!({"content": [{"type": "text", "text": "upstream said CONFIDENTIAL_BODY"}], "isError": true}),
            json!("Bearer SECRET_TOKEN rejected"),
        ] {
            let summary = serde_json::to_string(&result_summary(&output, true)).unwrap();
            assert!(
                !summary.contains("CONFIDENTIAL") && !summary.contains("SECRET"),
                "{summary}"
            );
        }
    }

    #[test]
    fn tool_labels_cover_every_alias_the_runtime_accepts() {
        for (name, label) in [
            ("mcp__ab12cd34__search_threads__9f8e7d6c", "search_threads"),
            ("mcp__ab12__send_mail", "send_mail"),
            ("tool_3_send_mail", "send_mail"),
            ("send_mail", "send_mail"),
            ("tool_call", "tool_call"),
            ("tool_x_y", "tool_x_y"),
        ] {
            assert_eq!(tool_label(name), label, "{name}");
        }
    }

    #[test]
    fn unfinished_connector_calls_in_history_lose_their_arguments() {
        let secret = |id: &str, name: &str| {
            json!({"tool_name": name, "tool_id": id, "kind": "other",
                   "input": {"type": "json", "value": {"to": "b@example.com", "body": "CONFIDENTIAL_BODY"}}})
        };
        let calls = vec![
            json!({"tool_name": "tool_0_send_mail", "tool_id": "c1", "input_text": "{\"body\":\"CONFIDENTIAL"}),
            secret("c2", "mcp__ab12__send_mail__ff00"),
            secret("c3", "mcp__ab12__send_mail"),
            secret("c4", "send_mail"),
            json!({"tool_name": "tool_call", "tool_id": "c5", "input_text": "CONFIDENTIAL"}),
            json!({"tool_name": "web_search_call", "tool_id": "w1", "kind": "web_search",
                   "web_search": {"query": "kept query"}}),
        ];
        let results = vec![
            json!({"tool_name": "tool_0_send_mail", "tool_id": "c1", "status": "error",
                   "output": {"error": "Tool execution did not complete"}}),
            json!({"tool_name": "send_mail", "tool_id": "c4", "status": "success",
                   "output": {"subject": "Hi", "body": "CONFIDENTIAL_BODY"}}),
        ];
        let (calls, results) = redact_history(calls, results);
        assert!(
            !serde_json::to_string(&(&calls, &results))
                .unwrap()
                .contains("CONFIDENTIAL")
        );
        for index in [1, 2, 3] {
            assert_eq!(
                calls[index]["summary"],
                json!({"connector": "Connector", "action": "Send mail", "target": "b@example.com"})
            );
        }
        assert_eq!(results[1]["summary"]["text"], "Hi");
        assert_eq!(calls[5]["web_search"]["query"], "kept query");
    }

    #[test]
    fn connectors_named_like_web_search_are_still_redacted() {
        let calls = vec![
            json!({"tool_name": "mcp__ab12__web_search_messages__ff00", "tool_id": "c1", "kind": "web_search",
                   "web_search": {"query": "CONFIDENTIAL"}, "input": {"type": "json", "value": {"query": "invoices", "body": "CONFIDENTIAL"}}}),
            json!({"tool_name": "tool_0_web_search_messages", "tool_id": "c2", "kind": "web_search",
                   "input_text": "CONFIDENTIAL"}),
            json!({"tool_name": "web_search_messages", "tool_id": "c3", "kind": "web_search",
                   "input": {"type": "json", "value": {"body": "CONFIDENTIAL"}}}),
            json!({"tool_name": "web_search_call", "tool_id": "w1", "kind": "web_search",
                   "web_search": {"query": "kept query"}}),
        ];
        let results = vec![
            json!({"tool_name": "web_search_call", "tool_id": "c1", "status": "success",
                   "mcp": {"server_name": "Gmail", "tool_name": "web_search_messages", "preview": {"text": "CONFIDENTIAL"}},
                   "output": {"messages": [{"subject": "Hi", "body": "CONFIDENTIAL"}]}}),
            json!({"tool_name": "web_search_call", "tool_id": "w1", "kind": "web_search", "status": "success",
                   "web_search": {"results": [{"title": "kept result"}]}}),
        ];
        let (calls, results) = redact_history(calls, results);
        assert!(
            !serde_json::to_string(&(&calls, &results))
                .unwrap()
                .contains("CONFIDENTIAL")
        );
        assert_eq!(
            calls[0]["summary"],
            json!({"connector": "Gmail", "action": "Web search messages", "target": "invoices"})
        );
        assert_eq!(
            (&calls[0]["kind"], &results[0]["kind"]),
            (&json!("other"), &json!("other"))
        );
        assert_eq!(calls[3]["web_search"]["query"], "kept query");
        assert_eq!(
            results[1]["web_search"]["results"][0]["title"],
            "kept result"
        );
    }

    #[test]
    fn calls_name_the_action_and_a_safe_target_only() {
        let summary = call_summary(
            "Gmail",
            "search_threads",
            Some(&json!({"query": "from:acme invoice", "maxResults": 5})),
        );
        assert_eq!(
            (summary.connector.as_str(), summary.action.as_str()),
            ("Gmail", "Search threads")
        );
        assert_eq!(summary.target.as_deref(), Some("from:acme invoice"));
        let send = call_summary(
            "Outlook",
            "sendMail",
            Some(&json!({"to": ["b@example.com"], "body": "secret"})),
        );
        assert_eq!(
            (send.action.as_str(), send.target.as_deref()),
            ("Send mail", Some("b@example.com"))
        );
        assert_eq!(
            call_summary("Drive", "get_file", Some(&json!({"fileId": "abc"}))).target,
            None
        );
    }

    #[test]
    fn stored_history_drops_connector_arguments_and_output_only() {
        let calls = vec![
            json!({"tool_name": "tool_0_search_threads", "tool_id": "c1", "input_text": "{\"query\":\"inv"}),
            json!({"tool_name": "tool_0_search_threads", "tool_id": "c1", "input": {"type": "json", "value": {"query": "invoice", "body": "secret"}}}),
            json!({"tool_name": "web_search_call", "tool_id": "w1", "input": {"type": "json", "value": {"query": "news"}}}),
        ];
        let results = vec![
            json!({"tool_id": "c1", "status": "success", "output": {"messages": [{"subject": "Hi", "body": "secret"}]},
                   "mcp": {"server_name": "Gmail", "tool_name": "search_threads", "preview": {"text": "secret"}}}),
            json!({"tool_name": "web_search_call", "tool_id": "w1", "kind": "web_search", "status": "success", "output": {"kept": true}}),
        ];
        let (calls, results) = redact_history(calls, results);
        let stored = serde_json::to_string(&(&calls, &results)).unwrap();
        assert!(!stored.contains("secret"));
        assert_eq!(
            calls[1]["summary"],
            json!({"connector": "Gmail", "action": "Search threads", "target": "invoice"})
        );
        assert_eq!(results[0]["summary"]["items"], json!(["Hi"]));
        assert_eq!(calls[2]["input"]["value"]["query"], "news");
        assert_eq!(results[1]["output"], json!({"kept": true}));
        assert_eq!(
            redact_history(calls.clone(), results.clone()),
            (calls, results)
        );
    }
}
