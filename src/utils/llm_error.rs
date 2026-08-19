// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

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
