//! Endpoint configuration owned by Zork; no file synchronization runtime.
pub(crate) use crate::punch::RelayHttpRoute;
use anyhow::Result;
use iroh::{
    address_lookup::{PkarrPublisher, PkarrResolver},
    dns::DnsResolver,
    endpoint::{Builder, RelayMode},
    NetReportConfig, RelayConfig, RelayMap,
};
use zork_config::MeshConfig;

#[derive(Clone, Debug, Default)]
pub(crate) struct Options {
    pub offline: bool,
    pub bind_addr: Option<std::net::SocketAddr>,
    pub relay_urls: Vec<String>,
    /// Public relays in the relay map that become home relay only while none of
    /// `relay_urls` answers a latency probe.
    pub fallback_relay_urls: Vec<String>,
    pub relay_quic_port: Option<u16>,
    pub discovery_url: Option<String>,
    pub quic_discovery_urls: Vec<String>,
}
pub(crate) fn options(config: &MeshConfig) -> Result<Options> {
    let mut config = config.clone();
    let services = zork_config::services::ServicesConfig::load_from_install()?;
    let fallback_relay_urls = services.fallback_relay_urls_for(&config);
    services.apply_defaults(&mut config)?;
    Ok(Options {
        offline: config.offline,
        bind_addr: config.bind.as_deref().map(str::parse).transpose()?,
        relay_urls: config.relay_urls.unwrap_or_default(),
        fallback_relay_urls,
        relay_quic_port: config.relay_quic_port,
        discovery_url: config.discovery_url,
        quic_discovery_urls: config.quic_discovery_urls.unwrap_or_default(),
    })
}
fn relays(urls: &[String], port: Option<u16>) -> Result<RelayMap> {
    Ok(RelayMap::from_iter(relay_list(urls, port)?))
}
fn relay_list(urls: &[String], port: Option<u16>) -> Result<Vec<RelayConfig>> {
    urls.iter()
        .map(|raw| {
            let url = raw.parse()?;
            Ok(match port {
                Some(port) => RelayConfig::new(url, Some(iroh_relay::RelayQuicConfig::new(port))),
                None => RelayConfig::from(url),
            })
        })
        .collect()
}
/// The relay map: configured relays, then public fallbacks (which keep iroh's
/// default QUIC port; `relay_quic_port` describes the configured deployment).
fn relay_map(options: &Options) -> Result<RelayMap> {
    let mut list = relay_list(&options.relay_urls, options.relay_quic_port)?;
    list.extend(relay_list(&options.fallback_relay_urls, None)?);
    Ok(RelayMap::from_iter(list))
}
/// Relays this endpoint would use when online: the configured map, or iroh's
/// default map when none is configured. Empty when offline.
pub(crate) fn relay_configs(options: &Options) -> Result<Vec<std::sync::Arc<RelayConfig>>> {
    if options.offline {
        return Ok(Vec::new());
    }
    Ok(if options.relay_urls.is_empty() {
        iroh::endpoint::default_relay_mode().relay_map().relays()
    } else {
        relay_map(options)?.relays()
    })
}
pub(crate) async fn configure_endpoint(
    mut builder: Builder,
    options: &Options,
) -> Result<(Builder, RelayHttpRoute)> {
    let mut route = RelayHttpRoute::disabled();
    if options.offline {
        builder = builder
            .relay_mode(RelayMode::Disabled)
            .clear_address_lookup();
    } else {
        if let Some(url) = &options.discovery_url {
            let url = url.parse::<url::Url>()?;
            builder = builder
                .clear_address_lookup()
                .address_lookup(PkarrPublisher::builder(url.clone()))
                .address_lookup(PkarrResolver::builder(url));
        }
        let mut report = NetReportConfig::default();
        if !options.relay_urls.is_empty() {
            builder = builder.relay_mode(RelayMode::Custom(relay_map(options)?));
            // Home relay selection is by latency; without this a closer public
            // relay would replace ours (see vendor/iroh/ZORK-PATCH.md).
            report.fallback_relays = options
                .fallback_relay_urls
                .iter()
                .map(|url| url.parse())
                .collect::<Result<_, _>>()?;
        }
        // QUIC address discovery resolves relay hosts with the system resolver (iroh falls
        // back to public servers only if the system configuration cannot be read). Fixed
        // UDP resolvers such as 1.1.1.1/8.8.8.8 are blocked on some networks and are
        // hijacked by fake-IP proxies anyway; iroh skips fake-IP (198.18.0.0/15) answers.
        report.quic_dns_resolver = Some(DnsResolver::builder().with_system_defaults().build());
        if !options.quic_discovery_urls.is_empty() {
            report.quic_discovery_servers =
                relays(&options.quic_discovery_urls, options.relay_quic_port)?.relays::<Vec<_>>();
        }
        builder = builder.net_report_config(report);
        route = crate::punch::relay_https_proxy().await?;
        if let Some(proxy) = route.url() {
            builder = builder.proxy_url(proxy);
        }
    }
    if let Some(address) = options.bind_addr {
        builder = builder.clear_ip_transports().bind_addr(address)?;
    }
    Ok((builder, route))
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::{endpoint::presets, Endpoint, Watcher};
    use std::{net::SocketAddr, time::Duration};

    async fn local_relay() -> (iroh_relay::server::Server, SocketAddr) {
        let mut config = iroh_relay::server::ServerConfig::default();
        config.relay = Some(iroh_relay::server::RelayConfig::new(
            "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        ));
        let server = iroh_relay::server::Server::spawn(config).await.unwrap();
        let addr = server.http_addr().unwrap();
        (server, addr)
    }

    /// Forwards TCP to `target` after `delay`, making a relay look far away.
    async fn slow(target: SocketAddr, delay: Duration) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut inbound, _)) = listener.accept().await {
                tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    if let Ok(mut outbound) = tokio::net::TcpStream::connect(target).await {
                        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                    }
                });
            }
        });
        addr
    }

    async fn home_relay(options: &Options) -> String {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let (builder, _route) = configure_endpoint(Endpoint::builder(presets::N0), options)
            .await
            .unwrap();
        let endpoint = builder.clear_ip_transports().bind().await.unwrap();
        if tokio::time::timeout(Duration::from_secs(20), endpoint.online())
            .await
            .is_err()
        {
            panic!(
                "no home relay: {:?} {:?}",
                endpoint.net_report().get(),
                endpoint.home_relay_status().get()
            );
        }
        // Let a few reports settle (hysteresis would keep a wrong first choice).
        tokio::time::sleep(Duration::from_secs(3)).await;
        let home = endpoint.home_relay_status().get();
        endpoint.close().await;
        home.iter()
            .find(|s| s.is_connected())
            .expect("connected home relay")
            .url()
            .to_string()
    }

    fn options(primary: String, fallback: String) -> Options {
        Options {
            relay_urls: vec![primary],
            fallback_relay_urls: vec![fallback],
            discovery_url: Some("http://127.0.0.1:9/pkarr".into()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_reachable_primary_relay_stays_home_even_when_a_fallback_is_closer() {
        let (_primary, primary) = local_relay().await;
        let (_fallback, fallback) = local_relay().await;
        let far = slow(primary, Duration::from_millis(250)).await;
        let options = options(format!("http://{far}"), format!("http://{fallback}"));
        assert_eq!(home_relay(&options).await, format!("http://{far}/"));
    }

    #[tokio::test]
    async fn a_fallback_relay_becomes_home_when_the_primary_does_not_answer() {
        let (_fallback, fallback) = local_relay().await;
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let options = options(format!("http://{closed}"), format!("http://{fallback}"));
        assert_eq!(home_relay(&options).await, format!("http://{fallback}/"));
    }
}
