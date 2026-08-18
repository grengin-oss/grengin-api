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

impl ProviderError {
    /// Whether retrying the same request could plausibly succeed.
    ///
    /// Manifest, mapping, credential and URL failures are deterministic properties of the provider
    /// configuration: retrying them burns quota and hides the misconfiguration. Transport faults and
    /// server-side errors are worth another attempt, and `QuotaExhausted` is retryable only after a
    /// backoff, which is the caller's job to apply.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Transport(_) | Self::StreamEnded | Self::QuotaExhausted => true,
            Self::HttpStatus { status, .. } => {
                matches!(status, 408 | 409 | 425 | 429 | 500 | 502 | 503 | 504)
            }
            Self::InvalidManifest(_)
            | Self::Configuration(_)
            | Self::MissingCredential(_)
            | Self::UnsupportedCapability(_)
            | Self::PayloadMapping(_)
            | Self::ResponseMapping(_)
            | Self::UrlNotAllowed(_)
            | Self::HeaderNotAllowed(_)
            | Self::PaymentRequired
            | Self::Cancelled
            | Self::ResponseTooLarge => false,
        }
    }

    /// Whether the failure is caused by how the provider was configured, as opposed to the provider
    /// or network misbehaving. Useful for deciding whether to surface a config-fix hint.
    pub fn is_configuration_fault(&self) -> bool {
        matches!(
            self,
            Self::InvalidManifest(_)
                | Self::Configuration(_)
                | Self::MissingCredential(_)
                | Self::UnsupportedCapability(_)
                | Self::PayloadMapping(_)
                | Self::UrlNotAllowed(_)
                | Self::HeaderNotAllowed(_)
        )
    }
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

#[cfg(test)]
mod tests {
    use super::ProviderError;

    #[test]
    fn separates_configuration_faults_from_worth_retrying() {
        // A misconfigured custom provider must not be retried: it would burn quota on every attempt
        // and bury the actual mistake.
        for error in [
            ProviderError::InvalidManifest("bad".to_string()),
            ProviderError::MissingCredential("api_key".to_string()),
            ProviderError::PayloadMapping("no path".to_string()),
            ProviderError::UrlNotAllowed("blocked".to_string()),
            ProviderError::PaymentRequired,
            ProviderError::HttpStatus {
                status: 401,
                message: "Unauthorized".to_string(),
            },
            ProviderError::HttpStatus {
                status: 400,
                message: "Bad Request".to_string(),
            },
        ] {
            assert!(!error.is_retryable(), "{error} should not be retried");
        }

        for error in [
            ProviderError::Transport("reset".to_string()),
            ProviderError::StreamEnded,
            ProviderError::QuotaExhausted,
            ProviderError::HttpStatus {
                status: 503,
                message: "Service Unavailable".to_string(),
            },
        ] {
            assert!(error.is_retryable(), "{error} should be retried");
        }

        assert!(ProviderError::MissingCredential("k".to_string()).is_configuration_fault());
        assert!(!ProviderError::QuotaExhausted.is_configuration_fault());
        // A mapping failure against a live response is the manifest's fault, but it is the provider
        // that changed shape, so it is not billed as a config fault the operator can pre-empt.
        assert!(!ProviderError::ResponseMapping("shape".to_string()).is_configuration_fault());
    }
}
