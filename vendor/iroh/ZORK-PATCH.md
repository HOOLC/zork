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

# Net reports without QAD (fake-IP proxies)

Behind a fake-IP TUN proxy QAD cannot run (the QAD servers only resolve to fake IPs, see
above), so HTTPS latency probes are the only measurement. Upstream considers such a
report complete only once every relay of the map answered. With Zork's map (relay.zork.ing
plus four n0 fallbacks) the far fallbacks decided: on macOS each TLS handshake to an n0
relay spends 0.5-2 s in the synchronous system certificate verifier (SecTrust, Let's
Encrypt "Root YE" chain; relay.zork.ing's chain takes ~10 ms), serialised and on tokio
worker threads. use1/euc1 never answered within the 3 s probe timeout, so every report
ran into the 5 s reportgen timeout (`reportgen timed out`), the first report often carried
no latency at all and the home relay was only chosen by the second report ~25 s later.

- `net_report.rs` `have_enough_reports`: when fallback relays are configured, a report is
  complete once every admitted primary relay (not a fallback, not refusing admission)
  answered. Fallbacks only matter while no primary answers (see "Fallback relays").
- `net_report/probes.rs` `FALLBACK_HOLD`, `reportgen.rs`: while an admitted primary exists,
  HTTPS probes to fallbacks wait 1 s before starting (outside their 3 s timeout, so they
  still get the full timeout when no primary answers). The complete report aborts them, so
  in the common case the fallbacks are not contacted at all. Without an admitted primary
  nothing is held.

Without fallback relays both are inactive and reports behave as upstream; on networks
where QAD works the QAD results complete the report as before. Covered by
`net_report::tests::test_report_does_not_wait_for_fallbacks`,
`test_report_is_complete_once_admitted_primaries_answered` and
`probes::tests::test_initial_probeplan_holds_fallbacks`; `zork-mesh`
`network::tests::net_report_lab` (ignored) prints reports of the production endpoint.

# Relay admission and home selection

relay.zork.ing refuses relay connections over budget with HTTP 429 (see "Relay admission
backoff") while its `/ping` latency probe keeps answering, so it stayed "alive" and home.
`net_report::RelayAdmission` (shared by the relay actors, net_report and the socket actor)
records relays whose last dial was refused:

- The active relay actor marks its relay refused on a 429, and clears the mark once a
  connection is established or the actor exits. While refused it does not exit for
  inactivity when it is not home, so it keeps redialing with the admission backoff.
- `add_report_history_and_set_preferred_relay` treats a refused relay as not alive: it is
  skipped (and a refused primary no longer suppresses the fallbacks) unless every answering
  relay refuses.
- A change of the set re-runs the net report as a full report (socket actor,
  `UpdateReason::RelayAdmission`), so home moves to a fallback right after the 429 and back
  to the primary right after it admits the endpoint again.

Covered by `net_report::tests::test_relay_refusing_admission_is_not_alive_for_home` and
`zork-mesh` `network::tests::a_relay_refusing_admission_hands_home_to_a_fallback_until_it_admits_again`
(a local relay front answering the upgrade with 429).
