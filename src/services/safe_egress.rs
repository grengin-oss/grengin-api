// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{
    net::SocketAddr,
    sync::{Arc, OnceLock},
};

use crate::services::http_client::HttpTimeouts;

// Remote MCP servers and OAuth token endpoints are admin-supplied. Their URLs are checked as text when
// saved, but a host name can still resolve to cloud metadata or a link-local address, so every address
// is checked again at connect time. Private networks stay allowed: on-premise MCP servers are expected.
async fn resolve_checked(host: &str) -> std::io::Result<Vec<SocketAddr>> {
    let addresses: Vec<_> = tokio::net::lookup_host((host, 0)).await?.collect();
    if addresses.is_empty() {
        return Err(std::io::Error::other("empty DNS answer"));
    }
    for address in &addresses {
        connector_server_plugin::validate_destination_ip(address.ip(), true)
            .map_err(|_| std::io::Error::other("destination address is not allowed"))?;
    }
    Ok(addresses)
}

struct CheckedResolver;

impl reqwest::dns::Resolve for CheckedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let addresses = resolve_checked(name.as_str()).await?;
            Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

struct CheckedMcpResolver;

impl reqwest_mcp::dns::Resolve for CheckedMcpResolver {
    fn resolve(&self, name: reqwest_mcp::dns::Name) -> reqwest_mcp::dns::Resolving {
        Box::pin(async move {
            let addresses = resolve_checked(name.as_str()).await?;
            Ok(Box::new(addresses.into_iter()) as reqwest_mcp::dns::Addrs)
        })
    }
}

// A proxy would resolve the name itself and bypass the check, and a redirect could leave the vetted host.
pub fn mcp_client_builder() -> reqwest_mcp::ClientBuilder {
    reqwest_mcp::Client::builder()
        .no_proxy()
        .redirect(reqwest_mcp::redirect::Policy::none())
        .dns_resolver(Arc::new(CheckedMcpResolver))
}

pub fn oauth_token_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        HttpTimeouts::SHORT_CALL
            .apply(
                reqwest::Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .dns_resolver(Arc::new(CheckedResolver)),
            )
            .build()
            .expect("the OAuth token client uses only static settings")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn names_resolving_to_metadata_or_link_local_are_refused() {
        for host in ["169.254.169.254", "fe80::1", "fd00:ec2::254"] {
            assert!(resolve_checked(host).await.is_err(), "{host}");
        }
    }

    #[tokio::test]
    async fn private_and_loopback_destinations_stay_allowed() {
        for host in ["127.0.0.1", "10.1.2.3", "192.168.1.10", "localhost"] {
            assert!(resolve_checked(host).await.is_ok(), "{host}");
        }
    }
}
