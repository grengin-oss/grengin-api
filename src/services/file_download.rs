// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::models::files;
use axum::{
    body::Body,
    http::{
        HeaderValue, Response,
        header::{CONTENT_DISPOSITION, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS},
    },
};

const FALLBACK_CONTENT_TYPE: &str = "application/octet-stream";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InlineType {
    Png,
    Jpeg,
    Gif,
    Webp,
    Pdf,
}

impl InlineType {
    fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(Self::Jpeg)
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            Some(Self::Gif)
        } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
            Some(Self::Webp)
        } else if bytes.starts_with(b"%PDF-") {
            Some(Self::Pdf)
        } else {
            None
        }
    }

    fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
            Self::Pdf => "application/pdf",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Disposition {
    Inline,
    Attachment,
}

impl Disposition {
    fn as_str(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::Attachment => "attachment",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct DownloadPolicy {
    disposition: Disposition,
    content_type: HeaderValue,
}

impl DownloadPolicy {
    // The declared type comes from the uploader, so only the stored bytes can make a file inline.
    fn for_file(declared_content_type: &str, bytes: &[u8]) -> Self {
        match InlineType::sniff(bytes) {
            Some(inline) => Self {
                disposition: Disposition::Inline,
                content_type: HeaderValue::from_static(inline.content_type()),
            },
            None => Self {
                disposition: Disposition::Attachment,
                content_type: HeaderValue::from_str(declared_content_type.trim())
                    .ok()
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| HeaderValue::from_static(FALLBACK_CONTENT_TYPE)),
            },
        }
    }
}

fn content_disposition(disposition: Disposition, file_name: &str) -> HeaderValue {
    if file_name.is_empty() {
        return HeaderValue::from_static(disposition.as_str());
    }
    let ascii_fallback = file_name
        .chars()
        .map(|character| match character {
            ' ' => ' ',
            '"' | '\\' | '%' => '_',
            character if character.is_ascii_graphic() => character,
            _ => '_',
        })
        .collect::<String>();
    let value = format!(
        "{}; filename=\"{ascii_fallback}\"; filename*=UTF-8''{}",
        disposition.as_str(),
        encode_rfc5987(file_name)
    );
    HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

fn encode_rfc5987(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'!' | b'#' | b'$' | b'&' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
            )
        {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

pub fn download_response(model: &files::Model, bytes: Vec<u8>) -> Response<Body> {
    let policy = DownloadPolicy::for_file(&model.content_type, &bytes);
    let disposition = content_disposition(policy.disposition, &model.name);
    let mut response = Response::new(Body::from(bytes));
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, policy.content_type);
    headers.insert(CONTENT_DISPOSITION, disposition);
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
}

#[cfg(test)]
mod tests {
    use super::{
        Disposition, DownloadPolicy, FALLBACK_CONTENT_TYPE, content_disposition, download_response,
        encode_rfc5987,
    };
    use crate::models::files::{FileUploadStatus, Model};
    use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS};
    use chrono::Utc;
    use uuid::Uuid;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
    const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00";
    const WEBP: &[u8] = b"RIFF\x24\x00\x00\x00WEBPVP8 ";
    const PDF: &[u8] = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3";
    const HTML: &[u8] = b"<!doctype html><script>alert(document.domain)</script>";
    const SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\" onload=\"alert(1)\"/>";

    fn policy(declared: &str, bytes: &[u8]) -> (Disposition, String) {
        let policy = DownloadPolicy::for_file(declared, bytes);
        (
            policy.disposition,
            policy.content_type.to_str().expect("ascii").to_string(),
        )
    }

    #[test]
    fn raster_images_and_pdfs_are_served_inline_with_their_sniffed_type() {
        for (declared, bytes, expected) in [
            ("image/png", PNG, "image/png"),
            ("image/jpg", JPEG, "image/jpeg"),
            ("image/gif", GIF, "image/gif"),
            ("image/webp", WEBP, "image/webp"),
            ("application/pdf", PDF, "application/pdf"),
            ("application/octet-stream", PNG, "image/png"),
            ("text/html", PNG, "image/png"),
        ] {
            assert_eq!(
                policy(declared, bytes),
                (Disposition::Inline, expected.to_string()),
                "{declared} with {expected} bytes"
            );
        }
    }

    #[test]
    fn html_and_svg_are_always_served_as_attachments() {
        for (declared, bytes) in [
            ("text/html", HTML),
            ("text/html; charset=utf-8", HTML),
            ("application/xhtml+xml", HTML),
            ("image/svg+xml", SVG),
            ("image/png", HTML),
            ("image/png", SVG),
            ("application/pdf", HTML),
        ] {
            assert_eq!(
                policy(declared, bytes),
                (Disposition::Attachment, declared.to_string()),
                "{declared}"
            );
        }
    }

    #[test]
    fn truncated_magic_bytes_are_not_trusted_for_inline() {
        for bytes in [
            &b""[..],
            &b"\x89PNG"[..],
            &[0xFF, 0xD8][..],
            &b"GIF8"[..],
            &b"RIFF\x24\x00\x00\x00WEB"[..],
            &b"RIFF\x24\x00\x00\x00WAVE"[..],
            &b"%PDF"[..],
        ] {
            assert_eq!(policy("image/png", bytes).0, Disposition::Attachment);
        }
    }

    #[test]
    fn unusable_declared_types_fall_back_to_octet_stream() {
        for declared in ["", "   ", "text/html\r\nSet-Cookie: a=b", "text/plain\0"] {
            assert_eq!(
                policy(declared, HTML),
                (Disposition::Attachment, FALLBACK_CONTENT_TYPE.to_string()),
                "{declared:?}"
            );
        }
    }

    #[test]
    fn content_disposition_quotes_ascii_names_and_encodes_unicode() {
        assert_eq!(
            content_disposition(Disposition::Attachment, "notes v2.md"),
            "attachment; filename=\"notes v2.md\"; filename*=UTF-8''notes%20v2.md"
        );
        assert_eq!(
            content_disposition(Disposition::Inline, "résumé.pdf"),
            "inline; filename=\"r_sum_.pdf\"; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf"
        );
    }

    #[test]
    fn content_disposition_neutralises_header_and_quote_injection() {
        let header = content_disposition(
            Disposition::Attachment,
            "a\"; filename=evil.html\r\nSet-Cookie: x=1;\\%.txt",
        );
        let value = header.to_str().expect("ascii header");
        assert!(!value.contains('\r') && !value.contains('\n'));
        assert_eq!(
            value,
            "attachment; filename=\"a_; filename=evil.html__Set-Cookie: x=1;__.txt\"; \
             filename*=UTF-8''a%22%3B%20filename%3Devil.html%0D%0ASet-Cookie%3A%20x%3D1%3B%5C%25.txt"
        );
    }

    #[test]
    fn content_disposition_omits_the_filename_when_none_is_stored() {
        assert_eq!(
            content_disposition(Disposition::Attachment, ""),
            "attachment"
        );
    }

    #[test]
    fn rfc5987_encoding_keeps_only_attr_chars() {
        assert_eq!(encode_rfc5987("a-b_c.~!"), "a-b_c.~!");
        assert_eq!(encode_rfc5987("../x y"), "..%2Fx%20y");
        assert_eq!(encode_rfc5987("日"), "%E6%97%A5");
    }

    #[test]
    fn download_responses_always_send_nosniff() {
        for (declared, bytes, disposition) in [
            (
                "image/png",
                PNG,
                "inline; filename=\"file.bin\"; filename*=UTF-8''file.bin",
            ),
            (
                "text/html",
                HTML,
                "attachment; filename=\"file.bin\"; filename*=UTF-8''file.bin",
            ),
        ] {
            let response = download_response(&file_model(declared), bytes.to_vec());
            let headers = response.headers();
            assert_eq!(headers[X_CONTENT_TYPE_OPTIONS], "nosniff");
            assert_eq!(headers[CONTENT_DISPOSITION], disposition);
            assert_eq!(headers[CONTENT_TYPE], declared);
        }
    }

    fn file_model(content_type: &str) -> Model {
        Model {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            name: "file.bin".to_string(),
            content_type: content_type.to_string(),
            size: 0,
            local_path: String::new(),
            description: None,
            url: None,
            sha256: None,
            status: FileUploadStatus::Uploaded,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            metadata: None,
        }
    }
}
