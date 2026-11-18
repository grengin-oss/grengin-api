// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{net::IpAddr, sync::Arc};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use url::{Host, Url};

use crate::ConnectorError;

pub fn validate_url(url: &Url, insecure: bool, private: bool) -> Result<(), ConnectorError> {
    llm_plugin::security::validate_provider_url(url, insecure, private)
        .map_err(|_| ConnectorError::Destination)?;
    if url.query().is_some() || url.fragment().is_some() {
        return Err(ConnectorError::Destination);
    }
    match url.host() {
        Some(Host::Ipv4(ip)) => validate_ip(ip.into(), private),
        Some(Host::Ipv6(ip)) => validate_ip(ip.into(), private),
        Some(Host::Domain(host)) if host.trim_end_matches('.') != "metadata.google.internal" => {
            Ok(())
        }
        _ => Err(ConnectorError::Destination),
    }
}

pub(crate) fn validate_ip(ip: IpAddr, private: bool) -> Result<(), ConnectorError> {
    // Even private-network approval must not expose cloud metadata credentials.
    let link_local = match ip {
        IpAddr::V4(ip) => ip.is_link_local(),
        IpAddr::V6(ip) => {
            ip.is_unicast_link_local()
                || ip.to_ipv4_mapped().is_some_and(|ip| ip.is_link_local())
                || ip
                    == "fd00:ec2::254"
                        .parse::<std::net::Ipv6Addr>()
                        .expect("metadata IP")
        }
    };
    if link_local {
        return Err(ConnectorError::Destination);
    }
    llm_plugin::security::validate_destination_ip(ip, private)
        .map_err(|_| ConnectorError::Destination)
}

pub(crate) struct SafeResolver {
    pub private: bool,
}

impl Resolve for SafeResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let private = self.private;
        Box::pin(async move {
            let addresses: Vec<_> = tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
            if addresses.is_empty() {
                return Err(std::io::Error::other("empty DNS answer").into());
            }
            for address in &addresses {
                validate_ip(address.ip(), private)?;
            }
            // reqwest connects to these validated addresses, rather than resolving them again.
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

pub(crate) fn http_client(private: bool) -> Result<reqwest::Client, ConnectorError> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(Arc::new(SafeResolver { private }))
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(30))
        .pool_max_idle_per_host(2)
        .build()
        .map_err(|_| ConnectorError::Transport)
}
