//! Utilities used in [`iroh`](crate).

/// Creates a [`reqwest::ClientBuilder`] from a [`rustls::ClientConfig`] and our [`DnsResolver`].
///
/// In a browser context these options are not supported, so this function takes no arguments
/// if `wasm_browser` is enabled.
///
/// [`DnsResolver`]: crate::dns::DnsResolver
#[cfg(not(wasm_browser))]
pub(crate) fn reqwest_client_builder(
    tls_client_config: rustls::ClientConfig,
    dns_resolver: crate::dns::DnsResolver,
) -> reqwest::ClientBuilder {
    use self::reqwest_dns_resolver::ReqwestDnsResolver;

    reqwest::Client::builder()
        .tls_backend_preconfigured(tls_client_config)
        .dns_resolver(ReqwestDnsResolver(dns_resolver))
}

#[cfg(wasm_browser)]
pub(crate) fn reqwest_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
}

#[cfg(not(wasm_browser))]
mod reqwest_dns_resolver {
    use std::net::SocketAddr;

    use iroh_dns::dns::{DNS_TIMEOUT, DnsResolver};

    /// Implementation of [`reqwest::dns::Resolve`] for [`DnsResolver`].
    ///
    /// Wrapped in a newtype to not expose this in the public iroh API.
    pub(super) struct ReqwestDnsResolver(pub(super) DnsResolver);

    impl reqwest::dns::Resolve for ReqwestDnsResolver {
        fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
            let this = self.0.clone();
            let name = name.as_str().to_string();
            Box::pin(async move {
                let res = this.lookup_ipv4_ipv6(name, DNS_TIMEOUT).await;
                match res {
                    Ok(addrs) => {
                        let addrs: reqwest::dns::Addrs =
                            Box::new(addrs.map(|addr| SocketAddr::new(addr, 0)));
                        Ok(addrs)
                    }
                    Err(err) => {
                        let err: Box<dyn std::error::Error + Send + Sync> = Box::new(err);
                        Err(err)
                    }
                }
            })
        }
    }
}

/// Returns `true` for addresses in `198.18.0.0/15` (RFC 2544 benchmarking range).
///
/// Zork patch. Proxy clients running a TUN interface in "fake-IP" mode (Surge
/// enhanced mode, Clash/mihomo, sing-box, ...) assign themselves an address from this
/// range and hand out addresses from it in DNS answers. Such addresses are only
/// meaningful to the local proxy: advertising them as direct candidates, probing them
/// for QUIC address discovery, or sending direct UDP through the proxy (which rewrites
/// the source to its own NAT mapping and so presents the peer with a second 4-tuple for
/// the same connection) all break direct connectivity. Other tunnel addresses, e.g.
/// Tailscale's `100.64.0.0/10` or WireGuard/corporate VPN ranges, are real routable
/// endpoints and are deliberately not matched.
#[cfg_attr(wasm_browser, allow(dead_code))]
pub(crate) fn is_proxy_fake_ip(ip: std::net::IpAddr) -> bool {
    match ip.to_canonical() {
        std::net::IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            a == 198 && (b & 0xfe) == 18
        }
        std::net::IpAddr::V6(_) => false,
    }
}

#[cfg(test)]
mod proxy_fake_ip_tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::is_proxy_fake_ip;

    #[test]
    fn matches_only_the_rfc2544_range() {
        for fake in ["198.18.0.1", "198.18.1.203", "198.19.255.254"] {
            assert!(is_proxy_fake_ip(fake.parse().unwrap()), "{fake}");
        }
        let mapped = IpAddr::V6(Ipv4Addr::new(198, 18, 0, 1).to_ipv6_mapped());
        assert!(is_proxy_fake_ip(mapped));
        for real in [
            "198.17.255.255",
            "198.20.0.1",
            "192.168.20.152",
            "100.120.232.91", // Tailscale CGNAT range stays usable
            "10.0.0.1",
            "127.0.0.1",
        ] {
            assert!(!is_proxy_fake_ip(real.parse().unwrap()), "{real}");
        }
        assert!(!is_proxy_fake_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }
}
