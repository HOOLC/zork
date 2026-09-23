//! Endpoint configuration owned by Zork; no file synchronization runtime.
pub(crate) use crate::punch::RelayHttpRoute;
use anyhow::Result;
use iroh::{
    address_lookup::{PkarrPublisher, PkarrResolver},
    dns::{DnsProtocol, DnsResolver},
    endpoint::{Builder, RelayMode},
    NetReportConfig, RelayConfig, RelayMap,
};
use zork_config::MeshConfig;

#[derive(Clone, Debug, Default)]
pub(crate) struct Options {
    pub offline: bool,
    pub bind_addr: Option<std::net::SocketAddr>,
    pub relay_urls: Vec<String>,
    pub relay_quic_port: Option<u16>,
    pub discovery_url: Option<String>,
    pub quic_discovery_urls: Vec<String>,
}
pub(crate) fn options(config: &MeshConfig) -> Result<Options> {
    let mut config = config.clone();
    zork_config::services::ServicesConfig::load_from_install()?.apply_defaults(&mut config)?;
    Ok(Options {
        offline: config.offline,
        bind_addr: config.bind.as_deref().map(str::parse).transpose()?,
        relay_urls: config.relay_urls.unwrap_or_default(),
        relay_quic_port: config.relay_quic_port,
        discovery_url: config.discovery_url,
        quic_discovery_urls: config.quic_discovery_urls.unwrap_or_default(),
    })
}
fn relays(urls: &[String], port: Option<u16>) -> Result<RelayMap> {
    Ok(RelayMap::from_iter(
        urls.iter()
            .map(|raw| {
                let url = raw.parse()?;
                Ok(match port {
                    Some(port) => {
                        RelayConfig::new(url, Some(iroh_relay::RelayQuicConfig::new(port)))
                    }
                    None => RelayConfig::from(url),
                })
            })
            .collect::<Result<Vec<_>>>()?,
    ))
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
        if !options.relay_urls.is_empty() {
            builder = builder.relay_mode(RelayMode::Custom(relays(
                &options.relay_urls,
                options.relay_quic_port,
            )?));
        }
        let dns = ["1.1.1.1:53", "8.8.8.8:53"]
            .into_iter()
            .map(|address| Ok((address.parse()?, DnsProtocol::Udp)))
            .collect::<Result<Vec<_>>>()?;
        let mut report = NetReportConfig::default();
        report.quic_dns_resolver = Some(DnsResolver::builder().with_nameservers(dns).build());
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
