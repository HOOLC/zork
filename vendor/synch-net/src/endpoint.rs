//! The iroh endpoint wrapper, with both ALPNs mounted and membership enforced
//! at accept time (§2, §3.2).

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
};

use iroh::{
    address_lookup::{PkarrPublisher, PkarrResolver},
    dns::{DnsProtocol, DnsResolver},
    endpoint::{presets, Connection, RelayMode},
    protocol::Router,
    tls::CaTlsConfig,
    Endpoint, EndpointAddr, NetReportConfig, RelayConfig, RelayMap, RelayUrl, TransportAddr,
};
use iroh_base::SecretKey;
use iroh_relay::RelayQuicConfig;
use iroh_mainline_address_lookup::DhtAddressLookup;
use synch_core::{NodeId, ALPN_BLOB, ALPN_MPT};
use synch_store::Store;

use crate::{
    blob::{BlobClient, BlobProtocol},
    error::NetError,
    mpt::{MptClient, MptProtocol},
};

/// How long a dial may take before the peer is reported unreachable.
///
/// Generous enough for hole-punching and a relay fallback; bounded so a dead
/// address costs seconds, not the 30–60 s QUIC would spend retrying, which a
/// stale binding would otherwise charge to peer exchange, adoption and every head push,
/// silently.
pub(crate) const DIAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The native control ALPN. Zork's mesh RPC rides its own protocol on this
/// endpoint instead of a published socket program, so a dial is a plain QUIC
/// connect: nothing is fetched, compiled or deployed on the way.
pub const ALPN_CONTROL: &[u8] = b"zork-control/1";

/// An object-safe control protocol mounted on the endpoint.
///
/// [`ProtocolHandler`] returns `impl Future`, so it is not object safe; the
/// endpoint keeps this shim and adapts it when mounting the router.
pub trait ControlProtocol: Send + Sync + std::fmt::Debug + 'static {
    /// Serves one inbound control connection for its lifetime.
    fn accept(
        &self,
        connection: Connection,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), iroh::protocol::AcceptError>> + Send>,
    >;
}

#[derive(Debug)]
struct ControlAdapter(Arc<dyn ControlProtocol>);

impl iroh::protocol::ProtocolHandler for ControlAdapter {
    async fn accept(
        &self,
        connection: Connection,
    ) -> Result<(), iroh::protocol::AcceptError> {
        self.0.accept(connection).await
    }
}

/// How long one request may wait for its answer before the peer is treated as
/// failed.
///
/// [`DIAL_TIMEOUT`] bounds the handshake and nothing after it, so every
/// exchange carries its own deadline: a peer that keeps a QUIC session alive
/// while answering nothing would otherwise hold a caller for as long as it
/// liked, and a stalled sync, fetch or head push has no other way to end. It
/// matches the serve side's budget for one stream, so an honest provider
/// doing real disk work for a window is never cut off; applied per exchange,
/// a windowed fetch gets the deadline once per window, not once for the walk.
pub(crate) const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Runs one client exchange under a deadline, naming what stalled.
///
/// The failure is a [`NetError`] like any other transport failure, which is what
/// puts a stalled peer on the same footing as an unreachable one: the caller
/// drops it and tries the next candidate.
pub(crate) async fn under_deadline<T>(
    deadline: std::time::Duration,
    what: &str,
    exchange: impl std::future::Future<Output = Result<T, NetError>>,
) -> Result<T, NetError> {
    match tokio::time::timeout(deadline, exchange).await {
        Ok(answered) => answered,
        Err(_) => Err(NetError::Endpoint(format!(
            "{what} went unanswered for {}s",
            deadline.as_secs_f64()
        ))),
    }
}

/// How the endpoint should be bound.
#[derive(Debug, Clone, Default)]
pub struct NetOptions {
    /// Async CAS backend mounted by the blob protocol. LocalFs is used when
    /// absent, which keeps standalone/test endpoint construction compatible.
    pub cas: Option<Arc<dyn synch_store::backend::CasBackend>>,
    /// An explicit bind address. `None` binds an ephemeral port on all
    /// interfaces.
    pub bind_addr: Option<SocketAddr>,
    /// Disables relays and address discovery entirely.
    ///
    /// Peers must then be reached by direct address. Tests use this so they
    /// never touch the network; self-hosted deployments can use it on closed
    /// networks.
    pub offline: bool,
    /// Iroh relay server URLs replacing n0's public relay map (§3.3).
    ///
    /// Self-hosted deployments point every node at their own relay here.
    /// A relay forwards encrypted QUIC traffic only — it learns neither
    /// membership nor content — so choosing one is an availability decision,
    /// not a trust one. Empty means n0's relays. Ignored when `offline`.
    pub relay_urls: Vec<String>,
    /// The UDP port the configured relays answer QUIC address discovery on,
    /// when it is not iroh's default 7842.
    ///
    /// Address discovery is what makes a direct path possible at all: a node
    /// learns the address a peer will see by probing the relay over UDP. A
    /// network that carries UDP only to particular destination ports — a
    /// gateway or proxy with a UDP allowlist — cannot probe 7842, so the
    /// relay's QUIC listener has to sit on a port that network does carry
    /// (`iroh-relay --quic-bind-addr 0.0.0.0:3478`) and the same port has to
    /// be named here. `None` keeps 7842. Applies to `relay_urls`, not to
    /// n0's default relays. Ignored when `offline`.
    pub relay_quic_port: Option<u16>,
    /// Public resolvers to re-resolve relay hosts through when the system
    /// resolver answers with addresses this machine cannot dial.
    ///
    /// A proxy that fakes DNS answers (Surge, Clash and friends hand out
    /// addresses from 198.18.0.0/15) keeps TCP working through itself while
    /// the UDP that QUIC address discovery needs is dropped, so the node is
    /// never told the address peers can reach it at and every link falls back
    /// to a relay. Relay hosts are resolved off this list in that case; every
    /// other name keeps the system resolver. Empty means the built-in list
    /// (`ZORK_MESH_PUBLIC_DNS` replaces it, and an empty value disables it).
    pub public_dns: Vec<SocketAddr>,
    /// A pkarr relay URL for address discovery — a self-hosted
    /// iroh-dns-server such as `https://dns.example.com/pkarr` — replacing
    /// n0's iroh.link publisher and resolver (§3.3).
    ///
    /// Discovery is addressing, not membership: a broken or hostile lookup
    /// can strand a dial but never redirect one, because the QUIC handshake
    /// authenticates the device key and membership is enforced at accept.
    /// `None` means n0's. Ignored when `offline`.
    pub discovery_url: Option<String>,
    /// QAD-only server URLs; these do not become relay forwarding routes.
    pub quic_discovery_urls: Vec<String>,
    /// Publishes and resolves addresses on the BitTorrent Mainline DHT as
    /// well, alongside whichever pkarr/DNS lookup is configured (§3.3).
    ///
    /// Same pkarr records, no server in the middle: the DHT holds the
    /// signed packet the pkarr relay would have held, so a node stays
    /// dialable when the discovery server is down, blocked, or simply not
    /// run. It is additive — a dial resolves through every configured
    /// lookup at once and takes whichever answers first — and it costs a
    /// UDP socket plus an hourly republish. Off by default. Ignored when
    /// `offline`.
    pub dht: bool,
    /// Bootstrap nodes for the DHT, as `HOST:PORT`, replacing mainline's
    /// public bootstrap set.
    ///
    /// This is what makes the swarm private: point every node at your own
    /// bootstrap nodes and they form a DHT of their own, reaching none of
    /// mainline's millions and reached by none of them. Empty means
    /// mainline's public bootstrap nodes. Ignored unless `dht`.
    pub dht_bootstrap: Vec<String>,
    /// Publishes direct IP addresses to the DHT, not just relay URLs.
    ///
    /// The DHT is a public, world-readable index, so by default only relay
    /// URLs go into the record — an IP address there tells anyone who asks
    /// where this node's operator sits. Turn it on for a node that already
    /// answers on a public address, where the gain is real: peers dial it
    /// straight, without a relay round trip. Ignored unless `dht`.
    pub dht_publish_direct_addrs: bool,
    /// The socket service, mounted as `sync/sock/1` when present
    /// (`docs/SOCKETS.md` §4).
    ///
    /// Absent means the ALPN is not offered at all, which is the right shape
    /// for a node that serves no sockets: a peer's dial fails at ALPN
    /// negotiation rather than after a handshake and a refusal, and a build
    /// with no eBPF runtime never advertises something it cannot do.
    pub sockets: Option<Arc<dyn crate::sock::SocketService>>,
    /// How long a stream may take to complete its `Open` handshake, per
    /// stream, before the callee drops it.
    ///
    /// The bound the shared accept path applies to every request
    /// (`crate::serve::STREAM_TIMEOUT`, 120 s) applied to the one phase of a
    /// socket stream that has no runtime of its own: a stream that never
    /// becomes an invocation would otherwise own a task and a buffer for as
    /// long as the peer keeps the connection. Once the invocation is
    /// admitted, its life is the socket runtime's deadlines, not this one —
    /// a socket that proxies is supposed to be long-lived. `None` is the
    /// default bound.
    pub sockets_open_timeout: Option<std::time::Duration>,
    /// How many inbound requests this endpoint may have in flight at once,
    /// across every connection and every ALPN it serves.
    ///
    /// `None` — the default, and the daemon's setting — leaves the only bound
    /// the per-connection one (`serve::MAX_CONCURRENT_STREAMS`), which is
    /// DESIGN §12's deliberate stance: a peer that can send a request at all is
    /// an authorized member of *this* cluster, members are extended basic trust
    /// not to DoS each other, and a member behaving abusively is a membership
    /// problem with a membership remedy.
    ///
    /// That stance does not survive being embedded. A process serving several
    /// clusters that do not trust each other — the cloud data plane, one node
    /// per hosted network (`docs/CLOUD-DATAPLANE.md` §4.1) — shares one tokio
    /// blocking pool between all of them, and every request handler on both
    /// ALPNs offloads its store work onto it. Nothing caps how many connections
    /// one peer opens, so a single member of one cluster can hold the whole
    /// pool: the store calls serialize on that node's one connection mutex, so
    /// the threads are spent *waiting*, and every other tenant's publishing,
    /// membership renewal and heartbeat queues behind them. The remedy §12
    /// names is unavailable there — org A's membership is not org B's to
    /// curate — so an embedder in that position sets a ceiling instead, and the
    /// containment becomes structural rather than social.
    ///
    /// It is a bound on concurrency, not a rate limit: an honest peer never
    /// notices one, because a request that has to wait for a slot waits
    /// microseconds. What it removes is the ability to make one endpoint's
    /// inbound work unbounded.
    pub max_inflight_requests: Option<usize>,
    /// Notified when a connection is refused because the dialing device key
    /// has no live binding (§3.4).
    ///
    /// A peer that rotated its key and whose new record this node has not
    /// resolved yet arrives exactly this way, so the refusal is the cue for an
    /// immediate DNS re-resolution. The endpoint only rings the bell; the rate
    /// limiting and the resolving belong to whoever is listening.
    pub on_unknown_key: Option<Arc<tokio::sync::Notify>>,
    /// Notified when a head flips to complete (§5.2): the unified tree just
    /// changed, and any checkout materializing it should look again.
    /// The endpoint only rings the bell.
    pub heads: Option<Arc<dyn crate::HeadSink>>,
    /// Native control protocol mounted on this endpoint, when the node is
    /// controlled by its peers (membership is checked by the handler).
    pub control: Option<Arc<dyn ControlProtocol>>,
}

impl NetOptions {
    /// Options for a loopback-only, fully offline endpoint.
    pub fn loopback() -> Self {
        NetOptions {
            bind_addr: Some("127.0.0.1:0".parse().expect("valid loopback address")),
            offline: true,
            ..NetOptions::default()
        }
    }
}

/// The resolvers relay hosts fall back to when the local DNS answers with
/// addresses this machine cannot dial. A deployment that runs its own
/// resolvers sets [`NetOptions::public_dns`] instead.
pub const DEFAULT_PUBLIC_DNS: [&str; 4] =
    ["223.5.5.5:53", "119.29.29.29:53", "1.1.1.1:53", "8.8.8.8:53"];

/// [`DEFAULT_PUBLIC_DNS`] parsed, for callers that decided a node needs it.
pub fn default_public_dns() -> Vec<SocketAddr> {
    DEFAULT_PUBLIC_DNS
        .iter()
        .filter_map(|server| server.parse().ok())
        .collect()
}

/// A resolver that reaches the real internet DNS rather than the answers the
/// local network decides to give.
pub(crate) fn public_resolver(servers: &[SocketAddr]) -> DnsResolver {
    DnsResolver::builder()
        .with_nameservers(
            servers
                .iter()
                .map(|addr| (*addr, DnsProtocol::Udp)),
        )
        .build()
}

/// Parses relay URLs into a custom relay mode, naming the offender.
///
/// `quic_port` aims the relays' QUIC address discovery at a port the local
/// network carries; `None` leaves every relay on iroh's default.
fn parse_relay_mode(
    urls: &[String],
    quic_port: Option<u16>,
) -> Result<RelayMode, NetError> {
    let mut parsed = Vec::with_capacity(urls.len());
    for raw in urls {
        let url: RelayUrl = raw
            .parse()
            .map_err(|e| NetError::Endpoint(format!("relay url {raw}: {e}")))?;
        let config = match quic_port {
            Some(port) => RelayConfig::new(url, Some(RelayQuicConfig::new(port))),
            None => RelayConfig::from(url),
        };
        parsed.push(config);
    }
    Ok(RelayMode::Custom(RelayMap::from_iter(parsed)))
}

/// Configures relay HTTPS and UDP discovery for an endpoint, retaining owned IO.
pub async fn configure_endpoint(
    mut builder: iroh::endpoint::Builder,
    options: &NetOptions,
) -> Result<(iroh::endpoint::Builder, crate::punch::RelayHttpRoute), NetError> {
    let mut route = crate::punch::RelayHttpRoute::disabled();
    if options.offline {
        builder = builder.relay_mode(RelayMode::Disabled).clear_address_lookup();
    } else {
        if let Some(raw) = &options.discovery_url {
            let url = parse_discovery_url(raw)?;
            builder = builder.clear_address_lookup()
                .address_lookup(PkarrPublisher::builder(url.clone()))
                .address_lookup(PkarrResolver::builder(url));
        }
        if options.dht {
            builder = builder.address_lookup(dht_address_lookup(&options.dht_bootstrap, options.dht_publish_direct_addrs)?);
        }
        if !options.relay_urls.is_empty() {
            builder = builder.relay_mode(parse_relay_mode(&options.relay_urls, options.relay_quic_port)?);
        }
        let dns = if options.public_dns.is_empty() { default_public_dns() } else { options.public_dns.clone() };
        let mut report = NetReportConfig::default();
        if !dns.is_empty() { report.quic_dns_resolver = Some(public_resolver(&dns)); }
        if !options.quic_discovery_urls.is_empty() {
            if let RelayMode::Custom(map) = parse_relay_mode(&options.quic_discovery_urls, options.relay_quic_port)? {
                report.quic_discovery_servers = map.relays::<Vec<_>>();
            }
        }
        builder = builder.net_report_config(report);
        route = crate::punch::relay_https_proxy().await.map_err(|error| NetError::Endpoint(error.to_string()))?;
        if let Some(proxy) = route.url() { builder = builder.proxy_url(proxy); }
    }
    if let Some(addr) = options.bind_addr {
        builder = builder.clear_ip_transports().bind_addr(addr).map_err(|error| NetError::Endpoint(error.to_string()))?;
    }
    Ok((builder, route))
}

/// Parses the pkarr relay URL address discovery publishes to and resolves
/// through, naming it on failure.
fn parse_discovery_url(raw: &str) -> Result<reqwest::Url, NetError> {
    reqwest::Url::parse(raw).map_err(|e| NetError::Endpoint(format!("discovery url {raw}: {e}")))
}

/// Checks a DHT bootstrap entry is `HOST:PORT`, naming the offender.
///
/// Mainline resolves these itself, at DHT construction, and drops whatever
/// does not resolve without a word — a typo would leave the node alone in a
/// DHT of one, publishing into the void. So the shape is checked here, where
/// there is still someone to tell. The host is left to the resolver: a name,
/// an IPv4 literal, and an IPv6 literal in brackets are all legal here, though
/// mainline is an IPv4 network and will discard the last one.
fn check_dht_bootstrap(raw: &str) -> Result<(), NetError> {
    let offender = |why: &str| NetError::Endpoint(format!("dht bootstrap node {raw}: {why}"));
    let (host, port) = raw
        .rsplit_once(':')
        .ok_or_else(|| offender("wants HOST:PORT"))?;
    if host.is_empty() {
        return Err(offender("has no host"));
    }
    if port.parse::<u16>().is_err() {
        return Err(offender("wants a port number after the colon"));
    }
    Ok(())
}

/// Builds the Mainline DHT address lookup from the DHT options.
fn dht_address_lookup(
    bootstrap: &[String],
    publish_direct_addrs: bool,
) -> Result<iroh_mainline_address_lookup::Builder, NetError> {
    let mut builder = DhtAddressLookup::builder();
    if !bootstrap.is_empty() {
        for node in bootstrap {
            check_dht_bootstrap(node)?;
        }
        let mut dht = n0_mainline::DhtBuilder::default();
        dht.bootstrap(bootstrap);
        builder = builder.dht_builder(dht);
    }
    if publish_direct_addrs {
        builder = builder.addr_filter(iroh::address_lookup::AddrFilter::unfiltered());
    }
    Ok(builder)
}

/// The live outbound connections this endpoint holds, keyed by peer and ALPN.
type Dialed = std::sync::Mutex<HashMap<(NodeId, &'static [u8]), Connection>>;

/// A [`HeadSink`](crate::HeadSink) with no head state at all.
///
/// The default when no reconciler is supplied — a bare endpoint that speaks the
/// protocol and has nothing to say through it. It answers with empty summaries
/// and empty head lists, and refuses pushed heads rather than pretending to have
/// taken them, which is what tests exercising only transport want and what a
/// misconfigured node should do rather than silently dropping heads.
#[derive(Debug)]
struct RefuseHeads;

impl crate::HeadSink for RefuseHeads {
    fn local_summaries(&self) -> Result<Vec<synch_core::HeadSummary>, NetError> {
        Ok(Vec::new())
    }

    fn observe_summaries_from(
        &self,
        _peer: synch_core::NodeId,
        _summaries: &[synch_core::HeadSummary],
        _now: i64,
    ) -> Result<(), NetError> {
        Ok(())
    }

    fn offer_head(&self, _head: &synch_core::SignedHead, _now: i64) -> Result<(), NetError> {
        Err(NetError::Unexpected(
            "this endpoint has no reconciler and cannot adopt heads".into(),
        ))
    }

    fn heads_for(
        &self,
        _origins: &[synch_core::OriginId],
    ) -> Result<Vec<synch_core::SignedHead>, NetError> {
        Ok(Vec::new())
    }
}

/// A bound endpoint serving both ALPNs.
#[derive(Debug, Clone)]
pub struct Net {
    router: Router,
    store: Arc<Store>,
    /// Handle onto the mounted socket protocol, so node shutdown can stop
    /// admission and drain final status frames before closing the endpoint.
    sockets: Option<crate::sock::SockProtocol>,
    /// One live connection per peer and ALPN, reused across requests.
    ///
    /// A QUIC session is not a request: opening one costs a handshake, a
    /// hole-punch or a relay round trip, and — until it idles out on both
    /// sides — a slot in each endpoint's connection table. Dialing per request
    /// made checkout reconciliation or a large fetch open one session *per file*, which
    /// is what turned checkout reconciliation over a big tree into thousands of
    /// handshakes: the provider filled with paths idling out, and the fetching
    /// endpoint drowned in its own churn. Streams are what a request costs
    /// here; the session is held open and shared.
    dialed: Arc<Dialed>,
    http_route: crate::punch::RelayHttpRoute,
    relay_access: Arc<crate::RelayAccess>,
}

impl Net {
    /// Binds an endpoint under `secret` and mounts both protocol handlers.
    pub async fn bind(
        store: Arc<Store>,
        secret: SecretKey,
        options: NetOptions,
    ) -> Result<Net, NetError> {
        let (builder, http_route) = configure_endpoint(
            Endpoint::builder(presets::N0).secret_key(secret).ca_tls_config(CaTlsConfig::system()),
            &options,
        ).await?;
        let endpoint = builder
            .alpns({
                let mut alpns = vec![ALPN_MPT.to_vec(), ALPN_BLOB.to_vec()];
                if options.sockets.is_some() {
                    alpns.push(synch_core::ALPN_SOCK.to_vec());
                }
                if options.control.is_some() {
                    alpns.push(ALPN_CONTROL.to_vec());
                }
                alpns
            })
            .bind()
            .await
            .map_err(|e| {
                // Name the address: "Failed to bind sockets" with no port
                // sends an operator to `synch init` when what they need is
                // another --bind.
                NetError::Endpoint(match options.bind_addr {
                    Some(addr) => {
                        format!("could not bind {addr}: {e} (is the port already in use?)")
                    }
                    None => e.to_string(),
                })
            })?;

        // One gate for the endpoint, shared by every ALPN mounted on it: the
        // resource it protects — the process's blocking pool — is shared too,
        // so a per-protocol cap would be three caps that add up to nothing in
        // particular.
        let inflight: crate::serve::Inflight = options
            .max_inflight_requests
            .map(|limit| Arc::new(tokio::sync::Semaphore::new(limit.max(1))));

        let router = Router::builder(endpoint)
            .accept(
                ALPN_MPT,
                MptProtocol::new(
                    store.clone(),
                    options
                        .heads
                        .clone()
                        .unwrap_or_else(|| Arc::new(RefuseHeads) as Arc<dyn crate::HeadSink>),
                )
                .on_unknown_key(options.on_unknown_key.clone())
                .inflight(inflight.clone()),
            )
            .accept(
                ALPN_BLOB,
                BlobProtocol::new(
                    store.clone(),
                    options.cas.clone().unwrap_or_else(|| {
                        Arc::new(synch_store::backend::LocalFs::new(store.clone()))
                    }),
                )
                .on_unknown_key(options.on_unknown_key.clone())
                .inflight(inflight.clone()),
            );
        let sockets = options.sockets.clone().map(|service| {
            crate::sock::SockProtocol::new(
                store.clone(),
                service,
                options
                    .sockets_open_timeout
                    .unwrap_or(crate::serve::STREAM_TIMEOUT),
            )
            .on_unknown_key(options.on_unknown_key.clone())
            .inflight(inflight.clone())
        });
        let router = match &sockets {
            Some(protocol) => router.accept(synch_core::ALPN_SOCK, protocol.clone()),
            None => router,
        };
        let router = match &options.control {
            Some(protocol) => router.accept(ALPN_CONTROL, ControlAdapter(protocol.clone())),
            None => router,
        };
        let router = router.spawn();
        let relay_access = crate::RelayAccess::new(router.endpoint().clone(), &options)?;

        Ok(Net {
            router,
            store,
            sockets,
            http_route,
            relay_access,
            dialed: Arc::new(std::sync::Mutex::new(HashMap::new())),
        })
    }

    /// The underlying iroh endpoint.
    pub fn endpoint(&self) -> &Endpoint {
        self.router.endpoint()
    }

    /// Updates one configured relay origin without restarting UDP or membership.
    pub async fn set_relay_access(&self, origin: &str, token: Option<&str>) -> Result<(), NetError> {
        self.relay_access.set(origin, token).await
    }

    /// The endpoint's scoped relay admission state.
    pub fn relay_access(&self) -> &Arc<crate::RelayAccess> { &self.relay_access }

    /// This endpoint's device key.
    pub fn id(&self) -> NodeId {
        self.endpoint().id()
    }

    /// The store this endpoint serves from.
    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    /// Stops this endpoint accepting new socket streams.
    pub fn stop_socket_admission(&self) {
        if let Some(protocol) = &self.sockets {
            protocol.stop();
        }
    }

    /// Waits for accepted socket streams to flush their final frames.
    pub async fn drain_socket_streams(&self) {
        if let Some(protocol) = &self.sockets {
            protocol.drain().await;
        }
    }

    /// An address carrying only this endpoint's directly bound sockets.
    ///
    /// This is what offline peers exchange: no relay, no discovery, just
    /// addresses that work on the local network.
    pub fn direct_addr(&self) -> EndpointAddr {
        EndpointAddr::from_parts(
            self.id(),
            self.endpoint()
                .bound_sockets()
                .into_iter()
                .map(TransportAddr::Ip),
        )
    }

    /// The endpoint's full address, including relay and discovered addresses
    /// when they are available.
    pub fn addr(&self) -> EndpointAddr {
        let addr = self.endpoint().addr();
        if addr.is_empty() {
            self.direct_addr()
        } else {
            addr
        }
    }

    /// Connects to a peer on the metadata ALPN, reusing a live session.
    ///
    /// A delegate may pull metadata only from a full member of its own cluster
    /// (§5.5), and this refuses before the dial: a session with a peer whose
    /// trie this node must not walk has nothing to do. Content is unaffected —
    /// see [`synch_store::Store::refuse_metadata_sync`].
    pub async fn connect_mpt(&self, addr: impl Into<EndpointAddr>) -> Result<MptClient, NetError> {
        let addr = addr.into();
        let refused = {
            let store = self.store.clone();
            let key = addr.id;
            crate::blocking::offload(move || {
                Ok(store.refuse_metadata_sync(&key, synch_core::now_ns())?)
            })
            .await?
        };
        if let Some(reason) = refused {
            return Err(NetError::Untrusted(format!(
                "{}: {reason}",
                addr.id.fmt_short()
            )));
        }
        Ok(MptClient::new(self.connect(addr, ALPN_MPT).await?))
    }

    /// Connects to a peer on the content ALPN, reusing a live session.
    pub async fn connect_blob(
        &self,
        addr: impl Into<EndpointAddr>,
    ) -> Result<BlobClient, NetError> {
        Ok(BlobClient::new(self.connect(addr, ALPN_BLOB).await?))
    }

    /// Connects to a peer on the socket ALPN.
    ///
    /// Not session-reusing, unlike the other two. A socket connection carries
    /// long-lived streams whose lifetime is the caller's business, and handing
    /// two unrelated `synch socket connect` invocations the same QUIC connection would
    /// make one of them able to close the other's.
    pub async fn connect_sock(
        &self,
        addr: impl Into<EndpointAddr>,
    ) -> Result<crate::sock::SockClient, NetError> {
        let addr = addr.into();
        let connection = self
            .endpoint()
            .connect(addr, synch_core::ALPN_SOCK)
            .await
            .map_err(|e| NetError::Endpoint(e.to_string()))?;
        Ok(crate::sock::SockClient::new(connection))
    }

    /// Connects to a peer on the native control ALPN.
    pub async fn connect_control(
        &self,
        addr: impl Into<EndpointAddr>,
    ) -> Result<Connection, NetError> {
        self.connect(addr, ALPN_CONTROL).await
    }

    async fn connect(
        &self,
        addr: impl Into<EndpointAddr>,
        alpn: &'static [u8],
    ) -> Result<Connection, NetError> {
        let addr = addr.into();
        // Checked on every request, cached session or not: trust is unilateral
        // per node, both sides must hold it for a session to work (§3.2), and a
        // binding that lapses mid-session must stop the next request rather
        // than ride the connection it was opened under.
        // On the blocking pool with every other store read: the query is one
        // indexed row, but the wait is on the store's single connection mutex,
        // and this runs on the worker driving the dial (§10).
        let trusted = {
            let store = self.store.clone();
            let key = addr.id;
            crate::blocking::offload(move || Ok(store.is_trusted_key(&key, synch_core::now_ns())?))
                .await?
        };
        if !trusted {
            // And the session we were holding goes with the trust: a binding
            // that lapsed is not a peer to keep a connection open with.
            self.forget(&addr.id);
            return Err(NetError::Untrusted(addr.id.fmt_short().to_string()));
        }
        if let Some(connection) = self.live(&addr.id, alpn) {
            return Ok(connection);
        }
        let id = addr.id;
        let peer = id.fmt_short().to_string();
        let connection =
            match tokio::time::timeout(DIAL_TIMEOUT, self.endpoint().connect(addr, alpn)).await {
                Ok(connected) => connected.map_err(|e| NetError::Endpoint(e.to_string()))?,
                Err(_) => {
                    return Err(NetError::Endpoint(format!(
                        "{peer} did not answer within {}s",
                        DIAL_TIMEOUT.as_secs()
                    )))
                }
            };
        let store = self.store.clone();
        if !crate::blocking::offload(move || Ok(store.is_trusted_key(&id, synch_core::now_ns())?)).await? {
            connection.close(403u32.into(), b"access revoked");
            return Err(NetError::Untrusted(peer));
        }
        // A concurrent dial to the same peer may have got there first. Both
        // sessions work and each caller holds its own, so the displaced one is
        // only dropped here — not closed — and lives as long as its user does.
        let _ = self.dialed().insert((id, alpn), connection.clone());
        Ok(connection)
    }

    /// The session held for a peer and ALPN, if one is still live.
    ///
    /// A session the peer closed — a lapsed binding, a restart — or one that
    /// idled out is dropped here rather than handed out, so the caller dials
    /// again instead of failing on a dead connection.
    fn live(&self, id: &NodeId, alpn: &'static [u8]) -> Option<Connection> {
        let mut dialed = self.dialed();
        match dialed.get(&(*id, alpn)) {
            Some(connection) if connection.close_reason().is_none() => Some(connection.clone()),
            Some(_) => {
                dialed.remove(&(*id, alpn));
                None
            }
            None => None,
        }
    }

    /// Closes and drops every session held with a peer.
    /// Closes cached outbound sessions immediately when an embedder revokes a peer.
    pub fn disconnect(&self, id: &NodeId) { self.forget(id); }

    fn forget(&self, id: &NodeId) {
        let dropped: Vec<Connection> = {
            let mut dialed = self.dialed();
            let keys: Vec<(NodeId, &'static [u8])> = dialed
                .keys()
                .filter(|(peer, _)| peer == id)
                .copied()
                .collect();
            keys.iter().filter_map(|key| dialed.remove(key)).collect()
        };
        for connection in dropped {
            connection.close(0u32.into(), b"untrusted");
        }
    }

    fn dialed(&self) -> std::sync::MutexGuard<'_, HashMap<(NodeId, &'static [u8]), Connection>> {
        self.dialed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Shuts the router and endpoint down cleanly.
    pub async fn shutdown(&self) -> Result<(), NetError> {
        // The held sessions go first: the endpoint closes them anyway, and
        // dropping them here means a shutdown does not race its own cache.
        self.dialed().clear();
        let result = self.router.shutdown().await.map_err(|e| NetError::Endpoint(e.to_string()));
        self.http_route.shutdown().await;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::test_store;

    /// The config-string validators share one contract: parse, or name the
    /// offender — before any socket opens.
    #[test]
    fn config_strings_parse_or_name_the_offender() {
        let offender = |parse: &dyn Fn(&str) -> Result<(), NetError>, raw: &str, why: &str| {
            let err = parse(raw).unwrap_err().to_string();
            assert!(err.contains(raw) && err.contains(why), "{raw}: {err}");
        };
        let relay = |raw: &str| parse_relay_mode(&[raw.to_string()], None).map(|_| ());
        let discovery = |raw: &str| parse_discovery_url(raw).map(|_| ());
        let dht = check_dht_bootstrap;

        // The good shapes all parse.
        assert!(matches!(
            parse_relay_mode(&["https://relay.example.com".to_string()], None).unwrap(),
            RelayMode::Custom(_)
        ));
        // The relay map carries the QUIC address-discovery port, so a network
        // that only carries UDP to one port can aim discovery at that port.
        let quic_port = |port: Option<u16>| match parse_relay_mode(
            &["https://relay.example.com".to_string()],
            port,
        )
        .unwrap()
        {
            RelayMode::Custom(map) => {
                let url: RelayUrl =
                    "https://relay.example.com".parse().expect("a relay url");
                map.get(&url)
                    .expect("the configured relay is in the map")
                    .quic
                    .as_ref()
                    .expect("a relay url carries a QUIC config")
                    .port
            }
            _ => panic!("custom relay urls build a custom relay map"),
        };
        assert_eq!(quic_port(None), 7842);
        assert_eq!(quic_port(Some(3478)), 3478);
        discovery("https://dns.example.com/pkarr").unwrap();
        for raw in [
            "router.bittorrent.com:6881",
            "10.0.0.1:6881",
            "[2001:db8::1]:6881",
        ] {
            dht(raw).unwrap();
        }
        // And every failure names the string that failed it — a typo in a
        // deployment's config is the error message, not a silent drop.
        offender(&relay, "not a url", "not a url");
        offender(&discovery, "dns.example.com/pkarr", "dns.example.com/pkarr");
        for (raw, why) in [
            ("router.bittorrent.com", "wants HOST:PORT"),
            (":6881", "has no host"),
            ("router.bittorrent.com:", "wants a port"),
        ] {
            offender(&dht, raw, why);
        }
    }

    #[tokio::test]
    async fn a_bad_bootstrap_node_fails_the_bind_before_any_socket_opens() {
        let (_dir, store) = test_store();
        let err = Net::bind(
            store,
            SecretKey::generate(),
            NetOptions {
                dht: true,
                dht_bootstrap: vec!["router.bittorrent.com".to_string()],
                ..NetOptions::default()
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("router.bittorrent.com"), "{err}");
    }

    #[tokio::test]
    async fn offline_binds_local_only_with_the_dht_asked_for() {
        let (_dir, store) = test_store();
        let net = Net::bind(
            store,
            SecretKey::generate(),
            NetOptions {
                dht: true,
                dht_bootstrap: vec!["router.bittorrent.com:6881".to_string()],
                ..NetOptions::loopback()
            },
        )
        .await
        .unwrap();
        // No relay, and every address is a loopback socket: --offline keeps
        // its promise whatever else was asked for.
        let addr = net.addr();
        assert!(addr.relay_urls().next().is_none());
        assert!(addr.ip_addrs().all(|a| a.ip().is_loopback()), "{addr:?}");
        net.shutdown().await.unwrap();
    }
}
