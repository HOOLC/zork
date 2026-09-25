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

    /// Answers the relay WebSocket upgrade with HTTP 429 while `refuse` is set,
    /// like the Worker over budget; everything else (latency probes included) is
    /// forwarded to `target`.
    async fn admission_gate(
        target: SocketAddr,
        refuse: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut inbound, _)) = listener.accept().await {
                let refuse = refuse.clone();
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut buf = [0u8; 4096];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 64 * 1024 {
                        match inbound.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    let upgrade = head.starts_with(b"GET /relay");
                    if upgrade && refuse.load(std::sync::atomic::Ordering::SeqCst) {
                        let _ = inbound
                            .write_all(
                                b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 30\r\n\
                                  Content-Length: 0\r\nConnection: close\r\n\r\n",
                            )
                            .await;
                        return;
                    }
                    if let Ok(mut outbound) = tokio::net::TcpStream::connect(target).await {
                        if outbound.write_all(&head).await.is_ok() {
                            let _ =
                                tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                        }
                    }
                });
            }
        });
        addr
    }

    async fn wait_for_home(endpoint: &Endpoint, url: &str, within: Duration) {
        let found = tokio::time::timeout(within, async {
            loop {
                let home = endpoint.home_relay_status().get();
                if home
                    .iter()
                    .any(|s| s.is_connected() && s.url().to_string() == url)
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        assert!(
            found.is_ok(),
            "home relay is not {url}: {:?} {:?}",
            endpoint.home_relay_status().get(),
            endpoint.net_report().get()
        );
    }

    #[tokio::test]
    async fn a_relay_refusing_admission_hands_home_to_a_fallback_until_it_admits_again() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let (_primary, primary) = local_relay().await;
        let (_fallback, fallback) = local_relay().await;
        let refuse = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let gate = admission_gate(primary, refuse.clone()).await;
        let options = options(format!("http://{gate}"), format!("http://{fallback}"));
        let (builder, _route) = configure_endpoint(Endpoint::builder(presets::N0), &options)
            .await
            .unwrap();
        let endpoint = builder.clear_ip_transports().bind().await.unwrap();

        // The primary answers its latency probe but refuses the relay connection.
        wait_for_home(
            &endpoint,
            &format!("http://{fallback}/"),
            Duration::from_secs(20),
        )
        .await;

        // Admitted again: the refused relay's actor redials after its admission
        // backoff (30s +-20%) and the primary becomes home once more.
        refuse.store(false, std::sync::atomic::Ordering::SeqCst);
        wait_for_home(
            &endpoint,
            &format!("http://{gate}/"),
            Duration::from_secs(60),
        )
        .await;
        endpoint.close().await;
    }

    /// Manual lab: the production endpoint (packaged services config, system CA,
    /// IP transports) printing every net report. Not run by default.
    /// `ZORK_NET_LAB_SECS=90 RUST_LOG=iroh::net_report=debug cargo test -p zork-mesh
    ///  net_report_lab -- --ignored --nocapture`
    /// `ZORK_NET_LAB_EMBEDDED_ROOTS=1` verifies TLS with the embedded webpki roots
    /// instead of the platform verifier (which is slow for some chains on macOS).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn net_report_lab() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let secs: u64 = std::env::var("ZORK_NET_LAB_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(90);
        let options = super::options(&zork_config::MeshConfig::default()).unwrap();
        println!("options: {options:?}");
        let (builder, _route) = configure_endpoint(
            Endpoint::builder(presets::N0).ca_tls_config(
                if std::env::var_os("ZORK_NET_LAB_EMBEDDED_ROOTS").is_some() {
                    iroh::tls::CaTlsConfig::embedded()
                } else {
                    iroh::tls::CaTlsConfig::system()
                },
            ),
            &options,
        )
        .await
        .unwrap();
        let endpoint = builder.bind().await.unwrap();
        let start = std::time::Instant::now();
        let mut reports = endpoint.net_report().stream();
        let mut homes = endpoint.home_relay_status().stream();
        let deadline = tokio::time::sleep(Duration::from_secs(secs));
        tokio::pin!(deadline);
        use futures_util::StreamExt;
        loop {
            tokio::select! {
                _ = &mut deadline => break,
                Some(report) = reports.next() => {
                    println!("[{:>6.2}s] REPORT {report:?}", start.elapsed().as_secs_f64());
                }
                Some(home) = homes.next() => {
                    println!("[{:>6.2}s] HOME {home:?}", start.elapsed().as_secs_f64());
                }
            }
        }
        println!("addrs {:?}", endpoint.addr());
        endpoint.close().await;
    }

    /// Two real endpoints with the production Mesh endpoint configuration:
    /// `ZORK_PATH_LAB=server` prints its address (one JSON line) and echoes;
    /// `ZORK_PATH_LAB=client ZORK_PATH_LAB_ADDR='<json>'` connects and prints
    /// every path once a second. Diagnoses LAN connections that stay on the relay.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual two-machine lab"]
    async fn path_lab() {
        use futures_util::StreamExt;
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        const ALPN: &[u8] = b"zork/path-lab/1";
        let secs: u64 = std::env::var("ZORK_PATH_LAB_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(40);
        let options = super::options(&zork_config::MeshConfig::default()).unwrap();
        let (builder, _route) = configure_endpoint(
            Endpoint::builder(presets::N0).ca_tls_config(iroh::tls::CaTlsConfig::system()),
            &options,
        )
        .await
        .unwrap();
        let endpoint = builder.alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let start = std::time::Instant::now();
        let print_paths = |label: &str, connection: &iroh::endpoint::Connection| {
            for path in connection.paths().iter() {
                println!(
                    "[{:>6.2}s] {label} path {:?} remote={:?} local={:?} selected={} rtt={:?}",
                    start.elapsed().as_secs_f64(),
                    path.id(),
                    path.remote_addr(),
                    path.local_addr(),
                    path.is_selected(),
                    path.rtt()
                );
            }
        };
        match std::env::var("ZORK_PATH_LAB").as_deref() {
            Ok("server") => {
                endpoint.online().await;
                tokio::time::sleep(Duration::from_secs(3)).await;
                println!("ADDR {}", serde_json::to_string(&endpoint.addr()).unwrap());
                let deadline = tokio::time::sleep(Duration::from_secs(secs));
                tokio::pin!(deadline);
                loop {
                    tokio::select! {
                        _ = &mut deadline => break,
                        Some(incoming) = endpoint.accept() => {
                            let connection = incoming.await.unwrap();
                            println!("[{:>6.2}s] accepted {}", start.elapsed().as_secs_f64(), connection.remote_id());
                            let c = connection.clone();
                            tokio::spawn(async move {
                                let mut tick = tokio::time::interval(Duration::from_secs(2));
                                loop {
                                    tick.tick().await;
                                    if c.close_reason().is_some() { break; }
                                    for path in c.paths().iter() {
                                        println!("server path {:?} remote={:?} local={:?} selected={} rtt={:?}",
                                            path.id(), path.remote_addr(), path.local_addr(), path.is_selected(), path.rtt());
                                    }
                                }
                            });
                            tokio::spawn(async move {
                                while let Ok((mut send, mut recv)) = connection.accept_bi().await {
                                    let data = recv.read_to_end(1 << 16).await.unwrap_or_default();
                                    let _ = send.write_all(&data).await;
                                    let _ = send.finish();
                                }
                            });
                        }
                    }
                }
            }
            Ok("client") => {
                let addr: iroh::EndpointAddr =
                    serde_json::from_str(&std::env::var("ZORK_PATH_LAB_ADDR").unwrap()).unwrap();
                println!("connecting to {addr:?}");
                let connection = endpoint.connect(addr, ALPN).await.unwrap();
                println!("[{:>6.2}s] connected", start.elapsed().as_secs_f64());
                let mut events = connection.path_events();
                let mut tick = tokio::time::interval(Duration::from_secs(1));
                // ZORK_PATH_LAB_PING_SECS: send a request only this often (idle in between).
                let ping_every: u64 = std::env::var("ZORK_PATH_LAB_PING_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
                let mut ticks: u64 = 0;
                let deadline = tokio::time::sleep(Duration::from_secs(secs));
                tokio::pin!(deadline);
                loop {
                    tokio::select! {
                        _ = &mut deadline => break,
                        Some(event) = events.next() => println!("[{:>6.2}s] EVENT {event:?}", start.elapsed().as_secs_f64()),
                        _ = tick.tick() => {
                            print_paths("client", &connection);
                            ticks += 1;
                            if (ticks - 1) % ping_every != 0 { continue; }
                            println!("[{:>6.2}s] ping", start.elapsed().as_secs_f64());
                            if let Ok((mut send, mut recv)) = connection.open_bi().await {
                                let _ = send.write_all(b"ping").await;
                                let _ = send.finish();
                                let _ = recv.read_to_end(64).await;
                            }
                        }
                    }
                }
                connection.close(0u32.into(), b"done");
            }
            _ => println!("set ZORK_PATH_LAB=server|client"),
        }
        endpoint.close().await;
    }
}
