# Relay credential replacement

This is the crates.io source of iroh 1.0.3, upstream commit
`f2eb930dda3779c6d852b72f3712aacd6e573ab1` (the `iroh` directory).
The original licenses, manifest, source and tests are retained.

The local patch makes a relay-map update retire an active relay actor when its
bearer credential changes or its configured relay is removed. The replacement
actor reads the current config. Upstream only schedules address discovery on
these updates, so the existing actor's ClientBuilder otherwise keeps using the
old credential, including after logout and a subsequent login.

Only relay admission lifecycle changes. Discovery, direct transports, relay
selection, DNS and hole-punching policy remain upstream behavior. Native
credential integration is exercised through Zork's CLI, Station and the Worker
by `deploy/cloudflare/test/native-lifecycle.ts`.

# Fake-IP proxy TUN handling

Proxy clients in TUN "fake-IP" mode (Surge enhanced mode, Clash/mihomo, sing-box) own the
default route and number their interface and DNS answers from 198.18.0.0/15
(`util::is_proxy_fake_ip`). Three small changes keep such addresses out of direct UDP:

- `socket.rs`: fake-IP addresses are not stored or advertised as direct addresses (and thus
  not offered as QNT candidates). Other tunnel addresses (Tailscale 100.64.0.0/10,
  WireGuard/corporate VPNs) stay, as they are real routable endpoints.
- `net_report/reportgen.rs`: fake-IP DNS answers for relay hosts are ignored for QUIC
  address discovery.
- `socket/transports/ip.rs`: `FakeIpRouteGuard` refuses direct UDP to a destination whose
  kernel-chosen source (connected-UDP probe, cached 30 s, reset on network change) is a
  fake-IP address. Such a datagram would be re-sent by the proxy from its own NAT port,
  giving the peer a second 4-tuple for the same QUIC connection; a proxied Initial that wins
  the race binds the handshake to the proxy's port and the handshake never completes. These
  peers are reached over the relay instead.

# Relay admission backoff

When a relay answers the WebSocket upgrade with HTTP 429, the active relay
actor waits 30s, doubling to 5 minutes (±20% jitter), instead of upstream's
10ms–16s exponential redial. The Zork Worker budgets relay connections per
client and replies 429 with `Retry-After` (30s for concurrency, up to the end
of the minute or UTC day for rate and volume budgets). iroh-relay 1.1 and
tokio-websockets discard response headers on a failed upgrade, so the header
value itself is not read; the status code is recognised from the dial error.
A connection that was established (received a pong) resets this backoff.
