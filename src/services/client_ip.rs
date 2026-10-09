// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use axum::http::HeaderMap;

const FORWARDED_FOR: &str = "x-forwarded-for";
const REAL_IP: &str = "x-real-ip";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct IpRange {
    network: IpAddr,
    prefix_len: u8,
}

impl IpRange {
    fn parse(raw: &str) -> Option<Self> {
        let (addr, prefix) = match raw.split_once('/') {
            Some((addr, prefix)) => (addr.trim(), Some(prefix.trim())),
            None => (raw, None),
        };
        let network = addr.parse::<IpAddr>().ok()?.to_canonical();
        let max_len = match network {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        let prefix_len = match prefix {
            Some(prefix) => prefix.parse::<u8>().ok().filter(|len| *len <= max_len)?,
            None => max_len,
        };
        Some(Self {
            network,
            prefix_len,
        })
    }

    fn contains(&self, ip: IpAddr) -> bool {
        match (self.network, ip) {
            (IpAddr::V4(network), IpAddr::V4(ip)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix_len))
                    .unwrap_or(0);
                u32::from(network) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(network), IpAddr::V6(ip)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix_len))
                    .unwrap_or(0);
                u128::from(network) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct TrustedProxies {
    ranges: Vec<IpRange>,
}

impl TrustedProxies {
    fn private_networks() -> Self {
        let private_networks = [
            (IpAddr::V4(Ipv4Addr::new(127, 0, 0, 0)), 8),
            (IpAddr::V4(Ipv4Addr::new(10, 0, 0, 0)), 8),
            (IpAddr::V4(Ipv4Addr::new(172, 16, 0, 0)), 12),
            (IpAddr::V4(Ipv4Addr::new(192, 168, 0, 0)), 16),
            (IpAddr::V6(Ipv6Addr::LOCALHOST), 128),
            (IpAddr::V6(Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0)), 7),
        ];
        Self {
            ranges: private_networks
                .into_iter()
                .map(|(network, prefix_len)| IpRange {
                    network,
                    prefix_len,
                })
                .collect(),
        }
    }

    pub fn from_setting(raw: Option<&str>) -> Option<Self> {
        let raw = raw.map(str::trim).unwrap_or_default();
        if raw.is_empty() {
            return Some(Self::private_networks());
        }
        if raw.eq_ignore_ascii_case("none") {
            return Some(Self { ranges: Vec::new() });
        }
        raw.split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(IpRange::parse)
            .collect::<Option<Vec<_>>>()
            .map(|ranges| Self { ranges })
    }

    pub fn client_ip(&self, peer: Option<IpAddr>, headers: &HeaderMap) -> Option<IpAddr> {
        let peer = peer?.to_canonical();
        if !self.contains(peer) {
            return Some(peer);
        }
        let hops = forwarded_for_hops(headers);
        if hops.is_empty() {
            return Some(real_ip(headers).unwrap_or(peer));
        }
        // Entries left of the first untrusted hop (walking right to left) are client-supplied.
        let mut client = peer;
        for hop in hops.into_iter().rev() {
            let Some(hop) = hop else {
                break;
            };
            client = hop;
            if !self.contains(hop) {
                break;
            }
        }
        Some(client)
    }

    fn contains(&self, ip: IpAddr) -> bool {
        self.ranges.iter().any(|range| range.contains(ip))
    }
}

fn forwarded_for_hops(headers: &HeaderMap) -> Vec<Option<IpAddr>> {
    headers
        .get_all(FORWARDED_FOR)
        .iter()
        .flat_map(|value| match value.to_str() {
            Ok(value) => value.split(',').map(parse_hop).collect::<Vec<_>>(),
            Err(_) => vec![None],
        })
        .collect()
}

fn real_ip(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get(REAL_IP)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_hop)
}

fn parse_hop(raw: &str) -> Option<IpAddr> {
    let raw = raw.trim();
    raw.parse::<IpAddr>()
        .or_else(|_| raw.parse::<SocketAddr>().map(|addr| addr.ip()))
        .ok()
        .map(|ip| ip.to_canonical())
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use axum::http::{HeaderMap, HeaderValue};

    use super::TrustedProxies;

    fn ip(raw: &str) -> IpAddr {
        raw.parse().expect("valid ip")
    }

    fn headers(entries: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in entries {
            map.append(*name, HeaderValue::from_str(value).expect("header value"));
        }
        map
    }

    fn defaults() -> TrustedProxies {
        TrustedProxies::from_setting(None).expect("default trusted proxies")
    }

    #[test]
    fn direct_public_peer_ignores_spoofed_forwarded_for() {
        let spoofed = headers(&[("x-forwarded-for", "1.2.3.4"), ("x-real-ip", "5.6.7.8")]);
        assert_eq!(
            defaults().client_ip(Some(ip("203.0.113.9")), &spoofed),
            Some(ip("203.0.113.9"))
        );
    }

    #[test]
    fn loopback_proxy_forwards_the_real_client_ip() {
        let forwarded = headers(&[("x-forwarded-for", "198.51.100.7")]);
        assert_eq!(
            defaults().client_ip(Some(ip("127.0.0.1")), &forwarded),
            Some(ip("198.51.100.7"))
        );
    }

    #[test]
    fn private_network_load_balancer_forwards_the_real_client_ip() {
        let forwarded = headers(&[("x-forwarded-for", "198.51.100.7")]);
        assert_eq!(
            defaults().client_ip(Some(ip("10.0.3.17")), &forwarded),
            Some(ip("198.51.100.7"))
        );
    }

    #[test]
    fn client_supplied_forwarded_for_prefix_is_ignored_behind_a_proxy() {
        let forwarded = headers(&[("x-forwarded-for", "6.6.6.6, 198.51.100.7")]);
        assert_eq!(
            defaults().client_ip(Some(ip("127.0.0.1")), &forwarded),
            Some(ip("198.51.100.7"))
        );
    }

    #[test]
    fn multi_hop_chain_skips_every_trusted_proxy() {
        let forwarded = headers(&[
            ("x-forwarded-for", "6.6.6.6, 198.51.100.7, 10.0.0.5"),
            ("x-forwarded-for", "172.20.0.2"),
        ]);
        assert_eq!(
            defaults().client_ip(Some(ip("127.0.0.1")), &forwarded),
            Some(ip("198.51.100.7"))
        );
    }

    #[test]
    fn chain_of_only_trusted_hops_resolves_to_the_leftmost_hop() {
        let forwarded = headers(&[("x-forwarded-for", "192.168.1.20, 10.0.0.5")]);
        assert_eq!(
            defaults().client_ip(Some(ip("127.0.0.1")), &forwarded),
            Some(ip("192.168.1.20"))
        );
    }

    #[test]
    fn unparsable_hop_stops_the_walk_at_the_last_trusted_address() {
        let forwarded = headers(&[("x-forwarded-for", "198.51.100.7, garbage, 10.0.0.5")]);
        assert_eq!(
            defaults().client_ip(Some(ip("127.0.0.1")), &forwarded),
            Some(ip("10.0.0.5"))
        );
    }

    #[test]
    fn forwarded_hops_with_ports_are_accepted() {
        let forwarded = headers(&[("x-forwarded-for", "198.51.100.7:51234")]);
        assert_eq!(
            defaults().client_ip(Some(ip("10.1.0.4")), &forwarded),
            Some(ip("198.51.100.7"))
        );
        let forwarded_v6 = headers(&[("x-forwarded-for", "[2001:db8::7]:443")]);
        assert_eq!(
            defaults().client_ip(Some(ip("::1")), &forwarded_v6),
            Some(ip("2001:db8::7"))
        );
    }

    #[test]
    fn real_ip_header_is_used_only_from_a_trusted_proxy_without_forwarded_for() {
        let real_ip = headers(&[("x-real-ip", "198.51.100.7")]);
        assert_eq!(
            defaults().client_ip(Some(ip("127.0.0.1")), &real_ip),
            Some(ip("198.51.100.7"))
        );
        assert_eq!(
            defaults().client_ip(Some(ip("203.0.113.9")), &real_ip),
            Some(ip("203.0.113.9"))
        );
    }

    #[test]
    fn cloudflare_header_is_never_trusted() {
        let spoofed = headers(&[("cf-connecting-ip", "1.2.3.4")]);
        assert_eq!(
            defaults().client_ip(Some(ip("127.0.0.1")), &spoofed),
            Some(ip("127.0.0.1"))
        );
    }

    #[test]
    fn trusted_proxy_without_forwarding_headers_logs_the_proxy_address() {
        assert_eq!(
            defaults().client_ip(Some(ip("127.0.0.1")), &HeaderMap::new()),
            Some(ip("127.0.0.1"))
        );
    }

    #[test]
    fn ipv4_mapped_ipv6_peer_is_treated_as_ipv4() {
        let forwarded = headers(&[("x-forwarded-for", "198.51.100.7")]);
        assert_eq!(
            defaults().client_ip(Some(ip("::ffff:127.0.0.1")), &forwarded),
            Some(ip("198.51.100.7"))
        );
        assert_eq!(
            defaults().client_ip(Some(ip("::ffff:203.0.113.9")), &forwarded),
            Some(ip("203.0.113.9"))
        );
    }

    #[test]
    fn missing_peer_address_yields_no_ip_even_with_headers() {
        let forwarded = headers(&[("x-forwarded-for", "198.51.100.7")]);
        assert_eq!(defaults().client_ip(None, &forwarded), None);
    }

    #[test]
    fn none_setting_trusts_no_proxy() {
        let trusted = TrustedProxies::from_setting(Some(" None ")).expect("none setting");
        let forwarded = headers(&[("x-forwarded-for", "198.51.100.7")]);
        assert_eq!(
            trusted.client_ip(Some(ip("127.0.0.1")), &forwarded),
            Some(ip("127.0.0.1"))
        );
    }

    #[test]
    fn explicit_proxy_list_replaces_the_private_network_default() {
        let trusted =
            TrustedProxies::from_setting(Some("203.0.113.0/24, 2001:db8::1")).expect("list");
        let forwarded = headers(&[("x-forwarded-for", "198.51.100.7")]);
        assert_eq!(
            trusted.client_ip(Some(ip("203.0.113.50")), &forwarded),
            Some(ip("198.51.100.7"))
        );
        assert_eq!(
            trusted.client_ip(Some(ip("2001:db8::1")), &forwarded),
            Some(ip("198.51.100.7"))
        );
        assert_eq!(
            trusted.client_ip(Some(ip("127.0.0.1")), &forwarded),
            Some(ip("127.0.0.1"))
        );
    }

    #[test]
    fn blank_setting_uses_the_private_network_default() {
        assert_eq!(TrustedProxies::from_setting(Some("  ")), Some(defaults()));
    }

    #[test]
    fn invalid_proxy_entries_are_rejected() {
        for raw in [
            "10.0.0.0/33",
            "not-an-ip",
            "::1/129",
            "10.0.0.0/x",
            "127.0.0.1, bogus",
        ] {
            assert!(
                TrustedProxies::from_setting(Some(raw)).is_none(),
                "{raw} must be rejected"
            );
        }
    }

    #[test]
    fn zero_length_prefix_trusts_every_address_of_that_family() {
        let trusted = TrustedProxies::from_setting(Some("0.0.0.0/0")).expect("catch-all");
        let forwarded = headers(&[("x-forwarded-for", "198.51.100.7")]);
        assert_eq!(
            trusted.client_ip(Some(ip("203.0.113.9")), &forwarded),
            Some(ip("198.51.100.7"))
        );
    }
}
