// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{future::Future, pin::Pin};

use openidconnect::{AsyncHttpClient, HttpRequest, HttpResponse};
use reqwest::{IntoUrl, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use thiserror::Error;
use url::{Host, Url};

use crate::services::{
    catalog_download::read_body_capped, safe_egress::checked_short_call_builder,
};

const SSO_RESPONSE_MAX_BYTES: usize = 1024 * 1024;

// reqwest never asks the resolver about an IP literal, so a literal host is checked here instead.
pub fn validate_literal_destination(url: &Url) -> Result<(), SsoEgressError> {
    let allowed = match url.host() {
        Some(Host::Ipv4(ip)) => connector_plugin::validate_destination_ip(ip.into(), true).is_ok(),
        Some(Host::Ipv6(ip)) => connector_plugin::validate_destination_ip(ip.into(), true).is_ok(),
        Some(Host::Domain(host)) => !host
            .trim_end_matches('.')
            .eq_ignore_ascii_case("metadata.google.internal"),
        None => false,
    };
    if allowed {
        Ok(())
    } else {
        Err(SsoEgressError::Destination)
    }
}

#[derive(Debug, Error)]
pub enum SsoEgressError {
    #[error("the SSO destination is not allowed")]
    Destination,
    #[error("SSO request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("SSO response was rejected: {0}")]
    Body(anyhow::Error),
}

pub struct SsoResponse {
    pub status: StatusCode,
    version: reqwest::Version,
    headers: reqwest::header::HeaderMap,
    body: Vec<u8>,
}

impl SsoResponse {
    pub fn json<T: DeserializeOwned>(&self) -> serde_json::Result<T> {
        serde_json::from_slice(&self.body)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn into_http(self) -> HttpResponse {
        let mut response = HttpResponse::new(self.body);
        *response.status_mut() = self.status;
        *response.version_mut() = self.version;
        *response.headers_mut() = self.headers;
        response
    }
}

// IdP and discovered endpoints are admin- or IdP-controlled; on-premise IdPs must stay reachable.
#[derive(Clone)]
pub struct SsoHttpClient {
    client: reqwest::Client,
}

impl SsoHttpClient {
    pub fn new() -> reqwest::Result<Self> {
        Ok(Self {
            client: checked_short_call_builder().build()?,
        })
    }

    pub fn get<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.client.get(url)
    }

    pub fn post<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.client.post(url)
    }

    pub async fn send(&self, request: RequestBuilder) -> Result<SsoResponse, SsoEgressError> {
        self.execute(request.build()?).await
    }

    async fn execute(&self, request: reqwest::Request) -> Result<SsoResponse, SsoEgressError> {
        validate_literal_destination(request.url())?;
        let response = self.client.execute(request).await?;
        let status = response.status();
        let version = response.version();
        let headers = response.headers().clone();
        let body = read_body_capped(response, SSO_RESPONSE_MAX_BYTES)
            .await
            .map_err(SsoEgressError::Body)?;
        Ok(SsoResponse {
            status,
            version,
            headers,
            body,
        })
    }
}

impl<'c> AsyncHttpClient<'c> for SsoHttpClient {
    type Error = SsoEgressError;
    type Future = Pin<Box<dyn Future<Output = Result<HttpResponse, SsoEgressError>> + Send + 'c>>;

    fn call(&'c self, request: HttpRequest) -> Self::Future {
        Box::pin(async move {
            let response = self.execute(reqwest::Request::try_from(request)?).await?;
            Ok(response.into_http())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, OnceLock};

    async fn serve(router: axum::Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let base = format!("http://{}", listener.local_addr().expect("address"));
        tokio::spawn(async move { axum::serve(listener, router).await.expect("server") });
        base
    }

    #[tokio::test]
    async fn the_sso_client_refuses_metadata_and_link_local_literals() {
        let client = SsoHttpClient::new().expect("SSO client");
        for url in [
            "http://169.254.169.254/latest/meta-data",
            "http://[::1]:1/",
            "http://[fe80::1]/",
            "http://[fd00:ec2::254]/latest/meta-data",
            "http://[::ffff:169.254.169.254]/",
            "http://[::ffff:127.0.0.1]:1/",
            "http://0.0.0.0/",
            "http://metadata.google.internal/computeMetadata/v1",
        ] {
            let refused = matches!(
                client.send(client.get(url)).await,
                Err(SsoEgressError::Destination)
            );
            let loopback = url.contains("[::1]") || url.contains("127.0.0.1");
            assert_eq!(refused, !loopback, "{url}");
        }
        let issuer = openidconnect::IssuerUrl::new("http://[fd00:ec2::254]".to_string()).unwrap();
        assert!(
            openidconnect::core::CoreProviderMetadata::discover_async(issuer, &client)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn discovered_endpoints_on_metadata_addresses_are_refused() {
        use axum::{Json, Router, extract::State, routing::get};
        async fn discovery(State(base): State<Arc<OnceLock<String>>>) -> Json<serde_json::Value> {
            let base = base.get().expect("base").clone();
            Json(serde_json::json!({
                "issuer": base,
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
                "jwks_uri": "http://169.254.169.254/latest/meta-data/iam",
                "response_types_supported": ["code"],
                "subject_types_supported": ["public"],
                "id_token_signing_alg_values_supported": ["RS256"]
            }))
        }
        let base = Arc::new(OnceLock::new());
        let url = serve(
            Router::new()
                .route("/.well-known/openid-configuration", get(discovery))
                .with_state(base.clone()),
        )
        .await;
        base.set(url.clone()).expect("base once");

        let error = openidconnect::core::CoreProviderMetadata::discover_async(
            openidconnect::IssuerUrl::new(url).unwrap(),
            &SsoHttpClient::new().expect("SSO client"),
        )
        .await
        .expect_err("a JWKS URL on the metadata address must not be fetched");
        assert!(
            matches!(
                error,
                openidconnect::DiscoveryError::Request(SsoEgressError::Destination)
            ),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn sso_response_bodies_are_capped() {
        use axum::{Router, body::Body, routing::get};
        let oversized = || vec![b' '; SSO_RESPONSE_MAX_BYTES + 1];
        let base = serve(
            Router::new()
                .route("/fits", get(|| async { "{}" }))
                .route("/declared", get(move || async move { oversized() }))
                .route(
                    "/streamed",
                    get(move || async move {
                        let chunks = (0..=SSO_RESPONSE_MAX_BYTES / 4096)
                            .map(|_| Ok::<_, std::io::Error>(vec![b' '; 4096]));
                        Body::from_stream(futures_util::stream::iter(chunks))
                    }),
                ),
        )
        .await;
        let client = SsoHttpClient::new().expect("SSO client");

        let fits = client
            .send(client.get(format!("{base}/fits")))
            .await
            .expect("small body");
        assert_eq!(
            fits.json::<serde_json::Value>().unwrap(),
            serde_json::json!({})
        );
        for path in ["declared", "streamed"] {
            assert!(
                matches!(
                    client.send(client.get(format!("{base}/{path}"))).await,
                    Err(SsoEgressError::Body(_))
                ),
                "{path}"
            );
        }
    }
}
