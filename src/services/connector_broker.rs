// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::error::AppError;
use crate::error::{ErrorDetailVariant, ErrorResponse};
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use openssl::{
    hash::{MessageDigest, hash},
    pkey::{Id, PKey},
    rand::rand_bytes,
    sign::Signer,
};
use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;
use url::Url;

#[derive(Debug)]
pub enum BrokerIssue {
    NotFound,
    Forbidden,
    ReconnectRequired,
    RateLimited(u64),
    CapacityExceeded,
    Disabled,
    InvalidRequest,
    Unavailable,
}

impl BrokerIssue {
    fn rate_limited(retry_after: Option<&str>) -> Self {
        Self::RateLimited(
            retry_after
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(1)
                .clamp(1, 300),
        )
    }
    fn classify(code: Option<&str>, retry_after: Option<&str>) -> Self {
        match code {
            Some("connector_not_found") => Self::NotFound,
            Some("installation_disconnected" | "invalid_connector_proof") => Self::Forbidden,
            Some(
                "connector_reconnect_required"
                | "connector_confirmation_invalid"
                | "connector_state_expired_or_used",
            ) => Self::ReconnectRequired,
            Some("connector_rate_limited" | "connector_refresh_busy") => {
                Self::rate_limited(retry_after)
            }
            Some("connector_broker_disabled") => Self::Disabled,
            Some("invalid_connector_request" | "invalid_connector" | "unsupported_connector") => {
                Self::InvalidRequest
            }
            _ => Self::Unavailable,
        }
    }
    fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "connector_not_found",
            Self::Forbidden => "connector_forbidden",
            Self::ReconnectRequired => "connector_reconnect_required",
            Self::RateLimited(_) => "connector_rate_limited",
            Self::CapacityExceeded => "connector_capacity_exceeded",
            Self::Disabled => "connector_broker_disabled",
            Self::InvalidRequest => "invalid_connector_request",
            Self::Unavailable => "connector_service_unavailable",
        }
    }
}

#[derive(Debug)]
pub enum ConnectorError {
    Local(AppError),
    Broker(BrokerIssue),
}

impl From<AppError> for ConnectorError {
    fn from(error: AppError) -> Self {
        Self::Local(error)
    }
}
impl From<ConnectorError> for AppError {
    fn from(error: ConnectorError) -> Self {
        match error {
            ConnectorError::Local(error) => error,
            ConnectorError::Broker(BrokerIssue::NotFound) => AppError::ResourceNotFound,
            ConnectorError::Broker(
                BrokerIssue::ReconnectRequired | BrokerIssue::CapacityExceeded,
            ) => AppError::DbConflict,
            ConnectorError::Broker(_) => AppError::ServiceTemporarilyUnavailable,
        }
    }
}
impl IntoResponse for ConnectorError {
    fn into_response(self) -> Response {
        let issue = match self {
            Self::Local(error) => return error.into_response(),
            Self::Broker(issue) => issue,
        };
        let (status, description, solution) = match issue {
            BrokerIssue::NotFound => (
                StatusCode::NOT_FOUND,
                "The connector connection was not found.",
                "Check the connection and signed-in account.",
            ),
            BrokerIssue::Forbidden => (
                StatusCode::FORBIDDEN,
                "The installation cannot authorize this connector.",
                "Ask an administrator to verify the installation connection.",
            ),
            BrokerIssue::ReconnectRequired => (
                StatusCode::CONFLICT,
                "The connector needs confirmation or reconnection.",
                "Check the connection status and restart consent if necessary.",
            ),
            BrokerIssue::RateLimited(_) => (
                StatusCode::TOO_MANY_REQUESTS,
                "The connector request limit was reached.",
                "Wait for Retry-After before trying again.",
            ),
            BrokerIssue::CapacityExceeded => (
                StatusCode::CONFLICT,
                "The account has too many stored connector connections.",
                "Disconnect unused connections before adding another.",
            ),
            BrokerIssue::InvalidRequest => (
                StatusCode::BAD_REQUEST,
                "The connector request is invalid or unsupported.",
                "Check the connector version and request fields.",
            ),
            BrokerIssue::Disabled | BrokerIssue::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "The connector broker is unavailable or not configured.",
                "Retry later or contact an administrator.",
            ),
        };
        let (_, mut detail) = AppError::ServiceTemporarilyUnavailable.to_detail();
        detail.description = description.into();
        detail.solution = solution.into();
        detail.description_key = format!("error.connector.{}.description", issue.code());
        detail.solution_key = format!("error.connector.{}.solution", issue.code());
        detail.external_code = Some(issue.code().into());
        let mut response = (
            status,
            Json(ErrorResponse {
                detail: ErrorDetailVariant::Rich(detail),
            }),
        )
            .into_response();
        if let BrokerIssue::RateLimited(seconds) = issue {
            response.headers_mut().insert(
                "Retry-After",
                seconds
                    .to_string()
                    .parse()
                    .unwrap_or_else(|_| axum::http::HeaderValue::from_static("1")),
            );
        }
        response
    }
}

pub struct ConnectorBroker {
    base: Url,
    installation: String,
    key: PKey<openssl::pkey::Private>,
    client: reqwest::Client,
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn random_secret() -> Result<String, AppError> {
    let mut bytes = [0; 32];
    rand_bytes(&mut bytes).map_err(|_| AppError::ServiceTemporarilyUnavailable)?;
    Ok(hex(&bytes))
}

fn decode_key(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.is_ascii() {
        return None;
    }
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}

fn service_url(raw: &str) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    (url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.path() == "/"
        && (url.scheme() == "https" || (url.scheme() == "http" && loopback)))
        .then_some(url)
}

pub fn proof_message(
    installation: &str,
    timestamp: i64,
    nonce: &str,
    path: &str,
    body: &[u8],
) -> Result<String, AppError> {
    let digest =
        hash(MessageDigest::sha256(), body).map_err(|_| AppError::ServiceTemporarilyUnavailable)?;
    Ok(format!(
        "grengin-connectors-v1\n{installation}\n{timestamp}\n{nonce}\nPOST\n{path}\n{}",
        hex(&digest)
    ))
}

impl ConnectorBroker {
    pub fn from_env() -> Result<Self, AppError> {
        let unavailable = || AppError::ServiceTemporarilyUnavailable;
        let base = service_url(
            &std::env::var("GRENGIN_SERVICE_URL")
                .unwrap_or_else(|_| "https://service.grengin.com".into()),
        )
        .ok_or_else(unavailable)?;
        let installation =
            std::env::var("GRENGIN_CONNECTOR_INSTALLATION_ID").map_err(|_| unavailable())?;
        if installation.is_empty()
            || installation.len() > 128
            || !installation
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(unavailable());
        }
        let seed =
            decode_key(&std::env::var("GRENGIN_CONNECTOR_PRIVATE_KEY").map_err(|_| unavailable())?)
                .ok_or_else(unavailable)?;
        let key =
            PKey::private_key_from_raw_bytes(&seed, Id::ED25519).map_err(|_| unavailable())?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| unavailable())?;
        Ok(Self {
            base,
            installation,
            key,
            client,
        })
    }

    pub async fn post<T: Serialize, R: DeserializeOwned>(
        &self,
        path: &str,
        payload: &T,
    ) -> Result<R, ConnectorError> {
        let body =
            serde_json::to_vec(payload).map_err(|_| AppError::ServiceTemporarilyUnavailable)?;
        let timestamp = chrono::Utc::now().timestamp();
        let nonce = random_secret()?;
        let message = proof_message(&self.installation, timestamp, &nonce, path, &body)?;
        let signature = Signer::new_without_digest(&self.key)
            .and_then(|mut signer| signer.sign_oneshot_to_vec(message.as_bytes()))
            .map_err(|_| AppError::ServiceTemporarilyUnavailable)?;
        let response = self
            .client
            .post(
                self.base
                    .join(path)
                    .map_err(|_| AppError::ServiceTemporarilyUnavailable)?,
            )
            .header("X-Grengin-Installation", &self.installation)
            .header("X-Grengin-Timestamp", timestamp.to_string())
            .header("X-Grengin-Nonce", nonce)
            .header("X-Grengin-Signature", hex(&signature))
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| AppError::ServiceTemporarilyUnavailable)?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get("Retry-After")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ConnectorError::Broker(BrokerIssue::rate_limited(
                retry_after.as_deref(),
            )));
        }
        if response.content_length().is_some_and(|size| size > 65536) {
            return Err(AppError::ServiceTemporarilyUnavailable.into());
        }
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| AppError::ServiceTemporarilyUnavailable)?
        {
            if bytes.len() + chunk.len() > 65536 {
                return Err(AppError::ServiceTemporarilyUnavailable.into());
            }
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
            return Err(ConnectorError::Broker(BrokerIssue::classify(
                body.pointer("/error/type").and_then(|value| value.as_str()),
                retry_after.as_deref(),
            )));
        }
        serde_json::from_slice(&bytes).map_err(|_| AppError::ServiceTemporarilyUnavailable.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn broker_errors_preserve_actionable_status_without_copying_vendor_text() {
        for (code, expected) in [
            ("connector_not_found", 404),
            ("installation_disconnected", 403),
            ("invalid_connector_request", 400),
            ("connector_reconnect_required", 409),
            ("connector_rate_limited", 429),
            ("UNTRUSTED_SECRET", 503),
        ] {
            let response = ConnectorError::Broker(BrokerIssue::classify(Some(code), Some("13")))
                .into_response();
            assert_eq!(response.status().as_u16(), expected);
            if expected == 429 {
                assert_eq!(response.headers().get("Retry-After").unwrap(), "13");
            }
            let body = axum::body::to_bytes(response.into_body(), 16384)
                .await
                .unwrap();
            assert!(
                !String::from_utf8(body.to_vec())
                    .unwrap()
                    .contains("UNTRUSTED_SECRET")
            );
        }
    }

    #[test]
    fn retry_after_is_bounded_and_invalid_values_do_not_become_headers() {
        assert!(matches!(
            BrokerIssue::classify(Some("connector_rate_limited"), Some("999999")),
            BrokerIssue::RateLimited(300)
        ));
        assert!(matches!(
            BrokerIssue::classify(Some("connector_rate_limited"), Some("bad\r\nheader")),
            BrokerIssue::RateLimited(1)
        ));
        assert!(matches!(
            BrokerIssue::classify(Some("connector_rate_limited"), Some("0")),
            BrokerIssue::RateLimited(1)
        ));
    }

    #[test]
    fn broker_urls_reject_insecure_remote_hosts_and_url_credentials() {
        for value in [
            "http://example.com",
            "https://user:secret@example.com",
            "https://example.com/path",
            "https://example.com/?token=secret",
            "file:///tmp/a",
        ] {
            assert!(service_url(value).is_none(), "{value}");
        }
        assert!(service_url("https://service.grengin.com").is_some());
        assert!(service_url("http://127.0.0.1:18081").is_some());
    }

    #[test]
    fn proof_is_bound_to_operation_identity_and_exact_body() {
        let proof = proof_message("inst_1", 123, "nonce", "/v1/connectors/start", b"{}").unwrap();
        assert_ne!(
            proof,
            proof_message("inst_2", 123, "nonce", "/v1/connectors/start", b"{}").unwrap()
        );
        assert_ne!(
            proof,
            proof_message("inst_1", 123, "nonce", "/v1/connectors/start", b"{ }").unwrap()
        );
        assert_ne!(
            proof,
            proof_message("inst_1", 123, "nonce", "/v1/connectors/token", b"{}").unwrap()
        );
        assert_eq!(decode_key(&"a".repeat(64)).unwrap(), [0xaa; 32]);
        assert!(decode_key(&"z".repeat(64)).is_none());
    }
}
