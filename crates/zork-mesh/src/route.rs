//! Transient, client-observed path scope. Never infer it from discovery hints.
use futures_util::{stream, StreamExt};
use iroh::{endpoint::Connection, TransportAddr};
use std::net::IpAddr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionScope {
    #[default]
    Unknown,
    Local,
    Lan,
    Public,
}

/// Classify only a selected path. Private addresses outside an attached physical
/// subnet may be VPN/routed addresses, so keep them unknown.
pub fn ip_route(remote: IpAddr, local: Option<IpAddr>) -> ConnectionScope {
    classify_ip(
        remote,
        local,
        &netdev::get_interfaces(),
        netdev::Interface::is_physical,
    )
}

fn classify_ip(
    remote: IpAddr,
    local: Option<IpAddr>,
    interfaces: &[netdev::Interface],
    is_physical: impl Fn(&netdev::Interface) -> bool,
) -> ConnectionScope {
    use ConnectionScope::*;
    if remote.is_loopback() {
        return Local;
    }
    let has_ip = |iface: &netdev::Interface, ip: IpAddr| match ip {
        IpAddr::V4(ip) => iface.ipv4.iter().any(|net| net.addr() == ip),
        IpAddr::V6(ip) => iface.ipv6.iter().any(|net| net.addr() == ip),
    };
    if interfaces.iter().any(|iface| has_ip(iface, remote)) {
        return Local;
    }
    if local.is_some_and(|ip| {
        interfaces
            .iter()
            .any(|iface| has_ip(iface, ip) && (iface.is_tun() || iface.is_point_to_point()))
    }) {
        return Unknown;
    }
    if interfaces.iter().any(|iface| {
        iface.is_up()
            && is_physical(iface)
            && !iface.is_tun()
            && !iface.is_point_to_point()
            && local.is_some_and(|ip| has_ip(iface, ip))
            && match remote {
                IpAddr::V4(ip) => iface
                    .ipv4
                    .iter()
                    .any(|net| net.prefix_len() > 0 && net.contains(&ip)),
                IpAddr::V6(ip) => iface
                    .ipv6
                    .iter()
                    .any(|net| net.prefix_len() > 0 && net.contains(&ip)),
            }
    }) {
        return Lan;
    }
    if public_ip(remote) {
        Public
    } else {
        Unknown
    }
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_documentation()
                && a != 0
                && a < 224
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 198 && (b == 18 || b == 19))
                && !(a == 192 && b == 0 && c == 0)
        }
        IpAddr::V6(ip) => {
            if let Some(ip) = ip.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(ip));
            }
            let s = ip.segments();
            // Conservatively accept global unicast, excluding special-use blocks.
            (s[0] & 0xe000) == 0x2000
                && !(s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8))
                && s[0] != 0x2002
                && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}

fn scope(path: &iroh::endpoint::Path<'_>) -> ConnectionScope {
    match path.remote_addr() {
        TransportAddr::Ip(addr) => {
            let local = match path.local_addr() {
                iroh::endpoint::LocalTransportAddr::Ip(ip) => *ip,
                _ => None,
            };
            ip_route(addr.ip(), local)
        }
        TransportAddr::Relay(url) => match url.host_str() {
            Some(host) => match host.trim_matches(['[', ']']).parse::<IpAddr>() {
                Ok(ip) if public_ip(ip) => ConnectionScope::Public,
                Ok(_) => ConnectionScope::Unknown,
                Err(_)
                    if host.contains('.')
                        && !host.ends_with(".local")
                        && !host.ends_with(".localhost") =>
                {
                    ConnectionScope::Public
                }
                _ => ConnectionScope::Unknown,
            },
            None => ConnectionScope::Unknown,
        },
        _ => ConnectionScope::Unknown,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConnectionRoute {
    pub scope: ConnectionScope,
    pub direct: bool,
}

fn snapshot(connection: &Connection) -> ConnectionRoute {
    let paths = connection.paths();
    match paths.iter().find(|path| path.is_selected()) {
        Some(path) => ConnectionRoute {
            scope: scope(&path),
            direct: path.is_ip(),
        },
        None => ConnectionRoute::default(),
    }
}

pub(crate) fn observe(
    connection: Option<Connection>,
) -> stream::BoxStream<'static, ConnectionRoute> {
    let Some(connection) = connection else {
        return stream::once(async {
            ConnectionRoute {
                scope: ConnectionScope::Local,
                direct: true,
            }
        })
        .boxed();
    };
    // Subscribe before reading the snapshot; recover from lag by re-reading it.
    let events = connection.path_events();
    let initial = snapshot(&connection);
    stream::once(async move { initial })
        .chain(stream::unfold(
            (connection, events),
            |(connection, mut events)| async move {
                events.next().await?;
                Some((snapshot(&connection), (connection, events)))
            },
        ))
        .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn physical_subnets_and_vpn_are_distinct() {
        let mut interface = netdev::Interface::dummy();
        interface.name = "zork-route-fixture".into();
        interface.flags = 0x1 | 0x2 | 0x40; // UP, BROADCAST, RUNNING
        interface.mac_addr = Some("00:1b:63:10:20:30".parse().unwrap());
        interface.ipv4 = vec!["192.168.7.2/24".parse().unwrap()];
        interface.ipv6 = vec!["2606:4700:1234:5678::2/64".parse().unwrap()];
        // Linux resolves physical hardware through sysfs, which has no entry
        // for this fixture. Supply that OS observation explicitly.
        for (remote, local) in [
            ("192.168.7.3", "192.168.7.2"),
            ("2606:4700:1234:5678::3", "2606:4700:1234:5678::2"),
        ] {
            assert_eq!(
                classify_ip(
                    remote.parse().unwrap(),
                    Some(local.parse().unwrap()),
                    &[interface.clone()],
                    |_| true,
                ),
                ConnectionScope::Lan
            );
        }
        assert_eq!(
            classify_ip(
                "192.168.7.3".parse().unwrap(),
                Some("192.168.7.2".parse().unwrap()),
                &[interface.clone()],
                |_| false,
            ),
            ConnectionScope::Unknown
        );
        assert_eq!(
            classify_ip(
                "192.168.8.3".parse().unwrap(),
                Some("192.168.7.2".parse().unwrap()),
                &[interface.clone()],
                |_| true,
            ),
            ConnectionScope::Unknown
        );
        // Same address prefix over a point-to-point tunnel isn't proof of LAN.
        interface.flags = 0x1 | 0x10 | 0x40;
        assert_eq!(
            classify_ip(
                "192.168.7.3".parse().unwrap(),
                Some("192.168.7.2".parse().unwrap()),
                &[interface],
                |_| true,
            ),
            ConnectionScope::Unknown
        );
    }

    #[tokio::test]
    async fn observes_real_direct_connection_and_closes() -> anyhow::Result<()> {
        use iroh::{endpoint::presets, Endpoint, EndpointAddr};
        let server = Endpoint::builder(presets::Minimal)
            .alpns(vec![b"zork-route-test".to_vec()])
            .bind_addr("127.0.0.1:0")?
            .bind()
            .await?;
        let client = Endpoint::builder(presets::Minimal)
            .bind_addr("127.0.0.1:0")?
            .bind()
            .await?;
        let addr = server
            .bound_sockets()
            .into_iter()
            .find(|addr| addr.is_ipv4())
            .unwrap();
        let target = EndpointAddr::new(server.id()).with_ip_addr(addr);
        let (outgoing, incoming) =
            tokio::join!(client.connect(target, b"zork-route-test"), async {
                server.accept().await.unwrap().await
            },);
        let outgoing = outgoing?;
        let incoming = incoming?;
        let mut routes = observe(Some(outgoing.clone()));
        let route = tokio::time::timeout(std::time::Duration::from_secs(5), routes.next())
            .await?
            .unwrap();
        assert!(route.direct);
        assert_eq!(route.scope, ConnectionScope::Local);
        incoming.close(0u32.into(), b"done");
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while routes.next().await.is_some() {}
        })
        .await?;
        client.close().await;
        server.close().await;
        Ok(())
    }

    #[test]
    fn public_and_ambiguous_addresses() {
        for ip in ["8.8.8.8", "2606:4700:4700::1111"] {
            assert_eq!(
                classify_ip(ip.parse().unwrap(), None, &[], |_| false),
                ConnectionScope::Public
            );
        }
        for ip in [
            "10.0.0.2",
            "100.64.0.1",
            "192.168.1.1",
            "169.254.1.2",
            "fe80::1",
            "fd00::1",
            "192.0.2.1",
            "2001:db8::1",
        ] {
            assert_eq!(
                classify_ip(ip.parse().unwrap(), None, &[], |_| false),
                ConnectionScope::Unknown
            );
        }
        assert_eq!(
            classify_ip("127.0.0.1".parse().unwrap(), None, &[], |_| false),
            ConnectionScope::Local
        );
    }
}
