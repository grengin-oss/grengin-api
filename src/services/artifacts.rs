use uuid::Uuid;

pub const ARTIFACT_SYSTEM_HINT: &str = "When producing a standalone artifact — a complete HTML page, \
full Markdown document, or self-contained code file — wrap it in XML tags:\n\n\
<artifact type=\"text/html\" title=\"Descriptive title\">\n\
...full content here...\n\
</artifact>\n\n\
Supported types: text/html, text/markdown, text/plain, application/javascript, text/css.\n\
Only use this for complete standalone files, not for short inline snippets.\n\
Before the tag, write one sentence describing what you are building.";

pub enum ArtifactParseEvent {
    Start { id: String, title: String, content_type: String },
    Delta { id: String, chunk: String },
    End { id: String },
}

pub struct ArtifactParser {
    buffer: String,
    current_id: Option<String>,
}

impl ArtifactParser {
    pub fn new() -> Self {
        Self { buffer: String::new(), current_id: None }
    }

    pub fn push(&mut self, text: &str) -> (String, Vec<ArtifactParseEvent>) {
        self.buffer.push_str(text);
        self.process()
    }

    pub fn flush(&mut self) -> (String, Vec<ArtifactParseEvent>) {
        let mut events = Vec::new();
        if let Some(id) = self.current_id.take() {
            let chunk = std::mem::take(&mut self.buffer);
            let chunk = chunk.trim_end_matches('\n').to_string();
            if !chunk.is_empty() {
                events.push(ArtifactParseEvent::Delta { id: id.clone(), chunk });
            }
            events.push(ArtifactParseEvent::End { id });
            (String::new(), events)
        } else {
            let rest = std::mem::take(&mut self.buffer);
            (rest, events)
        }
    }

    fn process(&mut self) -> (String, Vec<ArtifactParseEvent>) {
        let mut passthrough = String::new();
        let mut events = Vec::new();

        loop {
            if self.current_id.is_none() {
                // Outside: scan for opening tag
                if let Some(open_start) = self.buffer.find("<artifact") {
                    passthrough.push_str(&self.buffer[..open_start]);
                    self.buffer = self.buffer[open_start..].to_string();

                    if let Some(tag_end_rel) = self.buffer.find('>') {
                        let tag_end = tag_end_rel + 1;
                        let opening_tag = self.buffer[..tag_end].to_string();
                        self.buffer = self.buffer[tag_end..].to_string();

                        if self.buffer.starts_with('\n') {
                            self.buffer = self.buffer[1..].to_string();
                        }

                        let title = extract_attr(&opening_tag, "title")
                            .unwrap_or_else(|| "Untitled".to_string());
                        let content_type = extract_attr(&opening_tag, "type")
                            .unwrap_or_else(|| "text/plain".to_string());
                        let id = Uuid::new_v4().to_string();

                        events.push(ArtifactParseEvent::Start {
                            id: id.clone(),
                            title,
                            content_type,
                        });
                        self.current_id = Some(id);
                        // Continue loop to drain buffered content
                    } else {
                        // Opening tag not yet complete — hold
                        break;
                    }
                } else {
                    // No <artifact — pass through, holding the tail in case it's a partial tag.
                    // Work on bytes to avoid slicing inside multi-byte UTF-8 characters.
                    let hold = b"<artifact".len() - 1; // 8
                    let buf_bytes = self.buffer.as_bytes();
                    if buf_bytes.len() > hold {
                        let artifact_pfx = b"<artifact";
                        // Byte slice — never panics regardless of multi-byte chars.
                        let tail_bytes = &buf_bytes[buf_bytes.len() - hold..];
                        let mut found_partial = false;
                        for n in (1..=tail_bytes.len()).rev() {
                            if artifact_pfx.starts_with(&tail_bytes[tail_bytes.len() - n..]) {
                                // The n matching bytes are all ASCII (prefix of "<artifact"),
                                // so split = buf_bytes.len() - n is a valid char boundary.
                                let split = buf_bytes.len() - n;
                                passthrough.push_str(&self.buffer[..split]);
                                self.buffer = self.buffer[split..].to_string();
                                found_partial = true;
                                break;
                            }
                        }
                        if !found_partial {
                            passthrough.push_str(&self.buffer);
                            self.buffer.clear();
                        }
                    }
                    break;
                }
            } else {
                // Inside: stream content until closing tag
                let id = self.current_id.clone().unwrap();

                if let Some(close_rel) = self.buffer.find("</artifact>") {
                    let chunk = self.buffer[..close_rel].trim_end_matches('\n').to_string();
                    if !chunk.is_empty() {
                        events.push(ArtifactParseEvent::Delta { id: id.clone(), chunk });
                    }
                    self.buffer = self.buffer[close_rel + "</artifact>".len()..].to_string();
                    events.push(ArtifactParseEvent::End { id });
                    self.current_id = None;
                    // Continue loop to process text after closing tag
                } else {
                    // No closing tag yet — emit what we safely can, hold the tail.
                    // Use floor_char_boundary so we never split inside a multi-byte char.
                    let hold = b"</artifact>".len() - 1; // 10
                    if self.buffer.len() > hold {
                        let split = floor_char_boundary(&self.buffer, self.buffer.len() - hold);
                        let chunk = self.buffer[..split].to_string();
                        self.buffer = self.buffer[split..].to_string();
                        if !chunk.is_empty() {
                            events.push(ArtifactParseEvent::Delta { id, chunk });
                        }
                    }
                    break;
                }
            }
        }

        (passthrough, events)
    }
}

/// Returns the largest index ≤ i that is a valid UTF-8 char boundary in s.
fn floor_char_boundary(s: &str, i: usize) -> usize {
    let mut j = i.min(s.len());
    while j > 0 && !s.is_char_boundary(j) {
        j -= 1;
    }
    j
}

fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let pattern = format!("{}={}", attr, quote);
        if let Some(start) = tag.find(&pattern) {
            let after = &tag[start + pattern.len()..];
            if let Some(end) = after.find(quote) {
                return Some(after[..end].to_string());
            }
        }
    }
    None
}
