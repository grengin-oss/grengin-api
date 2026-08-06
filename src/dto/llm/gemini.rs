// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{llm::prompt::Prompt, models::messages::ChatRole};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiContent {
    pub role: String,
    pub parts: Vec<GeminiPart>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GeminiPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    #[serde(rename = "functionCall", skip_serializing_if = "Option::is_none")]
    pub function_call: Option<GeminiFunctionCall>,

    #[serde(rename = "functionResponse", skip_serializing_if = "Option::is_none")]
    pub function_response: Option<GeminiFunctionResponse>,

    // Gemini 3 thinking models may include a thought signature in any part.
    // When manually continuing a tool-call conversation, you must echo this back unchanged.
    #[serde(rename = "thoughtSignature", skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiFunctionCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiFunctionResponse {
    pub id: String,
    pub name: String,
    pub response: Value,
}

pub fn prompts_to_gemini_payload(prompts: &[Prompt]) -> (Option<Value>, Vec<Value>) {
    let mut system_parts: Vec<String> = Vec::new();
    let mut contents: Vec<Value> = Vec::new();

    for prompt in prompts {
        match prompt.role {
            ChatRole::System => {
                if !prompt.text.trim().is_empty() {
                    system_parts.push(prompt.text.clone());
                }
            }
            ChatRole::User | ChatRole::Assistant => {
                let role = if matches!(prompt.role, ChatRole::Assistant) {
                    "model"
                } else {
                    "user"
                };
                contents.push(json!({
                    "role": role,
                    "parts": [{ "text": prompt.text.clone() }],
                }));
            }
            ChatRole::Tool => {}
        }
    }

    let system_instruction = if system_parts.is_empty() {
        None
    } else {
        Some(json!({
            "parts": [{ "text": system_parts.join("\n\n") }]
        }))
    };

    (system_instruction, contents)
}

pub fn normalize_gemini_parameters(schema: &Value) -> Value {
    fn sanitize_schema(schema: &Value) -> Value {
        let Value::Object(map) = schema else {
            return json!({});
        };
        let mut out = Map::new();
        let allowed_keys = [
            "type",
            "properties",
            "required",
            "items",
            "oneOf",
            "anyOf",
            "allOf",
            "description",
            "title",
            "enum",
            "default",
            "nullable",
            "format",
            "minimum",
            "maximum",
            "minLength",
            "maxLength",
            "pattern",
        ];

        for key in allowed_keys {
            if let Some(value) = map.get(key) {
                let sanitized = match key {
                    "properties" => match value {
                        Value::Object(props) => {
                            let mut props_out = Map::new();
                            for (name, prop_schema) in props {
                                props_out.insert(name.clone(), sanitize_schema(prop_schema));
                            }
                            Value::Object(props_out)
                        }
                        _ => json!({}),
                    },
                    "items" => sanitize_items(value),
                    "oneOf" | "anyOf" | "allOf" => match value {
                        Value::Array(list) => {
                            Value::Array(list.iter().map(sanitize_schema).collect())
                        }
                        _ => Value::Array(Vec::new()),
                    },
                    "required" => match value {
                        Value::Array(reqs) => Value::Array(
                            reqs.iter()
                                .filter_map(|item| {
                                    item.as_str().map(|s| Value::String(s.to_string()))
                                })
                                .collect(),
                        ),
                        _ => Value::Array(Vec::new()),
                    },
                    "type" => sanitize_type(value),
                    _ => value.clone(),
                };
                out.insert(key.to_string(), sanitized);
            }
        }

        let needs_items = out
            .get("type")
            .and_then(|value| match value {
                Value::String(s) => Some(s == "array"),
                Value::Array(list) => Some(list.iter().any(|item| item.as_str() == Some("array"))),
                _ => None,
            })
            .unwrap_or(false);
        if needs_items {
            let items = out.get("items").cloned().unwrap_or_else(|| json!({}));
            out.insert("items".to_string(), sanitize_items(&items));
        }

        Value::Object(out)
    }

    fn sanitize_items(value: &Value) -> Value {
        match value {
            Value::Object(_) => sanitize_schema(value),
            Value::Array(list) => list
                .iter()
                .find(|item| item.is_object())
                .map(sanitize_schema)
                .unwrap_or_else(|| json!({})),
            _ => json!({}),
        }
    }

    fn sanitize_type(value: &Value) -> Value {
        match value {
            Value::String(_) => value.clone(),
            Value::Array(list) => Value::Array(
                list.iter()
                    .filter_map(|item| item.as_str().map(|s| Value::String(s.to_string())))
                    .collect(),
            ),
            _ => Value::String("object".to_string()),
        }
    }

    let mut normalized = sanitize_schema(schema);
    let Value::Object(ref mut map) = normalized else {
        return json!({"type":"object","properties":{}});
    };
    if !map.contains_key("type") {
        map.insert("type".to_string(), Value::String("object".to_string()));
    }
    if !map.contains_key("properties") {
        map.insert("properties".to_string(), json!({}));
    }
    normalized
}
