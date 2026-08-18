// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

/// Detect MIME type from leading magic bytes.
/// Returns `None` when the signature is not recognised.
pub fn detect_mime(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, ..] => Some("image/png"),
        [0x47, 0x49, 0x46, 0x38, ..] => Some("image/gif"),
        [
            0x52,
            0x49,
            0x46,
            0x46,
            _,
            _,
            _,
            _,
            0x57,
            0x45,
            0x42,
            0x50,
            ..,
        ] => Some("image/webp"),
        [0x42, 0x4D, ..] => Some("image/bmp"),
        [0x49, 0x49, 0x2A, 0x00, ..] | [0x4D, 0x4D, 0x00, 0x2A, ..] => Some("image/tiff"),
        [0x25, 0x50, 0x44, 0x46, ..] => Some("application/pdf"),
        [0x50, 0x4B, 0x03, 0x04, ..] => Some("application/zip"),
        [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, ..] => Some("application/vnd.ms-office"),
        [
            0x52,
            0x49,
            0x46,
            0x46,
            _,
            _,
            _,
            _,
            0x57,
            0x41,
            0x56,
            0x45,
            ..,
        ] => Some("audio/wav"),
        [0x4F, 0x67, 0x67, 0x53, ..] => Some("audio/ogg"),
        [0x49, 0x44, 0x33, ..] => Some("audio/mpeg"),
        [0xFF, 0xFB, ..] | [0xFF, 0xFA, ..] | [0xFF, 0xF3, ..] => Some("audio/mpeg"),
        [0x1A, 0x45, 0xDF, 0xA3, ..] => Some("video/webm"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_jpeg() {
        assert_eq!(
            detect_mime(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10]),
            Some("image/jpeg")
        );
    }

    #[test]
    fn detects_png() {
        assert_eq!(
            detect_mime(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]),
            Some("image/png")
        );
    }

    #[test]
    fn detects_gif() {
        assert_eq!(detect_mime(b"GIF89a"), Some("image/gif"));
    }

    #[test]
    fn detects_pdf() {
        assert_eq!(detect_mime(b"%PDF-1.4 ..."), Some("application/pdf"));
    }

    #[test]
    fn detects_zip() {
        assert_eq!(
            detect_mime(&[0x50, 0x4B, 0x03, 0x04, 0, 0]),
            Some("application/zip")
        );
    }

    #[test]
    fn unknown_returns_none() {
        assert_eq!(detect_mime(&[0x00, 0x01, 0x02, 0x03]), None);
    }

    #[test]
    fn empty_returns_none() {
        assert_eq!(detect_mime(&[]), None);
    }
}
