// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use anyhow::{Error, bail};
use reqwest::Response;

pub const MAX_CATALOG_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;

pub async fn read_body_capped(mut response: Response, max_bytes: usize) -> Result<Vec<u8>, Error> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        bail!(
            "response from {} is larger than {max_bytes} bytes",
            response.url()
        );
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > max_bytes {
            bail!(
                "response from {} is larger than {max_bytes} bytes",
                response.url()
            );
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
