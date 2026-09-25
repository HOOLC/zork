# Relay credential replacement

This is the crates.io source of iroh 1.0.3, upstream commit
`f2eb930dda3779c6d852b72f3712aacd6e573ab1` (the `iroh` directory).
The original licenses, manifest, source and tests are retained.

The local patch makes a relay-map update retire an active relay actor when its
bearer credential changes, when a relay that carried a credential is removed,
or when the map becomes empty. The replacement actor reads the current config.
Upstream only schedules address discovery on these updates, so the existing
actor's ClientBuilder otherwise keeps using the old credential, including after
logout and a subsequent login. A removed credential-bearing origin is withdrawn:
peers' discovery records cannot recreate it as an anonymous relay.

A relay without a credential that is removed while other relays remain keeps
upstream behaviour: it stays home until the next net report picks a remaining
relay, then closes once idle. Retiring it at once dropped datagrams from peers
still addressing it (upstream `endpoint_relay_map_change`). An emptied map
(enrollment giving up its relay) has no successor, so the connection closes
immediately.

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

# Fallback relays

`NetReportConfig::fallback_relays` names relays of the relay map that become the
home relay only while no other (primary) relay answered a latency probe in the
current report (`net_report.rs`, `add_report_history_and_set_preferred_relay`).
Zork ships relay.zork.ing plus n0's public relays as fallbacks. Upstream picks the
lowest-latency relay, so a device closer to an n0 region (for example in mainland
China, where Cloudflare's anycast often answers from far away) would make n0 its
home relay permanently. With the patch the fallbacks are probed as usual but are
skipped while a primary answers; when the previous home is a fallback and a
primary answers again, the hysteresis is skipped and the primary is taken back in
the next report. Relay selection among primaries, probing and dialing peers'
relays are unchanged. Covered by `net_report::tests::test_fallback_relays_only_when_no_primary_answers` and `zork-mesh` `network::tests`.
