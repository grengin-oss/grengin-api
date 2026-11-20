// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::io::{self, Write};

use serde_json::{Map, Value, json};

use crate::{ConnectorError, ConnectorResponse, ResponseFormat, ResponseOverflow};

pub(crate) const DEFAULT_LIMIT: usize = 1024 * 1024;
const MAX_SOURCE_LIMIT: usize = 8 * DEFAULT_LIMIT;
const ENVELOPE_RESERVE: usize = 128;

pub(crate) struct ResponseBody {
    pub value: Value,
    pub bytes_read: usize,
    pub complete: bool,
}

impl ConnectorResponse {
    pub(crate) fn source_limit(&self) -> usize {
        self.max_source_bytes.unwrap_or(DEFAULT_LIMIT)
    }

    pub(crate) fn result_limit(&self) -> usize {
        self.max_result_bytes.unwrap_or(DEFAULT_LIMIT)
    }

    pub(crate) fn is_content(&self) -> bool {
        self.format == ResponseFormat::Text
            || self.overflow == ResponseOverflow::Truncate
            || self.source_limit() > DEFAULT_LIMIT
    }

    pub(crate) fn validate_content_policy(&self) -> Result<(), ConnectorError> {
        if !(256..=MAX_SOURCE_LIMIT).contains(&self.source_limit())
            || !(256..=DEFAULT_LIMIT).contains(&self.result_limit())
            || (self.pagination.is_some() && self.is_content())
            || (self.format == ResponseFormat::Text
                && (self.error_if.is_some() || self.error_array_pointer.is_some()))
            || (self.accept_octet_stream && self.format != ResponseFormat::Text)
        {
            return Err(ConnectorError::Manifest("invalid content response policy"));
        }
        Ok(())
    }

    pub(crate) async fn read(
        &self,
        response: &mut reqwest::Response,
        total_bytes: &mut usize,
    ) -> Result<ResponseBody, ConnectorError> {
        let text = self.format == ResponseFormat::Text;
        if text && response.status() != reqwest::StatusCode::NO_CONTENT {
            let mime = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(';').next())
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            let octet_stream =
                self.accept_octet_stream && (mime.is_empty() || mime == "application/octet-stream");
            if !mime.starts_with("text/")
                && mime != "application/vnd.github.raw+json"
                && !octet_stream
            {
                return Err(ConnectorError::Response);
            }
        }
        let clip = text && self.overflow == ResponseOverflow::Truncate;
        let limit = if clip {
            self.source_limit().min(self.result_limit())
        } else {
            self.source_limit()
        };
        if !clip
            && response
                .content_length()
                .is_some_and(|size| size > limit as u64)
        {
            return Err(ConnectorError::ResponseTooLarge);
        }
        let mut bytes = Vec::new();
        let mut complete = true;
        let mut bytes_read = 0usize;
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            if error.is_timeout() {
                ConnectorError::Timeout
            } else {
                ConnectorError::Transport
            }
        })? {
            bytes_read = bytes_read.saturating_add(chunk.len());
            *total_bytes = total_bytes.saturating_add(chunk.len());
            if *total_bytes > self.source_limit().saturating_mul(4) {
                return Err(ConnectorError::ResponseTooLarge);
            }
            let available = limit.saturating_sub(bytes.len());
            if chunk.len() > available {
                if !clip {
                    return Err(ConnectorError::ResponseTooLarge);
                }
                bytes.extend_from_slice(&chunk[..available]);
                complete = false;
                break;
            }
            bytes.extend_from_slice(&chunk);
            if clip && bytes.len() == limit {
                complete = response
                    .content_length()
                    .is_some_and(|length| length == bytes_read as u64);
                break;
            }
        }
        let value = if text {
            let decoded = match std::str::from_utf8(&bytes) {
                Ok(text) => text,
                Err(error) if !complete && error.error_len().is_none() => {
                    std::str::from_utf8(&bytes[..error.valid_up_to()])
                        .map_err(|_| ConnectorError::Response)?
                }
                Err(_) => return Err(ConnectorError::Response),
            };
            // Untyped downloads must look like text, not a binary file that happens to be UTF-8.
            if self.accept_octet_stream && decoded.contains('\0') {
                return Err(ConnectorError::Response);
            }
            Value::String(decoded.to_owned())
        } else if bytes.is_empty() {
            Value::Null
        } else {
            // JSON must be complete before applying a projection or truncating a valid value.
            serde_json::from_slice(&bytes).map_err(|_| ConnectorError::Response)?
        };
        Ok(ResponseBody {
            value,
            bytes_read,
            complete,
        })
    }

    pub(crate) fn finish(
        &self,
        value: Value,
        bytes_read: usize,
        complete: bool,
    ) -> Result<Value, ConnectorError> {
        if self.overflow == ResponseOverflow::Error {
            if encoded_len(&value, self.result_limit()).is_none() {
                return Err(ConnectorError::ResponseTooLarge);
            }
            return Ok(value);
        }
        let mut budget = self.result_limit() - ENVELOPE_RESERVE;
        let mut nodes = 32768;
        let (content, changed) = prefix(value, &mut budget, &mut nodes, 0);
        let result = json!({
            "content": content,
            "truncated": changed || !complete,
            "source_bytes_read": bytes_read,
            "source_complete": complete,
        });
        if encoded_len(&result, self.result_limit()).is_none() {
            return Err(ConnectorError::ResponseTooLarge);
        }
        Ok(result)
    }
}

struct Counter {
    count: usize,
    limit: usize,
}

impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.count = self.count.saturating_add(bytes.len());
        if self.count > self.limit {
            return Err(io::Error::other("response budget"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encoded_len(value: &Value, limit: usize) -> Option<usize> {
    let mut counter = Counter { count: 0, limit };
    serde_json::to_writer(&mut counter, value).ok()?;
    Some(counter.count)
}

fn prefix(value: Value, budget: &mut usize, nodes: &mut usize, depth: usize) -> (Value, bool) {
    if let Some(length) = encoded_len(&value, *budget) {
        *budget -= length;
        return (value, false);
    }
    if *nodes == 0 || depth >= 64 {
        *budget = budget.saturating_sub(4);
        return (Value::Null, true);
    }
    *nodes -= 1;
    match value {
        Value::String(value) => {
            let mut text = String::with_capacity(value.len().min(*budget));
            *budget = budget.saturating_sub(2);
            for ch in value.chars() {
                let size = match ch {
                    '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
                    '\u{0}'..='\u{1f}' => 6,
                    _ => ch.len_utf8(),
                };
                if size > *budget {
                    break;
                }
                *budget -= size;
                text.push(ch);
            }
            (Value::String(text), true)
        }
        Value::Array(values) => {
            let count = values.len();
            let mut result = Vec::new();
            let mut changed = false;
            *budget = budget.saturating_sub(2);
            for value in values {
                let comma = usize::from(!result.is_empty());
                if *budget < 4 + comma || *nodes == 0 {
                    break;
                }
                *budget -= comma;
                let (value, truncated) = prefix(value, budget, nodes, depth + 1);
                changed |= truncated;
                result.push(value);
            }
            changed |= result.len() != count;
            (Value::Array(result), changed)
        }
        Value::Object(values) => {
            let count = values.len();
            let mut result = Map::new();
            let mut changed = false;
            *budget = budget.saturating_sub(2);
            for (key, value) in values {
                let Some(key_len) = encoded_len(&Value::String(key.clone()), *budget) else {
                    break;
                };
                let overhead = key_len + 1 + usize::from(!result.is_empty());
                if *budget < overhead + 4 || *nodes == 0 {
                    break;
                }
                *budget -= overhead;
                let (value, truncated) = prefix(value, budget, nodes, depth + 1);
                changed |= truncated;
                result.insert(key, value);
            }
            changed |= result.len() != count;
            (Value::Object(result), changed)
        }
        _ => {
            *budget = budget.saturating_sub(4);
            (Value::Null, true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_prefix_is_valid_bounded_json_for_unicode_escapes_and_containers() {
        let policy = ConnectorResponse {
            overflow: ResponseOverflow::Truncate,
            max_result_bytes: Some(256),
            ..Default::default()
        };
        for value in [
            json!("é水🙂\n\"\u{0}".repeat(10000)),
            json!({"body":"x".repeat(10000),"other":[1,2,3]}),
            json!(vec![json!({"body":"long".repeat(500)}); 100]),
            json!([null,true,false,{"nested":vec!["text";1000]}]),
        ] {
            let result = policy.finish(value, 10000, true).unwrap();
            assert_eq!(result["truncated"], true);
            assert!(serde_json::to_vec(&result).unwrap().len() <= 256);
            assert!(serde_json::from_slice::<Value>(&serde_json::to_vec(&result).unwrap()).is_ok());
        }
        let result = policy
            .finish(json!({"complete":"small"}), 20, true)
            .unwrap();
        assert_eq!(result["content"]["complete"], "small");
        assert_eq!(result["truncated"], false);
    }
}
