// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::net::{IpAddr, Ipv6Addr};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use url::Url;

use crate::ProviderError;

const BLOCKED_HEADERS: &[&str] = &[
    "connection",
    "content-length",
    "cookie",
    "forwarded",
    "host",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
];

pub fn validate_header_name(name: &str) -> Result<(), ProviderError> {
    let normalized = name.trim().to_ascii_lowercase();
    if normalized.is_empty()
        || !normalized.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
    {
        return Err(ProviderError::HeaderNotAllowed(name.to_string()));
    }
    if BLOCKED_HEADERS.contains(&normalized.as_str()) {
        return Err(ProviderError::HeaderNotAllowed(name.to_string()));
    }
    Ok(())
}

pub fn validate_provider_url(
    url: &Url,
    allow_insecure_http: bool,
    allow_private_network: bool,
) -> Result<(), ProviderError> {
    match url.scheme() {
        "https" => {}
        "http" if allow_insecure_http => {}
        "http" => {
            return Err(ProviderError::UrlNotAllowed(
                "plain HTTP requires explicit administrator approval".to_string(),
            ));
        }
        scheme => {
            return Err(ProviderError::UrlNotAllowed(format!(
                "unsupported URL scheme {scheme}"
            )));
        }
    }

    if !url.username().is_empty() || url.password().is_some() {
        return Err(ProviderError::UrlNotAllowed(
            "credentials must not be embedded in provider URLs".to_string(),
        ));
    }
    if url.fragment().is_some() {
        return Err(ProviderError::UrlNotAllowed(
            "provider URLs must not contain fragments".to_string(),
        ));
    }

    let host = url
        .host_str()
        .ok_or_else(|| ProviderError::UrlNotAllowed("provider URL has no host".to_string()))?;
    if is_metadata_hostname(host) {
        return Err(ProviderError::UrlNotAllowed(format!(
            "cloud metadata host {host} is never allowed"
        )));
    }
    if !allow_private_network && is_local_hostname(host) {
        return Err(ProviderError::UrlNotAllowed(format!(
            "local provider host {host} requires private-network approval"
        )));
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        validate_destination_ip(ip, allow_private_network)?;
    }
    Ok(())
}

pub fn validate_destination_ip(
    ip: IpAddr,
    allow_private_network: bool,
) -> Result<(), ProviderError> {
    if ip.is_unspecified() || ip.is_multicast() {
        return Err(ProviderError::UrlNotAllowed(format!(
            "destination address {ip} is not routable"
        )));
    }
    // Private-network approval is for self-hosted models, never for cloud metadata credentials.
    if is_link_local_or_metadata(ip) {
        return Err(ProviderError::UrlNotAllowed(format!(
            "link-local destination {ip} is never allowed"
        )));
    }
    if allow_private_network {
        return Ok(());
    }

    let blocked = match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || octets[0] == 0
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                || (octets[0] == 198 && matches!(octets[1], 18 | 19))
                || octets[0] >= 240
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_unspecified()
                || ip.segments()[0..2] == [0x2001, 0x0db8]
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|mapped| validate_destination_ip(mapped.into(), false).is_err())
        }
    };
    if blocked {
        return Err(ProviderError::UrlNotAllowed(format!(
            "destination address {ip} requires private-network approval"
        )));
    }
    Ok(())
}

const AWS_IPV6_METADATA: Ipv6Addr = Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254);

fn is_link_local_or_metadata(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_link_local(),
        IpAddr::V6(ip) => {
            ip.is_unicast_link_local()
                || ip.to_ipv4_mapped().is_some_and(|ip| ip.is_link_local())
                || ip == AWS_IPV6_METADATA
        }
    }
}

fn is_metadata_hostname(host: &str) -> bool {
    host.trim_end_matches('.')
        .eq_ignore_ascii_case("metadata.google.internal")
}

fn is_local_hostname(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost")
}

// Validating at connect time closes the gap where a host re-resolves to a private address
// after the pre-flight check.
pub struct SafeResolver {
    pub allow_private_network: bool,
}

impl Resolve for SafeResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let allow_private_network = self.allow_private_network;
        Box::pin(async move {
            let addresses: Vec<_> = tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
            if addresses.is_empty() {
                return Err(std::io::Error::other("empty DNS answer").into());
            }
            for address in &addresses {
                validate_destination_ip(address.ip(), allow_private_network)?;
            }
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use url::Url;

    use super::{validate_destination_ip, validate_header_name, validate_provider_url};

    #[test]
    fn rejects_authority_and_forwarding_headers() {
        for header in ["Host", "Content-Length", "X-Forwarded-For", "Connection"] {
            assert!(validate_header_name(header).is_err(), "{header}");
        }
        assert!(validate_header_name("Authorization").is_ok());
        assert!(validate_header_name("X-Provider-Version").is_ok());
    }

    #[test]
    fn metadata_destinations_stay_blocked_with_private_network_approval() {
        for metadata in [
            IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
            IpAddr::V6("fe80::1".parse::<Ipv6Addr>().unwrap()),
            IpAddr::V6("::ffff:169.254.169.254".parse::<Ipv6Addr>().unwrap()),
            IpAddr::V6("fd00:ec2::254".parse::<Ipv6Addr>().unwrap()),
        ] {
            assert!(
                validate_destination_ip(metadata, true).is_err(),
                "{metadata}"
            );
        }
        let url = Url::parse("http://metadata.google.internal./computeMetadata/v1").unwrap();
        assert!(validate_provider_url(&url, true, true).is_err());
    }

    #[test]
    fn blocks_private_destinations_by_default() {
        for private in [
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1)),
            IpAddr::V6("::ffff:127.0.0.1".parse::<Ipv6Addr>().unwrap()),
            IpAddr::V6("2001:db8::1".parse::<Ipv6Addr>().unwrap()),
        ] {
            assert!(
                validate_destination_ip(private, false).is_err(),
                "{private}"
            );
            assert!(validate_destination_ip(private, true).is_ok(), "{private}");
        }
        assert!(validate_destination_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), false).is_ok());
    }

    #[test]
    fn insecure_local_provider_requires_both_approvals() {
        let url = Url::parse("http://127.0.0.1:11434/v1").unwrap();
        assert!(validate_provider_url(&url, false, false).is_err());
        assert!(validate_provider_url(&url, true, false).is_err());
        assert!(validate_provider_url(&url, true, true).is_ok());
    }
}
