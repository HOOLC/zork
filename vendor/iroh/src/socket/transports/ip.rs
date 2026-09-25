use std::{
    collections::HashMap,
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    num::NonZeroUsize,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::{Duration, Instant},
};

use ipnet::{Ipv4Net, Ipv6Net};
use n0_watcher::Watchable;
use netwatch::{UdpSender, UdpSocket};
use pin_project::pin_project;
use tracing::{debug, info, trace};

use super::{RecvInfo, Transmit};
use crate::{
    metrics::{EndpointMetrics, SocketMetrics},
    util::is_proxy_fake_ip,
};

#[derive(Debug)]
pub(crate) struct IpTransport {
    config: Config,
    socket: Arc<UdpSocket>,
    local_addr: Watchable<SocketAddr>,
    metrics: Arc<SocketMetrics>,
    fake_ip_routes: FakeIpRouteGuard,
}

impl std::fmt::Display for IpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let version = if self.config.is_ipv4() { "v4" } else { "v6" };
        write!(f, "IpTransport({version})")
    }
}

/// IP transport configuration
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum Config {
    /// General IPv4 binding
    V4 {
        /// The IP address to bind on
        ip_net: Ipv4Net,
        /// The port to bind on
        port: u16,
        /// Is binding mandatory?
        is_required: bool,
        /// Is this a default route?
        is_default: bool,
    },
    /// General IPv6 binding
    V6 {
        /// The IP address to bind on
        ip_net: Ipv6Net,
        /// The scope id.
        scope_id: u32,
        /// The port to bind on
        port: u16,
        /// Is binding mandatory?
        is_required: bool,
        /// Is this a default route?
        is_default: bool,
    },
}

impl Config {
    /// Is this a v4 config.
    pub(crate) fn is_ipv4(&self) -> bool {
        matches!(self,  | Self::V4 { .. })
    }

    /// Is this a v6 config.
    pub(crate) fn is_ipv6(&self) -> bool {
        matches!(self, | Self::V6 { .. })
    }

    /// Returns the prefix len for the address.
    pub(crate) fn prefix_len(&self) -> u8 {
        match self {
            Self::V4 { ip_net, .. } => ip_net.prefix_len(),
            Self::V6 { ip_net, .. } => ip_net.prefix_len(),
        }
    }

    /// Is this a default config?
    pub(crate) fn is_default(&self) -> bool {
        match self {
            Self::V4 { is_default, .. } => *is_default,
            Self::V6 { is_default, .. } => *is_default,
        }
    }

    /// Is this required to bind.
    pub(crate) fn is_required(&self) -> bool {
        match self {
            Self::V4 { is_required, .. } => *is_required,
            Self::V6 { is_required, .. } => *is_required,
        }
    }

    pub(crate) fn is_valid_default_addr(&self, src: Option<IpAddr>, dst: SocketAddr) -> bool {
        match src {
            Some(src) => match (self, src) {
                (Self::V4 { is_default, .. }, IpAddr::V4(_)) => *is_default,
                (Self::V6 { is_default, .. }, IpAddr::V6(_)) => *is_default,
                _ => false,
            },
            None => match (self, dst) {
                (Self::V4 { is_default, .. }, SocketAddr::V4(_)) => *is_default,
                (Self::V6 { is_default, .. }, SocketAddr::V6(_)) => *is_default,
                _ => false,
            },
        }
    }

    /// Does this configuration match to send to the given `src` and `dst` address.
    pub(crate) fn is_valid_send_addr(&self, src: Option<IpAddr>, dst: SocketAddr) -> bool {
        match src {
            Some(src) => match (self, src) {
                (Self::V4 { ip_net, .. }, IpAddr::V4(src)) => {
                    ip_net.addr().is_unspecified() || ip_net.addr() == src
                }
                (Self::V6 { ip_net, .. }, IpAddr::V6(src)) => {
                    ip_net.addr().is_unspecified() || ip_net.addr() == src
                }
                _ => false,
            },
            None => {
                match (self, dst) {
                    (Self::V4 { ip_net, .. }, SocketAddr::V4(dst_v4)) => {
                        ip_net.contains(dst_v4.ip())
                    }
                    (
                        Self::V6 {
                            ip_net, scope_id, ..
                        },
                        SocketAddr::V6(dst_v6),
                    ) => {
                        if ip_net.contains(dst_v6.ip()) {
                            return true;
                        }
                        if dst_v6.ip().is_unicast_link_local() {
                            // If we have a link local interface, use the scope id
                            if *scope_id == dst_v6.scope_id() {
                                return true;
                            }
                        }
                        false
                    }
                    _ => false,
                }
            }
        }
    }
}

impl From<Config> for SocketAddr {
    fn from(value: Config) -> Self {
        match value {
            Config::V4 { ip_net, port, .. } => {
                SocketAddr::V4(SocketAddrV4::new(ip_net.addr(), port))
            }
            Config::V6 {
                ip_net,
                scope_id,
                port,
                ..
            } => SocketAddr::V6(SocketAddrV6::new(ip_net.addr(), port, 0, scope_id)),
        }
    }
}

impl IpTransport {
    pub(crate) fn bind(config: Config, metrics: Arc<SocketMetrics>) -> io::Result<Self> {
        let addr: SocketAddr = config.into();
        debug!(?addr, "binding");
        let socket = netwatch::UdpSocket::bind_full(addr).inspect_err(|err| {
            debug!(%addr, "failed to bind: {err:#}");
        })?;
        let local_addr = socket.local_addr()?;
        debug!(%addr, %local_addr, "successfully bound");
        // Currently gets updated on manual rebind
        // TODO: update when UdpSocket under the hood rebinds automatically
        let local_addr = Watchable::new(local_addr);

        Ok(Self {
            config,
            socket: Arc::new(socket),
            local_addr,
            metrics,
            fake_ip_routes: FakeIpRouteGuard::default(),
        })
    }

    /// NOTE: Receiving on a closed socket will return [`Poll::Pending`] indefinitely.
    pub(super) fn poll_recv(
        &mut self,
        cx: &mut Context,
        bufs: &mut [io::IoSliceMut<'_>],
        metas: &mut [noq_udp::RecvMeta],
        recv_infos: &mut [RecvInfo],
    ) -> Poll<io::Result<usize>> {
        assert_eq!(bufs.len(), metas.len(), "non matching bufs & metas");
        assert_eq!(
            bufs.len(),
            recv_infos.len(),
            "non matching bufs & recv_infos"
        );
        match self.socket.poll_recv_noq(cx, bufs, metas) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(n)) => {
                for i in 0..n {
                    let meta = &mut metas[i];
                    let recv_info = &mut recv_infos[i];
                    if meta.addr.is_ipv4() {
                        // The AsyncUdpSocket is an AF_INET6 socket and needs to show this
                        // as coming from an IPv4-mapped IPv6 addresses, since Noq will
                        // use those when sending on an INET6 socket.
                        let v6_ip = match meta.addr.ip() {
                            IpAddr::V4(ipv4_addr) => ipv4_addr.to_ipv6_mapped(),
                            IpAddr::V6(ipv6_addr) => ipv6_addr,
                        };
                        meta.addr = SocketAddr::new(v6_ip.into(), meta.addr.port());
                    }
                    // The transport addresses are internal to iroh and we always want those
                    // to remain the canonical address.
                    *recv_info = RecvInfo::from_addr(
                        SocketAddr::new(meta.addr.ip().to_canonical(), meta.addr.port()).into(),
                    );
                }
                Poll::Ready(Ok(n))
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
        }
    }

    pub(super) fn local_addr_watch(&self) -> n0_watcher::Direct<SocketAddr> {
        self.local_addr.watch()
    }

    pub(super) fn max_transmit_segments(&self) -> NonZeroUsize {
        self.socket.max_gso_segments()
    }

    pub(super) fn max_receive_segments(&self) -> NonZeroUsize {
        self.socket.gro_segments()
    }

    pub(super) fn may_fragment(&self) -> bool {
        self.socket.may_fragment()
    }

    pub(crate) fn bind_addr(&self) -> SocketAddr {
        self.config.into()
    }

    pub(super) fn create_network_change_sender(&self) -> IpNetworkChangeSender {
        IpNetworkChangeSender {
            socket: self.socket.clone(),
            local_addr: self.local_addr.clone(),
            fake_ip_routes: self.fake_ip_routes.clone(),
        }
    }

    pub(super) fn create_sender(&self) -> IpSender {
        let sender = self.socket.clone().create_sender();
        IpSender {
            config: self.config,
            sender,
            metrics: self.metrics.clone(),
            fake_ip_routes: self.fake_ip_routes.clone(),
        }
    }
}

#[derive(Debug)]
pub(super) struct IpNetworkChangeSender {
    socket: Arc<UdpSocket>,
    local_addr: Watchable<SocketAddr>,
    fake_ip_routes: FakeIpRouteGuard,
}

impl IpNetworkChangeSender {
    pub(super) fn rebind(&self) -> io::Result<()> {
        let old_addr = self.local_addr.get();
        self.socket.rebind()?;
        let addr = self.socket.local_addr()?;
        self.local_addr.set(addr).ok();
        trace!("rebound from {} to {}", old_addr, addr);

        Ok(())
    }

    pub(super) fn on_network_change(&self, _info: &crate::socket::Report) {
        // Routes may have changed (e.g. a proxy TUN was switched on or off).
        self.fake_ip_routes.clear();
    }
}

#[derive(Debug, Clone)]
#[pin_project]
pub(super) struct IpSender {
    config: Config,
    #[pin]
    sender: UdpSender,
    metrics: Arc<SocketMetrics>,
    fake_ip_routes: FakeIpRouteGuard,
}

impl IpSender {
    pub(super) fn is_valid_send_addr(&self, src: Option<IpAddr>, dst: &SocketAddr) -> bool {
        self.config.is_valid_send_addr(src, *dst)
    }

    pub(super) fn is_valid_default_addr(&self, src: Option<IpAddr>, dst: &SocketAddr) -> bool {
        self.config.is_valid_default_addr(src, *dst)
    }

    /// Creates a canonical socket address.
    ///
    /// We may be asked to send IPv4-mapped IPv6 addresses.  But our sockets are configured
    /// to only send their actual family.  So we need to map those back to the canonical
    /// addresses.
    #[inline]
    fn canonical_addr(addr: SocketAddr) -> SocketAddr {
        SocketAddr::new(addr.ip().to_canonical(), addr.port())
    }

    pub(super) fn poll_send(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context,
        dst: SocketAddr,
        src: Option<IpAddr>,
        transmit: &Transmit<'_>,
    ) -> Poll<io::Result<()>> {
        if self.fake_ip_routes.blocks(dst) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::NetworkUnreachable,
                "direct UDP route leaves through a fake-IP proxy TUN",
            )));
        }
        let total_bytes = transmit.contents.len() as u64;
        let res = Pin::new(&mut self.sender).poll_send(
            &noq_udp::Transmit {
                destination: Self::canonical_addr(dst),
                ecn: transmit.ecn,
                contents: transmit.contents,
                segment_size: transmit.segment_size,
                src_ip: src,
            },
            cx,
        );

        match res {
            Poll::Ready(Ok(res)) => {
                match dst {
                    SocketAddr::V4(_) => {
                        self.metrics.send_ipv4.inc_by(total_bytes);
                    }
                    SocketAddr::V6(_) => {
                        self.metrics.send_ipv6.inc_by(total_bytes);
                    }
                }
                Poll::Ready(Ok(res))
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Zork patch: refuses direct UDP whose route leaves through a fake-IP proxy TUN.
///
/// Proxy clients in TUN "fake-IP" mode (Surge enhanced mode, Clash/mihomo, ...) take the
/// default route and number their interface from `198.18.0.0/15`. A datagram routed
/// there is re-sent by the proxy from its own socket and NAT port, so the peer sees a
/// different 4-tuple than for datagrams sent directly on the LAN. QUIC treats the
/// proxied copy as another network path: when a proxied Initial wins the race the
/// server binds the handshake to the proxy's port and discards everything later sent
/// directly (or the proxy's reply pins our path to the TUN address), and the handshake
/// never completes. Such destinations are left to the relay instead, which the proxy
/// can carry correctly over TCP.
///
/// The decision uses the kernel's own route: the source address a connected UDP socket
/// gets for the destination (`connect()` on UDP sends nothing). Results are cached per
/// destination IP for [`FakeIpRouteGuard::TTL`] and dropped on network changes.
#[derive(Debug, Clone, Default)]
struct FakeIpRouteGuard {
    routes: Arc<Mutex<HashMap<IpAddr, (bool, Instant)>>>,
}

impl FakeIpRouteGuard {
    const TTL: Duration = Duration::from_secs(30);
    const MAX_ENTRIES: usize = 512;

    /// Returns `true` if direct UDP to `dst` must not be sent.
    fn blocks(&self, dst: SocketAddr) -> bool {
        let ip = dst.ip().to_canonical();
        if is_proxy_fake_ip(ip) {
            return true;
        }
        // The fake-IP range is IPv4; loopback and IPv6 never take that route.
        if !ip.is_ipv4() || ip.is_loopback() || ip.is_unspecified() {
            return false;
        }
        let now = Instant::now();
        let mut routes = self.routes.lock().expect("poisoned");
        if let Some(&(blocked, at)) = routes.get(&ip)
            && now.duration_since(at) < Self::TTL
        {
            return blocked;
        }
        let source = route_source(SocketAddr::new(ip, dst.port().max(1)));
        let blocked = routes_through_fake_ip(source);
        if blocked {
            debug!(%dst, ?source, "not sending direct UDP through a fake-IP proxy TUN");
        }
        if routes.len() >= Self::MAX_ENTRIES {
            routes.clear();
        }
        routes.insert(ip, (blocked, now));
        blocked
    }

    fn clear(&self) {
        self.routes.lock().expect("poisoned").clear();
    }
}

/// Whether a destination whose kernel-chosen source is `source` goes through a
/// fake-IP proxy TUN. An unknown route is not blocked; the real send reports errors.
fn routes_through_fake_ip(source: Option<IpAddr>) -> bool {
    source.is_some_and(is_proxy_fake_ip)
}

/// The IPv4 source address the kernel would pick for `dst`, without sending anything.
fn route_source(dst: SocketAddr) -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(dst).ok()?;
    socket.local_addr().ok().map(|addr| addr.ip())
}

#[derive(Debug, Clone)]
pub(super) struct IpTransportsSender {
    /// Stored sorted by prefix len
    v4: Vec<IpSender>,
    default_v4_index: Option<usize>,
    /// Stored sorted by prefix len
    v6: Vec<IpSender>,
    default_v6_index: Option<usize>,
}

impl IpTransportsSender {
    pub(super) fn v4_iter_mut(&mut self) -> impl Iterator<Item = &mut IpSender> {
        self.v4.iter_mut()
    }

    pub(super) fn v4_default_mut(&mut self) -> Option<&mut IpSender> {
        if let Some(i) = self.default_v4_index {
            return Some(&mut self.v4[i]);
        }
        None
    }

    pub(super) fn v6_iter_mut(&mut self) -> impl Iterator<Item = &mut IpSender> {
        self.v6.iter_mut()
    }

    pub(super) fn v6_default_mut(&mut self) -> Option<&mut IpSender> {
        if let Some(i) = self.default_v6_index {
            return Some(&mut self.v6[i]);
        }
        None
    }
}

#[derive(Debug)]
pub(super) struct IpTransports {
    v4: Vec<IpTransport>,
    default_v4_index: Option<usize>,
    v6: Vec<IpTransport>,
    default_v6_index: Option<usize>,
}

impl IpTransports {
    pub(super) fn create_sender(&self) -> IpTransportsSender {
        let ip_v4 = self.v4.iter().map(|t| t.create_sender()).collect();
        let ip_v6 = self.v6.iter().map(|t| t.create_sender()).collect();

        IpTransportsSender {
            v4: ip_v4,
            default_v4_index: self.default_v4_index,
            v6: ip_v6,
            default_v6_index: self.default_v6_index,
        }
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &IpTransport> {
        self.v4.iter().chain(self.v6.iter())
    }

    pub(super) fn bind(
        configs: impl Iterator<Item = Config>,
        metrics: &EndpointMetrics,
    ) -> io::Result<Self> {
        let mut has_v4_default = false;
        let mut ip_v4 = Vec::new();

        let mut has_v6_default = false;
        let mut ip_v6 = Vec::new();

        for config in configs {
            match IpTransport::bind(config, metrics.socket.clone()) {
                Ok(transport) => {
                    if config.is_ipv4() {
                        if config.is_default() {
                            if has_v4_default {
                                return Err(io::Error::other(
                                    "can only have a single IPv4 default transport",
                                ));
                            }
                            has_v4_default = true;
                        }
                        ip_v4.push(transport);
                    } else if config.is_ipv6() {
                        if config.is_default() {
                            if has_v6_default {
                                return Err(io::Error::other(
                                    "can only have a single IPv6 default transport",
                                ));
                            }
                            has_v6_default = true;
                        }
                        ip_v6.push(transport);
                    }
                }
                Err(err) => {
                    if config.is_required() {
                        return Err(err);
                    }
                    info!("ignoring non required bind failure: {:?}", err);
                }
            }
        }

        // Sort in descending order by prefix len
        ip_v4.sort_by_key(|i| std::cmp::Reverse(i.config.prefix_len()));
        ip_v6.sort_by_key(|i| std::cmp::Reverse(i.config.prefix_len()));

        let default_v4_index = ip_v4.iter().position(|i| i.config.is_default());
        let default_v6_index = ip_v6.iter().position(|i| i.config.is_default());

        Ok(Self {
            v4: ip_v4,
            default_v4_index,
            v6: ip_v6,
            default_v6_index,
        })
    }

    pub(super) fn iter_mut(&mut self) -> impl Iterator<Item = &mut IpTransport> {
        self.v4.iter_mut().chain(self.v6.iter_mut())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_ip_route_decision() {
        let tun: IpAddr = "198.18.0.1".parse().unwrap();
        let lan: IpAddr = "192.168.20.152".parse().unwrap();
        let tailscale: IpAddr = "100.120.232.91".parse().unwrap();
        assert!(routes_through_fake_ip(Some(tun)));
        assert!(!routes_through_fake_ip(Some(lan)));
        assert!(!routes_through_fake_ip(Some(tailscale)));
        assert!(!routes_through_fake_ip(None));
    }

    #[test]
    fn fake_ip_guard_blocks_fake_destinations_only() {
        let guard = FakeIpRouteGuard::default();
        // Fake-IP destinations are refused without consulting the route table.
        assert!(guard.blocks("198.18.1.203:7842".parse().unwrap()));
        assert!(guard.blocks("[::ffff:198.18.0.1]:1234".parse().unwrap()));
        // Loopback and IPv6 never go through the proxy TUN.
        assert!(!guard.blocks("127.0.0.1:1234".parse().unwrap()));
        assert!(!guard.blocks("[::1]:1234".parse().unwrap()));
        // The route source of loopback is loopback, and the probe sends nothing.
        assert_eq!(
            route_source("127.0.0.1:9".parse().unwrap()),
            Some(Ipv4Addr::LOCALHOST.into())
        );
        guard.clear();
    }

    #[tokio::test]
    async fn test_bind_sorting() -> n0_error::Result {
        let has_ipv6 = tokio::net::UdpSocket::bind("[::1]:0").await.is_ok();
        eprintln!("testing with ipv6? {has_ipv6}");

        let metrics = EndpointMetrics::default();
        let config = vec![
            Config::V4 {
                ip_net: Ipv4Net::new("127.0.0.1".parse().unwrap(), 8).unwrap(),
                port: 2222,
                is_required: true,
                is_default: false,
            },
            Config::V4 {
                ip_net: Ipv4Net::new("127.0.0.1".parse().unwrap(), 24).unwrap(),
                port: 1111,
                is_required: true,
                is_default: true,
            },
            Config::V4 {
                ip_net: Ipv4Net::new("127.0.0.1".parse().unwrap(), 0).unwrap(),
                port: 9999,
                is_required: true,
                is_default: false,
            },
            Config::V6 {
                ip_net: Ipv6Net::new("::1".parse().unwrap(), 4).unwrap(),
                port: 2228,
                scope_id: 0,
                is_required: has_ipv6,
                is_default: false,
            },
            Config::V6 {
                ip_net: Ipv6Net::new("::1".parse().unwrap(), 2).unwrap(),
                port: 9998,
                scope_id: 0,
                is_required: has_ipv6,
                is_default: true,
            },
            Config::V6 {
                ip_net: Ipv6Net::new("::1".parse().unwrap(), 32).unwrap(),
                port: 1118,
                scope_id: 0,
                is_required: has_ipv6,
                is_default: false,
            },
        ];

        let transports = IpTransports::bind(config.into_iter(), &metrics)?;
        assert_eq!(transports.v4[0].config.prefix_len(), 24);
        assert_eq!(transports.v4[1].config.prefix_len(), 8);
        assert_eq!(transports.v4[2].config.prefix_len(), 0);

        assert_eq!(transports.default_v4_index, Some(0));

        if has_ipv6 {
            assert_eq!(transports.v6[0].config.prefix_len(), 32);
            assert_eq!(transports.v6[1].config.prefix_len(), 4);
            assert_eq!(transports.v6[2].config.prefix_len(), 2);

            assert_eq!(transports.default_v6_index, Some(2));
        }
        Ok(())
    }
}
