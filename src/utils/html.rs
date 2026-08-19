// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

/// Extract visible text from an HTML document, stripping all tags.
/// Collapses whitespace and decodes common HTML entities.
pub fn extract_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut rest = html;

    loop {
        // Find next tag or entity marker
        let next = rest.find(|c| c == '<' || c == '&');
        let Some(pos) = next else {
            out.push_str(rest);
            break;
        };

        out.push_str(&rest[..pos]);
        rest = &rest[pos..];

        if rest.starts_with("<!--") {
            // HTML comment — skip to -->
            if let Some(end) = rest.find("-->") {
                rest = &rest[end + 3..];
            } else {
                break;
            }
        } else if rest.starts_with('<') {
            rest = &rest[1..]; // consume <
            let Some(gt) = rest.find('>') else {
                out.push('<');
                continue;
            };
            let tag_content = &rest[..gt];
            let is_close = tag_content.starts_with('/');
            let name_part = if is_close {
                &tag_content[1..]
            } else {
                tag_content
            };
            let tag_name: String = name_part
                .split(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            rest = &rest[gt + 1..];

            // Block-level tags separate adjacent words
            if matches!(
                tag_name.as_str(),
                "p" | "div"
                    | "br"
                    | "li"
                    | "td"
                    | "th"
                    | "tr"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "blockquote"
                    | "pre"
                    | "section"
                    | "article"
            ) {
                out.push(' ');
            }

            // Skip script/style content
            if !is_close && matches!(tag_name.as_str(), "script" | "style") {
                let close = if tag_name == "script" {
                    "</script"
                } else {
                    "</style"
                };
                let lower = rest.to_ascii_lowercase();
                if let Some(p) = lower.find(close) {
                    rest = &rest[p + close.len()..];
                    if let Some(gt2) = rest.find('>') {
                        rest = &rest[gt2 + 1..];
                    }
                } else {
                    rest = "";
                }
            }
        } else if rest.starts_with('&') {
            rest = &rest[1..]; // consume &
            // Entity name is at most ~10 ASCII chars before ;
            if let Some(semi) = rest.find(';').filter(|&p| p <= 12) {
                let entity = &rest[..semi];
                let replacement = match entity {
                    "amp" => Some("&"),
                    "lt" => Some("<"),
                    "gt" => Some(">"),
                    "quot" => Some("\""),
                    "apos" => Some("'"),
                    "nbsp" => Some(" "),
                    _ => None,
                };
                if let Some(r) = replacement {
                    out.push_str(r);
                } else {
                    out.push('&');
                    out.push_str(entity);
                    out.push(';');
                }
                rest = &rest[semi + 1..];
            } else {
                out.push('&');
            }
        }
    }

    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_simple_tags() {
        assert_eq!(extract_text("<p>Hello <b>world</b></p>"), "Hello world");
    }

    #[test]
    fn ignores_script_content() {
        assert_eq!(
            extract_text("<p>Text</p><script>var x = 1;</script><p>More</p>"),
            "Text More"
        );
    }

    #[test]
    fn ignores_style_content() {
        assert_eq!(
            extract_text("<style>body { color: red; }</style><p>Visible</p>"),
            "Visible"
        );
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(extract_text("<p>&amp; &lt; &gt; &quot;</p>"), "& < > \"");
    }

    #[test]
    fn strips_html_comments() {
        assert_eq!(extract_text("<!-- ignored --><p>visible</p>"), "visible");
    }

    #[test]
    fn collapses_whitespace() {
        assert_eq!(extract_text("<p>  spaced   out  </p>"), "spaced out");
    }

    #[test]
    fn empty_input() {
        assert_eq!(extract_text(""), "");
    }

    #[test]
    fn plain_text_passthrough() {
        assert_eq!(extract_text("no tags here"), "no tags here");
    }
}
