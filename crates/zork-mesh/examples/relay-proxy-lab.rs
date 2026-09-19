//! Real HTTPS relay, a CONNECT proxy and independent UDP QAD, all isolated.
use anyhow::{ensure, Context, Result};
use iroh::{endpoint::presets, tls::CaTlsConfig, Endpoint, EndpointAddr, EndpointId, Watcher};
use iroh_relay::server::{Access, AccessControl, ClientRequest, ConnectionId, Server};
use std::{
    collections::HashMap,
    net::Ipv4Addr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

#[derive(Debug, Default)]
struct Policy {
    expected: Mutex<HashMap<EndpointId, String>>,
    accepted: Mutex<Vec<(EndpointId, String)>>,
    active: Mutex<HashMap<ConnectionId, EndpointId>>,
    changed: tokio::sync::Notify,
}
impl AccessControl for Policy {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        let expected = self
            .expected
            .lock()
            .unwrap()
            .get(&request.endpoint_id())
            .cloned()
            .unwrap_or("first".into());
        if request.auth_token().as_deref() != Some(&expected) {
            return Access::Deny { reason: None };
        }
        self.accepted
            .lock()
            .unwrap()
            .push((request.endpoint_id(), expected));
        self.active
            .lock()
            .unwrap()
            .insert(request.connection_id(), request.endpoint_id());
        self.changed.notify_waiters();
        Access::Allow
    }
    fn on_disconnect(&self, _: EndpointId, connection: ConnectionId) {
        self.active.lock().unwrap().remove(&connection);
        self.changed.notify_waiters();
    }
}
impl Policy {
    async fn until(&self, predicate: impl Fn() -> bool) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let changed = self.changed.notified();
                if predicate() {
                    return;
                }
                changed.await;
            }
        })
        .await
        .context("relay admission did not converge")?;
        Ok(())
    }
    fn seen(&self, id: EndpointId, token: &str) -> bool {
        self.accepted
            .lock()
            .unwrap()
            .iter()
            .any(|(peer, credential)| *peer == id && credential == token)
    }
}

async fn proxy(listener: TcpListener, count: Arc<std::sync::atomic::AtomicUsize>) {
    let mut sessions = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = sessions.join_next(), if !sessions.is_empty() => {},
            incoming = listener.accept() => {
                let Ok((mut socket, _)) = incoming else { return; };
                let count = count.clone();
                sessions.spawn(async move {
                    let result: Result<()> = async {
                        let mut header = Vec::new();
                        while !header.ends_with(b"\r\n\r\n") {
                            ensure!(header.len() < 8192, "proxy header bound");
                            header.push(socket.read_u8().await?);
                        }
                        let header = String::from_utf8(header)?;
                        let target = header.strip_prefix("CONNECT ").context("expected CONNECT")?.split_whitespace().next().context("target")?;
                        let (host, port) = target.rsplit_once(':').context("proxy target")?;
                        ensure!(host == "relay.invalid", "unexpected proxy target");
                        let mut upstream = TcpStream::connect((Ipv4Addr::LOCALHOST, port.parse::<u16>()?)).await?;
                        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        socket.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
                        tokio::io::copy_bidirectional(&mut socket, &mut upstream).await?;
                        Ok(())
                    }.await;
                    let _ = result;
                });
            }
        }
    }
}
fn echo(endpoint: Endpoint) -> zork_notify::Task<()> {
    zork_notify::Task(tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        while let Some(incoming) = endpoint.accept().await {
            connections.spawn(async move {
                let connection = incoming.await?;
                while let Ok((mut send, mut recv)) = connection.accept_bi().await {
                    let bytes = recv.read_to_end(zork_mesh::MAX_FRAME).await?;
                    send.write_all(&bytes).await?;
                    send.finish()?;
                }
                Ok::<_, anyhow::Error>(())
            });
        }
    }))
}
async fn exchange(connection: &iroh::endpoint::Connection) -> Result<()> {
    let bytes =
        serde_json::to_vec(&serde_json::json!({"padding":"native control\n".repeat(8192)}))?;
    let (mut send, mut recv) = connection.open_bi().await?;
    send.write_all(&bytes).await?;
    send.finish()?;
    ensure!(
        tokio::time::timeout(
            Duration::from_secs(15),
            recv.read_to_end(zork_mesh::MAX_FRAME)
        )
        .await??
            == bytes,
        "relay byte mismatch"
    );
    Ok(())
}
async fn endpoint(
    options: &synch_net::NetOptions,
    cert: rustls::pki_types::CertificateDer<'static>,
    only_relay: bool,
) -> Result<(
    Endpoint,
    synch_net::RelayHttpRoute,
    Arc<synch_net::RelayAccess>,
)> {
    let builder = Endpoint::builder(presets::N0)
        .clear_address_lookup()
        .ca_tls_config(CaTlsConfig::custom_roots([cert]))
        .alpns(vec![synch_net::ALPN_CONTROL.to_vec()]);
    let (mut builder, route) = synch_net::configure_endpoint(builder, options).await?;
    builder = builder.clear_address_lookup();
    if only_relay {
        builder = builder.clear_ip_transports();
    }
    let endpoint = builder.bind().await?;
    let access = synch_net::RelayAccess::new(endpoint.clone(), options)?;
    Ok((endpoint, route, access))
}
async fn run(listener: std::net::TcpListener) -> Result<()> {
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let _proxy = zork_notify::Task(tokio::spawn(proxy(
        TcpListener::from_std(listener)?,
        count.clone(),
    )));
    let cert =
        rcgen::generate_simple_self_signed(vec!["relay.invalid".into(), "127.0.0.1".into()])?;
    let certificate = cert.cert.der().clone();
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_no_client_auth()
    .with_single_cert(vec![certificate.clone()], key.into())?;
    let policy = Arc::new(Policy::default());
    let mut relay = iroh_relay::server::RelayConfig::new((Ipv4Addr::LOCALHOST, 0));
    relay.tls = Some(iroh_relay::server::TlsConfig::new(
        (Ipv4Addr::LOCALHOST, 0),
        iroh_relay::server::CertConfig::Manual {
            server_config: tls.clone(),
        },
    ));
    relay.access = policy.clone();
    let mut server_config = iroh_relay::server::ServerConfig::default();
    server_config.relay = Some(relay);
    let relay = Server::spawn(server_config).await?;
    let relay_url = format!(
        "https://relay.invalid:{}",
        relay.https_addr().context("TLS relay")?.port()
    );
    let mut qad = iroh_relay::server::QuicConfig::new((Ipv4Addr::LOCALHOST, 0));
    qad.server_config = Some(tls);
    let mut qad_config = iroh_relay::server::ServerConfig::default();
    qad_config.quic = Some(qad);
    let qad = Server::spawn(qad_config).await?;
    let options = synch_net::NetOptions {
        relay_urls: vec![relay_url.clone()],
        relay_quic_port: Some(qad.quic_addr().context("QAD")?.port()),
        quic_discovery_urls: vec!["https://127.0.0.1:9".into()],
        ..Default::default()
    };
    let (a, route_a, access_a) = endpoint(&options, certificate.clone(), true).await?;
    let (b, route_b, access_b) = endpoint(&options, certificate.clone(), true).await?;
    let _echo_b = echo(b.clone());
    access_a.set(&relay_url, Some("first")).await?;
    access_b.set(&relay_url, Some("first")).await?;
    policy
        .until(|| policy.seen(a.id(), "first") && policy.seen(b.id(), "first"))
        .await?;
    let target = EndpointAddr::new(b.id()).with_relay_url(relay_url.parse()?);
    let connection = tokio::time::timeout(
        Duration::from_secs(20),
        a.connect(target.clone(), synch_net::ALPN_CONTROL),
    )
    .await??;
    ensure!(
        connection.remote_id() == b.id() && connection.paths().iter().all(|path| path.is_relay()),
        "forced relay escaped onto IP"
    );
    exchange(&connection).await?;
    access_a
        .set("https://unrelated.invalid", Some("must-not-be-forwarded"))
        .await?;
    policy
        .expected
        .lock()
        .unwrap()
        .insert(a.id(), "second".into());
    access_a.set(&relay_url, Some("second")).await?;
    policy.until(|| policy.seen(a.id(), "second")).await?;
    exchange(&connection).await?;
    access_a.set(&relay_url, None).await?;
    policy
        .until(|| {
            !policy
                .active
                .lock()
                .unwrap()
                .values()
                .any(|id| *id == a.id())
        })
        .await?;
    ensure!(
        policy
            .active
            .lock()
            .unwrap()
            .values()
            .any(|id| *id == b.id()),
        "logout closed another endpoint"
    );
    connection.close(0u32.into(), b"test reconnect");
    access_a.set(&relay_url, Some("second")).await?;
    let connection = tokio::time::timeout(
        Duration::from_secs(20),
        a.connect(target, synch_net::ALPN_CONTROL),
    )
    .await??;
    exchange(&connection).await?;
    println!("PASS: forced HTTPS relay through CONNECT with strict TLS; scoped refresh, logout and relogin without endpoint restart");

    let (c, route_c, access_c) = endpoint(&options, certificate.clone(), false).await?;
    let (d, route_d, access_d) = endpoint(&options, certificate, false).await?;
    access_c.set(&relay_url, Some("first")).await?;
    access_d.set(&relay_url, Some("first")).await?;
    let _echo_d = echo(d.clone());
    let mut reports = c.net_report().stream();
    use futures_util::StreamExt;
    let report = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(Some(report)) = reports.next().await {
                if report.udp_v4 {
                    break report;
                }
            }
        }
    })
    .await
    .context("independent QAD did not answer")?;
    let observed = report.global_v4.context("QAD IPv4")?;
    ensure!(
        c.bound_sockets()
            .iter()
            .any(|socket| socket.port() == observed.port()),
        "QAD used a throwaway socket"
    );
    ensure!(
        report
            .preferred_relay
            .as_ref()
            .is_some_and(|url| url.as_str().trim_end_matches('/') == relay_url),
        "QAD-only server became the preferred relay"
    );
    policy
        .until(|| policy.seen(c.id(), "first") && policy.seen(d.id(), "first"))
        .await?;
    let direct = tokio::time::timeout(
        Duration::from_secs(20),
        c.connect(
            EndpointAddr::new(d.id()).with_relay_url(relay_url.parse()?),
            synch_net::ALPN_CONTROL,
        ),
    )
    .await??;
    exchange(&direct).await?;
    tokio::time::timeout(Duration::from_secs(20), async {
        let mut paths = direct.paths_stream();
        while let Some(paths) = paths.next().await {
            if paths.iter().any(|path| path.is_selected() && path.is_ip()) {
                return;
            }
        }
    })
    .await
    .context("relay bootstrap did not select a direct UDP path")?;
    println!("PASS: independent UDP QAD uses the endpoint socket; relay bootstrap upgrades to direct UDP and never selects the QAD-only server as relay");
    for endpoint in [a, b, c, d] {
        endpoint.close().await;
    }
    for route in [route_a, route_b, route_c, route_d] {
        route.shutdown().await;
    }
    relay.shutdown().await?;
    qad.shutdown().await?;
    ensure!(
        count.load(std::sync::atomic::Ordering::SeqCst) >= 4,
        "proxy was bypassed"
    );
    Ok(())
}
fn main() -> Result<()> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    // Set only this isolated test process's environment, before creating threads.
    std::env::set_var("HTTPS_PROXY", format!("http://{}", listener.local_addr()?));
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()?
        .block_on(run(listener))
}
