// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("invalid provider manifest: {0}")]
    InvalidManifest(String),

    #[error("provider configuration error: {0}")]
    Configuration(String),

    #[error("missing provider credential: {0}")]
    MissingCredential(String),

    #[error("provider capability is not configured: {0}")]
    UnsupportedCapability(&'static str),

    #[error("payload mapping failed: {0}")]
    PayloadMapping(String),

    #[error("response mapping failed: {0}")]
    ResponseMapping(String),

    #[error("provider URL is not allowed: {0}")]
    UrlNotAllowed(String),

    #[error("provider header is not allowed: {0}")]
    HeaderNotAllowed(String),

    #[error("provider request failed: {0}")]
    Transport(String),

    #[error("provider returned HTTP {status}: {message}")]
    HttpStatus { status: u16, message: String },

    #[error("provider quota or rate limit was exhausted")]
    QuotaExhausted,

    #[error("provider billing is required")]
    PaymentRequired,

    #[error("provider stream ended unexpectedly")]
    StreamEnded,

    #[error("provider operation was cancelled")]
    Cancelled,

    #[error("provider response exceeded the configured size limit")]
    ResponseTooLarge,
}

impl From<reqwest::Error> for ProviderError {
    fn from(error: reqwest::Error) -> Self {
        Self::Transport(error.to_string())
    }
}

impl From<serde_json::Error> for ProviderError {
    fn from(error: serde_json::Error) -> Self {
        Self::ResponseMapping(error.to_string())
    }
}
