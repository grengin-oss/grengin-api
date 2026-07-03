use crate::dto::chat_stream::ArtifactStreamEvent;
use uuid::Uuid;

fn extract_artifact_attr(tag: &str, attr: &str) -> Option<String> {
    let needle = format!("{}=\"", attr);
    let start = tag.find(&needle)? + needle.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

// Strip a partial </artifact...> suffix that some models leave at stream end.
fn strip_partial_artifact_close(s: &str) -> &str {
    let trimmed = s.trim_end();
    for suffix in &[
        "</artifact>", "</artifact", "</artifac", "</artifa",
        "</artif", "</arti", "</art", "</ar", "</a", "</",
    ] {
        if trimmed.ends_with(suffix) {
            return trimmed[..trimmed.len() - suffix.len()].trim_end();
        }
    }
    trimmed
}

/// Filter one streaming chunk, removing `<artifact>…</artifact>` blocks so the
/// delta stream seen by the frontend never contains raw artifact markup.
///
/// Call once per `TextDelta` chunk. `in_progress` and `buf` carry state between
/// calls — initialise both to `false` / empty before the first chunk.
/// After the stream ends, call once more with `chunk = ""` and `flush = true`
/// to drain any remaining look-ahead buffer.
pub fn filter_artifact_chunk(
    chunk: &str,
    in_progress: &mut bool,
    buf: &mut String,
    flush: bool,
) -> String {
    buf.push_str(chunk);
    let mut visible = String::new();

    loop {
        if *in_progress {
            // Searching for the closing tag.
            if let Some(close_pos) = buf.find("</artifact") {
                let after = &buf[close_pos + "</artifact".len()..];
                if let Some(gt) = after.find('>') {
                    // Full closing tag found — resume emitting after it.
                    let end = close_pos + "</artifact".len() + gt + 1;
                    *in_progress = false;
                    *buf = buf[end..].to_string();
                    // Loop: text after the close may contain another artifact.
                } else if flush {
                    // Stream ended mid-close-tag — discard the partial tag.
                    *buf = String::new();
                    break;
                } else {
                    // Close tag incomplete; keep buffered for next chunk.
                    break;
                }
            } else if flush {
                *buf = String::new();
                break;
            } else {
                // Keep the last N chars — the close tag may straddle chunks.
                let keep = buf.len().saturating_sub("</artifact>".len() - 1);
                *buf = buf[keep..].to_string();
                break;
            }
        } else {
            // Not currently inside an artifact; emit up to the next opening tag.
            if let Some(open_pos) = buf.find("<artifact ") {
                visible.push_str(&buf[..open_pos]);
                *in_progress = true;
                *buf = buf[open_pos..].to_string();
                // Loop: artifact may close in the same buffer.
            } else if flush {
                visible.push_str(buf.as_str());
                *buf = String::new();
                break;
            } else {
                // Emit everything except the last N chars (potential partial tag).
                let keep = "<artifact ".len() - 1;
                let safe_end = buf.len().saturating_sub(keep);
                visible.push_str(&buf[..safe_end]);
                *buf = buf[safe_end..].to_string();
                break;
            }
        }
    }

    visible
}

pub fn extract_artifacts(text: &str) -> Vec<ArtifactStreamEvent> {
    let mut results = Vec::new();
    let mut pos = 0;
    while pos < text.len() {
        let Some(rel_open) = text[pos..].find("<artifact ") else { break };
        let open_start = pos + rel_open;
        let after_open = &text[open_start..];
        let Some(tag_len) = after_open.find('>') else { break };
        let tag = &after_open[..=tag_len];
        let title = extract_artifact_attr(tag, "title").unwrap_or_else(|| "Untitled".to_string());
        let content_type = extract_artifact_attr(tag, "contentType")
            .unwrap_or_else(|| "text/markdown".to_string());
        let content_start = open_start + tag_len + 1;
        let remaining = &text[content_start..];

        let (content, next_pos) = if let Some(rel_close) = remaining.find("</artifact") {
            let close_tag_len = if remaining[rel_close..].starts_with("</artifact>") {
                "</artifact>".len()
            } else {
                "</artifact".len()
            };
            (remaining[..rel_close].trim().to_string(), content_start + rel_close + close_tag_len)
        } else {
            // Stream was cut off before the closing tag — strip any partial suffix.
            (strip_partial_artifact_close(remaining).to_string(), text.len())
        };

        if content.is_empty() {
            break;
        }
        pos = next_pos;
        results.push(ArtifactStreamEvent {
            id: Uuid::new_v4().to_string(),
            title,
            content_type,
            content,
        });
    }
    results
}
