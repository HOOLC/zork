//! Runtime-owned LAN discovery. Each physical IPv4 interface recovers
//! independently, so a broken interface cannot disable working LAN paths.
use anyhow::{Context, Result};
use futures_util::{stream, StreamExt};
use iroh::{
    address_lookup::{AddressLookup, EndpointData, Error, Item},
    Endpoint, EndpointAddr, EndpointId, Watcher,
};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};
use swarm_discovery::{Discoverer, DropGuard, IpClass};
use tokio::sync::{oneshot, watch};

const RESOLVE_WINDOW: Duration = Duration::from_secs(5);
const INTERFACE_REFRESH: Duration = Duration::from_secs(5);
const HEALTHY_RESET: Duration = Duration::from_secs(60);

use crate::retry::DiscoveryBackoff;

pub fn interfaces() -> Vec<Ipv4Addr> {
    let mut interfaces: Vec<_> = netdev::get_interfaces()
        .into_iter()
        .filter(|iface| {
            iface.is_up() && !iface.is_tun() && !iface.is_point_to_point() && !iface.is_loopback()
        })
        .flat_map(|iface| iface.ipv4.into_iter().map(|network| network.addr()))
        .filter(|ip| {
            !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified() && !ip.is_multicast()
        })
        .collect();
    interfaces.sort_unstable();
    interfaces.dedup();
    interfaces
}

fn label(id: EndpointId) -> String {
    data_encoding::BASE32_NOPAD
        .encode(id.as_bytes())
        .to_ascii_lowercase()
}

type Peers = HashMap<Ipv4Addr, HashMap<String, Vec<(IpAddr, u16)>>>;
#[derive(Clone, Debug)]
struct LanLookup {
    peers: Arc<Mutex<Peers>>,
    changed: watch::Sender<u64>,
}
impl LanLookup {
    fn address(&self, id: EndpointId) -> Option<EndpointAddr> {
        let peers = self.peers.lock().ok()?;
        let label = label(id);
        let mut address = EndpointAddr::new(id);
        for routes in peers.values() {
            if let Some(addrs) = routes.get(&label) {
                for (ip, port) in addrs {
                    address = address.with_ip_addr(SocketAddr::new(*ip, *port));
                }
            }
        }
        (!address.addrs.is_empty()).then_some(address)
    }
    fn notify(&self) {
        self.changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
    fn clear(&self, interface: Ipv4Addr) {
        self.peers.lock().expect("LAN peers").remove(&interface);
        self.notify();
    }
}
impl AddressLookup for LanLookup {
    fn resolve(
        &self,
        id: EndpointId,
    ) -> Option<stream::BoxStream<'static, std::result::Result<Item, Error>>> {
        let lookup = self.clone();
        let changed = self.changed.subscribe();
        let deadline = tokio::time::Instant::now() + RESOLVE_WINDOW;
        Some(Box::pin(stream::unfold(
            (lookup, changed, deadline, None),
            move |(lookup, mut changed, deadline, mut previous)| async move {
                loop {
                    if tokio::time::Instant::now() >= deadline {
                        return None;
                    }
                    changed.borrow_and_update();
                    if let Some(address) = lookup.address(id) {
                        if previous.as_ref() != Some(&address) {
                            previous = Some(address.clone());
                            return Some((
                                Ok(Item::new(address.into(), "zork_lan", None)),
                                (lookup, changed, deadline, previous),
                            ));
                        }
                    }
                    if !matches!(
                        tokio::time::timeout_at(deadline, changed.changed()).await,
                        Ok(Ok(()))
                    ) {
                        return None;
                    }
                }
            },
        )))
    }
    fn publish(&self, _data: &EndpointData) {
        // The owned manager observes the endpoint and refreshes registrations.
    }
}

fn port_on(endpoint: &Endpoint, interface: Ipv4Addr) -> Option<u16> {
    endpoint
        .bound_sockets()
        .into_iter()
        .find(|socket| {
            socket.port() != 0
                && socket.is_ipv4()
                && (socket.ip().is_unspecified() || socket.ip() == IpAddr::V4(interface))
        })
        .map(|socket| socket.port())
}

fn announce(
    endpoint: &Endpoint,
    service: &str,
    interface: Ipv4Addr,
    routes: &[(Ipv4Addr, u16)],
    lookup: LanLookup,
) -> Result<DropGuard> {
    port_on(endpoint, interface).context("mesh_endpoint_unbound_on_interface")?;
    let mut discoverer = Discoverer::new_interactive(service.to_owned(), label(endpoint.id()));
    // Every announcement for this identity must carry the same complete set.
    // mDNS peers replace a record; publishing one NIC per record loses the
    // other NIC whenever that interface's announcement arrives last.
    for (address, port) in routes {
        discoverer = discoverer.with_addrs(*port, [IpAddr::V4(*address)]);
    }
    Ok(discoverer
        .with_ip_class(IpClass::V4Only)
        .with_multicast_interfaces_v4(vec![interface])
        .with_callback(move |id, peer| {
            let mut all = lookup.peers.lock().expect("LAN peers");
            let peers = all.entry(interface).or_default();
            if peer.is_expiry() {
                peers.remove(id);
            } else if id.len() == 52 && (peers.contains_key(id) || peers.len() < 1024) {
                peers.insert(
                    id.to_owned(),
                    peer.addrs().iter().copied().take(24).collect(),
                );
            }
            drop(all);
            lookup.notify();
        })
        .spawn(&tokio::runtime::Handle::current())?)
}

async fn stopped(stop: &mut watch::Receiver<bool>) {
    while !*stop.borrow_and_update() {
        if stop.changed().await.is_err() {
            break;
        }
    }
}

struct InterfaceTask {
    stop: watch::Sender<bool>,
    task: zork_notify::Task<()>,
    routes: Vec<(Ipv4Addr, u16)>,
}
impl InterfaceTask {
    async fn shutdown(mut self) {
        self.stop.send_replace(true);
        let _ = (&mut self.task).await;
    }
}

trait DiscoverySession: Send {
    fn failed(&mut self) -> impl std::future::Future<Output = String> + Send;
    fn shutdown(self) -> impl std::future::Future<Output = ()> + Send;
}
impl DiscoverySession for DropGuard {
    async fn failed(&mut self) -> String {
        DropGuard::failed(self).await
    }
    async fn shutdown(self) {
        DropGuard::shutdown(self).await;
    }
}

async fn maintain_interface<G: DiscoverySession>(
    mut stopping: watch::Receiver<bool>,
    mut announce: impl FnMut() -> Result<G>,
    mut clear: impl FnMut(),
    interface: Ipv4Addr,
) {
    let mut backoff = DiscoveryBackoff::default();
    loop {
        if *stopping.borrow() {
            return;
        }
        let error = match announce() {
            Ok(mut guard) => {
                let mut stable = false;
                let healthy = tokio::time::sleep(HEALTHY_RESET);
                tokio::pin!(healthy);
                let error = loop {
                    tokio::select! {
                        _ = stopped(&mut stopping) => { guard.shutdown().await; clear(); return; },
                        error = guard.failed() => break error,
                        _ = &mut healthy, if !stable => { backoff.reset(); stable = true; },
                    }
                };
                guard.shutdown().await;
                error
            }
            Err(error) => error.to_string(),
        };
        clear();
        let delay = backoff.next_delay();
        tracing::warn!(%interface, %error, retry_in_secs = delay.as_secs(), "LAN Mesh discovery retry");
        tokio::select! {
            _ = stopped(&mut stopping) => return,
            _ = tokio::time::sleep(delay) => {},
        }
    }
}

fn start_interface(
    endpoint: Endpoint,
    service: String,
    interface: Ipv4Addr,
    routes: Vec<(Ipv4Addr, u16)>,
    lookup: LanLookup,
) -> InterfaceTask {
    let (stop, stopping) = watch::channel(false);
    let published = routes.clone();
    let task = tokio::spawn(async move {
        maintain_interface(
            stopping,
            || announce(&endpoint, &service, interface, &published, lookup.clone()),
            || lookup.clear(interface),
            interface,
        )
        .await;
    });
    InterfaceTask {
        stop,
        task: zork_notify::Task(task),
        routes,
    }
}

pub(crate) struct Registration {
    stop: Option<oneshot::Sender<()>>,
    task: zork_notify::Task<()>,
}
impl Registration {
    pub(crate) async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = (&mut self.task).await;
    }
}

pub(crate) fn install(endpoint: &Endpoint, service: &str) -> Result<Registration> {
    let (changed, _) = watch::channel(0);
    let lookup = LanLookup {
        peers: Default::default(),
        changed,
    };
    endpoint.address_lookup()?.add(lookup.clone());
    let endpoint = endpoint.clone();
    let service = service.to_owned();
    let (stop, mut stopping) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut tasks = HashMap::<Ipv4Addr, InterfaceTask>::new();
        let mut addresses = endpoint.watch_addr().stream();
        let mut missing = DiscoveryBackoff::default();
        let refresh = tokio::time::sleep(Duration::ZERO);
        tokio::pin!(refresh);
        loop {
            tokio::select! {
                _ = &mut stopping => break,
                _ = endpoint.closed() => break,
                address = addresses.next() => { if address.is_none() { break; } },
                _ = &mut refresh => {},
            }
            let interfaces = tokio::task::spawn_blocking(interfaces)
                .await
                .unwrap_or_default();
            let routes: Vec<_> = interfaces
                .iter()
                .filter_map(|ip| port_on(&endpoint, *ip).map(|port| (*ip, port)))
                .collect();
            let removed: Vec<_> = tasks
                .iter()
                .filter(|(ip, task)| !interfaces.contains(ip) || task.routes != routes)
                .map(|(ip, _)| *ip)
                .collect();
            for ip in removed {
                if let Some(task) = tasks.remove(&ip) {
                    task.shutdown().await;
                }
                lookup.clear(ip);
            }
            for ip in interfaces {
                if port_on(&endpoint, ip).is_some() {
                    tasks.entry(ip).or_insert_with(|| {
                        start_interface(
                            endpoint.clone(),
                            service.clone(),
                            ip,
                            routes.clone(),
                            lookup.clone(),
                        )
                    });
                }
            }
            let delay = if tasks.is_empty() {
                missing.next_delay()
            } else {
                missing.reset();
                INTERFACE_REFRESH
            };
            refresh.as_mut().reset(tokio::time::Instant::now() + delay);
        }
        for (_, task) in tasks {
            task.shutdown().await;
        }
    });
    Ok(Registration {
        stop: Some(stop),
        task: zork_notify::Task(task),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn lookup_keeps_delivering_routes_that_arrive_after_the_first_interface() {
        let (changed, _) = watch::channel(0);
        let lookup = LanLookup {
            peers: Default::default(),
            changed,
        };
        let id = iroh::SecretKey::generate().public();
        let interface = Ipv4Addr::new(192, 168, 1, 2);
        let first: IpAddr = "192.168.100.2".parse().unwrap();
        let second: IpAddr = "192.168.1.3".parse().unwrap();
        let mut resolved = lookup.resolve(id).unwrap();
        lookup
            .peers
            .lock()
            .unwrap()
            .entry(interface)
            .or_default()
            .insert(label(id), vec![(first, 1234)]);
        lookup.notify();
        assert_eq!(
            resolved
                .next()
                .await
                .unwrap()
                .unwrap()
                .to_endpoint_addr()
                .ip_addrs()
                .count(),
            1
        );
        lookup
            .peers
            .lock()
            .unwrap()
            .get_mut(&interface)
            .unwrap()
            .get_mut(&label(id))
            .unwrap()
            .push((second, 1234));
        lookup.notify();
        assert_eq!(
            resolved
                .next()
                .await
                .unwrap()
                .unwrap()
                .to_endpoint_addr()
                .ip_addrs()
                .count(),
            2
        );
        tokio::time::advance(RESOLVE_WINDOW).await;
        assert!(resolved.next().await.is_none());
    }
    #[tokio::test]
    #[ignore = "requires an interface permitting multicast; no public or same-host lookup fallback"]
    async fn live_mdns_discovers_peer_without_any_address_hint() -> Result<()> {
        use iroh::endpoint::presets;
        anyhow::ensure!(!interfaces().is_empty(), "no multicast interface");
        let server = Endpoint::builder(presets::Minimal)
            .alpns(vec![b"zork-mdns-test".to_vec()])
            .bind_addr("0.0.0.0:0")?
            .bind()
            .await?;
        let client = Endpoint::builder(presets::Minimal)
            .bind_addr("0.0.0.0:0")?
            .bind()
            .await?;
        let server_guard = install(&server, "zork-mdns-test")?;
        let client_guard = install(&client, "zork-mdns-test")?;
        let result = tokio::time::timeout(Duration::from_secs(15), async {
            let (outgoing, incoming) =
                tokio::join!(client.connect(server.id(), b"zork-mdns-test"), async {
                    server
                        .accept()
                        .await
                        .context("listener closed")?
                        .await
                        .map_err(anyhow::Error::from)
                });
            let outgoing = outgoing?;
            let incoming = incoming?;
            anyhow::ensure!(
                outgoing.remote_id() == server.id(),
                "wrong discovered identity"
            );
            incoming.close(0u32.into(), b"mdns validated");
            Ok::<_, anyhow::Error>(())
        })
        .await;
        client_guard.shutdown().await;
        server_guard.shutdown().await;
        client.close().await;
        server.close().await;
        result??;
        Ok(())
    }
    #[test]
    fn five_attempts_per_round_without_an_unrequested_cap() {
        let mut retry = DiscoveryBackoff::default();
        assert_eq!(
            (0..12)
                .map(|_| retry.next_delay().as_secs())
                .collect::<Vec<_>>(),
            vec![1, 1, 1, 1, 1, 6, 6, 6, 6, 6, 11, 11]
        );
        for _ in 12..65 {
            retry.next_delay();
        }
        assert_eq!(retry.next_delay().as_secs(), 66);
        retry.reset();
        assert_eq!(retry.next_delay(), Duration::from_secs(1));
    }
    struct FakeSession {
        failure: tokio::sync::oneshot::Receiver<()>,
        active: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl Drop for FakeSession {
        fn drop(&mut self) {
            self.active
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    impl DiscoverySession for FakeSession {
        async fn failed(&mut self) -> String {
            let _ = (&mut self.failure).await;
            "injected socket failure".into()
        }
        async fn shutdown(self) {
            drop(self);
        }
    }
    #[tokio::test(start_paused = true)]
    async fn runtime_failures_cleanup_before_backoff_and_recovery_resets_it() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let active = Arc::new(AtomicUsize::new(0));
        let clears = Arc::new(AtomicUsize::new(0));
        let (installed, mut installs) = tokio::sync::mpsc::unbounded_channel();
        let (stop, stopping) = watch::channel(false);
        let owned = active.clone();
        let cleared = clears.clone();
        let task = tokio::spawn(maintain_interface(
            stopping,
            move || {
                assert_eq!(
                    owned.fetch_add(1, Ordering::SeqCst),
                    0,
                    "old sockets survived recreation"
                );
                let (fail, failure) = tokio::sync::oneshot::channel();
                installed.send((tokio::time::Instant::now(), fail)).unwrap();
                Ok(FakeSession {
                    failure,
                    active: owned.clone(),
                })
            },
            move || {
                cleared.fetch_add(1, Ordering::SeqCst);
            },
            Ipv4Addr::LOCALHOST,
        ));
        let (mut at, mut failure) = installs.recv().await.unwrap();
        for seconds in [1, 1, 1, 1, 1, 6, 6, 6, 6, 6, 11] {
            failure.send(()).unwrap();
            tokio::task::yield_now().await;
            assert_eq!(active.load(Ordering::SeqCst), 0);
            tokio::time::advance(Duration::from_secs(seconds) - Duration::from_millis(1)).await;
            assert!(installs.try_recv().is_err(), "retried too early");
            tokio::time::advance(Duration::from_millis(1)).await;
            let next = installs.recv().await.unwrap();
            assert_eq!(next.0.duration_since(at), Duration::from_secs(seconds));
            (at, failure) = next;
        }
        tokio::time::advance(HEALTHY_RESET).await;
        tokio::task::yield_now().await;
        failure.send(()).unwrap();
        let next = installs.recv().await.unwrap();
        assert_eq!(
            next.0.duration_since(at),
            HEALTHY_RESET + Duration::from_secs(1)
        );
        stop.send_replace(true);
        task.await.unwrap();
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert_eq!(clears.load(Ordering::SeqCst), 13);
    }
}
