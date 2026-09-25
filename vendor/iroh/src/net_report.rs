//! Checks the network conditions from the current host.
//!
//! NetReport is responsible for finding out the network conditions of the current host, like
//! whether it is connected to the internet via IPv4 and/or IPv6, what the NAT situation is
//! etc and reachability to the configured relays.
// Based on <https://github.com/tailscale/tailscale/blob/main/net/netcheck/netcheck.go>

#![cfg_attr(wasm_browser, allow(unused))]

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Debug,
    net::SocketAddr,
    sync::Arc,
};

use defaults::timeouts::PROBES_TIMEOUT;
use iroh_base::RelayUrl;
#[cfg(not(wasm_browser))]
use iroh_dns::dns::DnsResolver;
#[cfg(not(wasm_browser))]
use iroh_relay::{RelayConfig, quic::QuicClient};
use iroh_relay::{
    RelayMap,
    quic::{QUIC_ADDR_DISC_CLOSE_CODE, QUIC_ADDR_DISC_CLOSE_REASON},
};
use n0_error::e;
#[cfg(not(wasm_browser))]
use n0_error::stack_error;
#[cfg(not(wasm_browser))]
use n0_future::task;
use n0_future::{
    StreamExt,
    task::AbortOnDropHandle,
    time::{self, Duration, Instant},
};
use n0_watcher::{Watchable, Watcher};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace, warn};

use self::reportgen::{ProbeFinished, ProbeReport};
#[cfg(not(wasm_browser))]
use self::reportgen::{QadProbeReport, SocketState};
#[cfg_attr(not(feature = "unstable-net-report"), allow(unreachable_pub))]
pub use self::{
    // exported primarily for use in documentation
    defaults::timeouts::TIMEOUT,
    metrics::Metrics,
    probes::Probe,
    report::{RelayLatencies, Report},
};
pub(crate) use self::{
    options::Options,
    reportgen::{IfStateDetails, QuicConfig},
};

mod defaults;
mod metrics;
mod options;
mod probes;
mod report;
mod reportgen;

#[cfg(not(wasm_browser))]
#[allow(missing_docs)]
#[stack_error(derive, add_meta)]
#[non_exhaustive]
enum QadProbeError {
    #[error("Failed to resolve relay address")]
    GetRelayAddr {
        source: self::reportgen::GetRelayAddrError,
    },
    #[error("Missing host in relay URL")]
    MissingHost,
    #[error("QUIC connection failed")]
    Quic { source: iroh_relay::quic::Error },
    #[error("Receiver dropped")]
    ReceiverDropped,
}

/// Configuration for the net report component.
///
/// Controls which probes and checks are performed when generating network reports.
/// All options default to `true`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct NetReportConfig {
    /// Optional QAD-only servers, independent of relay forwarding and authentication.
    /// Empty uses the configured relays. These URLs never become home relays.
    pub quic_discovery_servers: Vec<Arc<iroh_relay::RelayConfig>>,
    /// DNS used only for UDP address discovery; HTTPS keeps the endpoint resolver.
    #[cfg(not(wasm_browser))]
    pub quic_dns_resolver: Option<DnsResolver>,
    /// Relays of the relay map that become the home relay only while none of the
    /// other (primary) relays answered a latency probe. Zork lists public fallback
    /// relays after its own relay; latency alone would pick whichever is closer.
    pub fallback_relays: Vec<RelayUrl>,
    /// Run HTTPS latency probes against relay servers.
    ///
    /// HTTPS latency probes perform an empty HTTPS GET request to each configured
    /// relay server and measure latency.
    ///
    /// They are performed in addition to the QUIC address discovery (QAD) probes.
    /// In networks that do not allow QUIC traffic, they are the only way to detect
    /// relay latencies and thus the preferred relay.
    ///
    /// Disabling them is harmless on networks that do allow QUIC traffic, but will
    /// completely prevent finding the home relay on networks that do block QUIC.
    pub https_probes: bool,

    /// Check for captive portals when generating the first report.
    ///
    /// This is done by accessing a well-known URL that is available on each relay
    /// server, `/generate_204`. If a GET request to this URL returns anything else
    /// but a 204 No Content response, we assume we are behind a captive portal.
    ///
    /// When we have detected that we are behind a captive portal, we try to contact
    /// the relay servers more frequently in case the captive portal status changes.
    pub captive_portal_check: bool,
}

impl NetReportConfig {
    /// Creates a minimal configuration that disables all optional probes and checks.
    pub fn minimal() -> Self {
        Self {
            quic_discovery_servers: Vec::new(),
            #[cfg(not(wasm_browser))]
            quic_dns_resolver: None,
            fallback_relays: Vec::new(),
            https_probes: false,
            captive_portal_check: false,
        }
    }
}

impl Default for NetReportConfig {
    fn default() -> Self {
        Self {
            quic_discovery_servers: Vec::new(),
            #[cfg(not(wasm_browser))]
            quic_dns_resolver: None,
            fallback_relays: Vec::new(),
            https_probes: true,
            captive_portal_check: true,
        }
    }
}

const FULL_REPORT_INTERVAL: Duration = Duration::from_secs(5 * 60);
const ENOUGH_ENDPOINTS: usize = 3;

/// Zork patch: relays that currently refuse this endpoint's relay connection
/// (HTTP 429 on the WebSocket upgrade).
///
/// The active relay actors write it; home relay selection reads it. A relay
/// stays refused until one of its connections is admitted again (or its actor
/// stops). Clones share state.
#[derive(Debug, Clone, Default)]
pub(crate) struct RelayAdmission {
    lock: Arc<std::sync::Mutex<()>>,
    refused: Watchable<BTreeSet<RelayUrl>>,
}

impl RelayAdmission {
    /// Marks `url` as refusing (or admitting) this endpoint.
    pub(crate) fn set_refused(&self, url: &RelayUrl, refused: bool) {
        let _guard = self.lock.lock().expect("poisoned");
        let mut set = self.refused.get();
        let changed = if refused {
            set.insert(url.clone())
        } else {
            set.remove(url)
        };
        if changed {
            let _ = self.refused.set(set);
        }
    }

    /// The relays currently refusing admission.
    pub(crate) fn refused(&self) -> BTreeSet<RelayUrl> {
        self.refused.get()
    }

    pub(crate) fn watch(&self) -> n0_watcher::Direct<BTreeSet<RelayUrl>> {
        self.refused.watch()
    }
}

/// Client to run net_reports.
#[derive(Debug)]
pub(crate) struct Client {
    #[cfg(not(wasm_browser))]
    socket_state: SocketState,
    metrics: Arc<Metrics>,
    probes: BTreeSet<Probe>,
    relay_map: RelayMap,
    #[cfg(not(wasm_browser))]
    qad_conns: QadConns,
    #[cfg(not(wasm_browser))]
    tls_config: rustls::ClientConfig,
    /// Whether to check for captive portals.
    captive_portal_check: bool,
    /// Relays chosen as home only when no primary relay answered.
    fallback_relays: BTreeSet<RelayUrl>,
    /// Relays refusing admission, and the set the previous report saw.
    admission: RelayAdmission,
    refused_at_last_report: BTreeSet<RelayUrl>,
    /// A collection of previously generated reports.
    ///
    /// Sometimes it is useful to look at past reports to decide what to do.
    reports: Reports,
}

#[cfg(not(wasm_browser))]
#[derive(Debug, Default)]
struct QadConns {
    v4: Option<(RelayUrl, QadConn)>,
    v6: Option<(RelayUrl, QadConn)>,
}

#[cfg(not(wasm_browser))]
impl QadConns {
    fn clear(&mut self) {
        if let Some((_, conn)) = self.v4.take() {
            conn.conn
                .close(QUIC_ADDR_DISC_CLOSE_CODE, QUIC_ADDR_DISC_CLOSE_REASON);
        }
        if let Some((_, conn)) = self.v6.take() {
            conn.conn
                .close(QUIC_ADDR_DISC_CLOSE_CODE, QUIC_ADDR_DISC_CLOSE_REASON);
        }
    }

    fn current_v4(&self) -> Option<ProbeReport> {
        if let Some((_, ref conn)) = self.v4
            && let Some(mut r) = conn.observer.get()
        {
            // grab latest rtt

            use noq_proto::PathId;
            if let Some(latency) = conn.conn.rtt(PathId::ZERO) {
                r.latency = latency;
            }
            return Some(ProbeReport::QadIpv4(r));
        }
        None
    }

    fn current_v6(&self) -> Option<ProbeReport> {
        if let Some((_, ref conn)) = self.v6
            && let Some(mut r) = conn.observer.get()
        {
            // grab latest rtt

            use noq_proto::PathId;
            if let Some(latency) = conn.conn.rtt(PathId::ZERO) {
                r.latency = latency;
            }
            return Some(ProbeReport::QadIpv6(r));
        }
        None
    }

    fn watch_v4(&self) -> impl n0_future::Stream<Item = Option<QadProbeReport>> + Unpin + use<> {
        let watcher = self.v4.as_ref().map(|(_url, conn)| conn.observer.watch());

        if let Some(watcher) = watcher {
            watcher.stream_updates_only().boxed()
        } else {
            n0_future::stream::empty().boxed()
        }
    }

    fn watch_v6(&self) -> impl n0_future::Stream<Item = Option<QadProbeReport>> + Unpin + use<> {
        let watcher = self.v6.as_ref().map(|(_url, conn)| conn.observer.watch());
        if let Some(watcher) = watcher {
            watcher.stream_updates_only().boxed()
        } else {
            n0_future::stream::empty().boxed()
        }
    }
}

#[cfg(not(wasm_browser))]
#[derive(Debug)]
struct QadConn {
    conn: noq::Connection,
    observer: Watchable<Option<QadProbeReport>>,
    _handle: AbortOnDropHandle<Option<()>>,
}

#[derive(Debug)]
struct Reports {
    /// Do a full relay scan, even if last is `Some`.
    next_full: bool,
    /// Some previous reports.
    prev: BTreeMap<Instant, Report>,
    /// Most recent report.
    last: Option<Report>,
    /// Time of last full (non-incremental) report.
    last_full: Instant,
}

impl Default for Reports {
    fn default() -> Self {
        Self {
            next_full: true,
            prev: Default::default(),
            last: Default::default(),
            last_full: Instant::now(),
        }
    }
}

impl Client {
    /// Creates a new net_report client.
    pub(crate) fn new(
        #[cfg(not(wasm_browser))] dns_resolver: DnsResolver,
        relay_map: RelayMap,
        opts: Options,
        metrics: Arc<Metrics>,
    ) -> Self {
        let probes = opts.as_protocols();

        #[cfg(not(wasm_browser))]
        let quic_client = opts
            .quic_config
            .map(|c| iroh_relay::quic::QuicClient::new(c.ep, c.client_config));

        #[cfg(not(wasm_browser))]
        let socket_state = SocketState {
            quic_client,
            qad_dns_resolver: opts.user_config.quic_dns_resolver.clone().unwrap_or_else(|| dns_resolver.clone()),
            qad_servers: opts.user_config.quic_discovery_servers.clone(),
            proxy_url: opts.proxy_url,
            dns_resolver,
        };

        Client {
            #[cfg(not(wasm_browser))]
            socket_state,
            metrics,
            reports: Reports::default(),
            probes,
            relay_map,
            #[cfg(not(wasm_browser))]
            qad_conns: QadConns::default(),
            #[cfg(not(wasm_browser))]
            tls_config: opts.tls_config,
            captive_portal_check: opts.user_config.captive_portal_check,
            fallback_relays: opts.user_config.fallback_relays.iter().cloned().collect(),
            admission: opts.relay_admission,
            refused_at_last_report: BTreeSet::new(),
        }
    }

    /// Relays of the map that may become home without being a fallback and
    /// that are not refusing admission.
    fn admitted_primaries(&self, refused: &BTreeSet<RelayUrl>) -> Vec<RelayUrl> {
        self.relay_map
            .urls::<Vec<_>>()
            .into_iter()
            .filter(|url| !self.fallback_relays.contains(url) && !refused.contains(url))
            .collect()
    }

    /// Fallback relays whose HTTPS probes wait for [`probes::FALLBACK_HOLD`]:
    /// all of them while an admitted primary relay exists, none otherwise.
    fn held_fallbacks(&self, refused: &BTreeSet<RelayUrl>) -> BTreeSet<RelayUrl> {
        if self.admitted_primaries(refused).is_empty() {
            BTreeSet::new()
        } else {
            self.fallback_relays.clone()
        }
    }

    pub(crate) fn has_qad_servers(&self) -> bool {
        #[cfg(not(wasm_browser))]
        { !self.socket_state.qad_servers.is_empty() }
        #[cfg(wasm_browser)]
        { false }
    }

    /// Generates a [`Report`].
    ///
    /// Look at [`Options`] for the different configuration options.
    pub(crate) async fn get_report(
        &mut self,
        if_state: IfStateDetails,
        is_major: bool,
        shutdown_token: CancellationToken,
    ) -> Report {
        let now = Instant::now();

        let mut do_full = is_major
            || self.reports.next_full
            || now.duration_since(self.reports.last_full) > FULL_REPORT_INTERVAL;

        debug!(%do_full, "net_report starting");

        // If the last report had a captive portal and reported no UDP access,
        // it's possible that we didn't get a useful net_report due to the
        // captive portal blocking us. If so, make this report a full (non-incremental) one.
        if !do_full
            && let Some(ref last) = self.reports.last
            && !last.has_udp()
            && last.captive_portal == Some(true)
        {
            do_full = true;
        }
        // Zork patch: a relay started or stopped refusing admission. Probe every
        // relay so home selection sees current latencies (an incremental report
        // after a complete one probes nothing and would keep the home relay).
        let refused = self.admission.refused();
        if refused != self.refused_at_last_report {
            debug!(?refused, "relay admission changed");
            do_full = true;
        }
        let held = self.held_fallbacks(&refused);
        if do_full {
            self.reports.last = None; // causes ProbePlan::new below to do a full (initial) plan
            self.reports.next_full = false;
            self.reports.last_full = now;
            self.metrics.reports_full.inc();
        }
        self.metrics.reports.inc();

        let num_relays = self.relay_map.len();
        let enough_relays = std::cmp::min(num_relays, ENOUGH_ENDPOINTS);
        #[cfg(wasm_browser)]
        let if_state = IfStateDetails::default();
        #[cfg(not(wasm_browser))]
        let if_state = IfStateDetails {
            have_v4: if_state.have_v4,
            have_v6: if_state.have_v6,
        };

        let mut report = Report::default();

        // Start the reportgen client to start any needed probes
        let (actor, mut probe_rx) = reportgen::Client::new(
            self.reports.last.clone(),
            self.relay_map.clone(),
            self.probes.clone(),
            held,
            self.captive_portal_check,
            if_state.clone(),
            shutdown_token.child_token(),
            #[cfg(not(wasm_browser))]
            self.socket_state.clone(),
            #[cfg(not(wasm_browser))]
            self.tls_config.clone(),
        );

        #[cfg(not(wasm_browser))]
        let reports = self
            .spawn_qad_probes(
                &if_state,
                enough_relays,
                do_full,
                shutdown_token.child_token(),
            )
            .await;

        #[cfg(not(wasm_browser))]
        for r in reports {
            report.update(&r);
        }

        if self.have_enough_reports(&if_state, do_full, num_relays, &refused, &report) {
            // check if we already have enough probes immediately after QAD returns
            trace!("have enough probe reports, aborting further probes");
            // shuts down the probes
            drop(actor);
        } else {
            #[cfg(not(wasm_browser))]
            let mut qad_v4_stream = self.qad_conns.watch_v4();
            #[cfg(wasm_browser)]
            let mut qad_v4_stream = n0_future::stream::empty::<Option<()>>();
            #[cfg(not(wasm_browser))]
            let mut qad_v6_stream = self.qad_conns.watch_v6();
            #[cfg(wasm_browser)]
            let mut qad_v6_stream = n0_future::stream::empty::<Option<()>>();

            loop {
                tokio::select! {
                    biased;

                    Some(Some(r)) = qad_v4_stream.next() => {
                        #[cfg(not(wasm_browser))]
                        {
                            trace!(?r, "new report from QAD V4");
                            report.update(&ProbeReport::QadIpv4(r));
                        }
                    }

                    Some(Some(r)) = qad_v6_stream.next() => {
                        #[cfg(not(wasm_browser))]
                        {
                            trace!(?r, "new report from QAD V6");
                            report.update(&ProbeReport::QadIpv6(r));
                        }
                    }

                    maybe_probe = probe_rx.recv() => {
                        let Some(probe_res) = maybe_probe else {
                            break;
                        };
                        match probe_res {
                            ProbeFinished::Regular(probe) => match probe {
                                Ok(probe) => {
                                    report.update(&probe);
                                    if self.have_enough_reports(&if_state, do_full, num_relays, &refused, &report) {
                                        trace!("have enough probe reports, aborting further probes");
                                        // shuts down the probes
                                        drop(actor);
                                        break;
                                    }
                                }
                                Err(err) => {
                                    trace!("probe failed: {:?}", err);
                                }
                            },
                            #[cfg(not(wasm_browser))]
                            ProbeFinished::CaptivePortal(portal) => {
                                report.captive_portal = portal;
                            }
                        }
                    }
                }
            }
        }
        self.add_report_history_and_set_preferred_relay(&mut report);
        debug!(
            ?report,
            duration = ?now.elapsed(),
            "net_report generated",
        );

        report
    }

    #[cfg(not(wasm_browser))]
    async fn spawn_qad_probes(
        &mut self,
        if_state: &IfStateDetails,
        enough_relays: usize,
        do_full: bool,
        shutdown_token: CancellationToken,
    ) -> Vec<ProbeReport> {
        use tracing::{Instrument, info_span};

        let Some(ref quic_client) = self.socket_state.quic_client else {
            return Vec::new();
        };

        if do_full {
            // clear out existing connections if we are doing a full reset
            self.qad_conns.clear();
        }

        if let Some((url, conn)) = &self.qad_conns.v4 {
            // verify conn is still around
            if let Some(reason) = conn.conn.close_reason() {
                trace!(?url, "QAD v4 conn closed: {}", reason);
                self.qad_conns.v4.take();
            }
        }
        if let Some((url, conn)) = &self.qad_conns.v6 {
            // verify conn is still around
            if let Some(reason) = conn.conn.close_reason() {
                trace!(?url, "QAD v6 conn closed: {}", reason);
                self.qad_conns.v6.take();
            }
        }

        let v4_report = self.qad_conns.current_v4();
        let v6_report = self.qad_conns.current_v6();
        let needs_v4_probe = v4_report.is_none();
        let needs_v6_probe = v6_report.is_some() != if_state.have_v6;

        let mut reports = Vec::new();

        if let Some(report) = v4_report {
            reports.push(report);
        }
        if let Some(report) = v6_report {
            reports.push(report);
        }

        if !needs_v4_probe && !needs_v6_probe {
            return reports;
        }

        trace!("spawning QAD probes");

        // TODO: randomize choice?
        const MAX_RELAYS: usize = 5;

        let mut v4_buf = JoinSet::new();
        let cancel_v4 = shutdown_token.child_token();
        let mut v6_buf = JoinSet::new();
        let cancel_v6 = shutdown_token.child_token();

        let relays = if self.socket_state.qad_servers.is_empty() {
            self.relay_map.relays::<Vec<_>>()
        } else {
            self.socket_state.qad_servers.clone()
        };
        for relay in relays.into_iter().filter(|relay| relay.quic.is_some()).take(MAX_RELAYS) {
            if if_state.have_v4 && needs_v4_probe {
                trace!(?relay.url, "v4 QAD probe starting");
                let relay = relay.clone();
                let dns_resolver = self.socket_state.qad_dns_resolver.clone();
                let quic_client = quic_client.clone();
                let relay_url = relay.url.clone();
                let inner_token = cancel_v4.child_token();
                v4_buf.spawn(
                    cancel_v4
                        .child_token()
                        .run_until_cancelled_owned(time::timeout(
                            PROBES_TIMEOUT,
                            run_probe_v4(relay, quic_client, dns_resolver, inner_token),
                        ))
                        .instrument(info_span!("QADv4", %relay_url)),
                );
            }
            if if_state.have_v6 && needs_v6_probe {
                trace!(?relay.url, "v6 QAD probe starting");
                let relay = relay.clone();
                let dns_resolver = self.socket_state.qad_dns_resolver.clone();
                let quic_client = quic_client.clone();
                let relay_url = relay.url.clone();
                let inner_token = cancel_v6.child_token();
                v6_buf.spawn(
                    cancel_v6
                        .child_token()
                        .run_until_cancelled_owned(time::timeout(
                            PROBES_TIMEOUT,
                            run_probe_v6(relay, quic_client, dns_resolver, inner_token),
                        ))
                        .instrument(info_span!("QADv6", %relay_url)),
                );
            }
        }

        // We set _pending to true if at least one report was started for each category.
        // If we did not start any report for either category, _pending is set to false right away
        // (it "completed" in the sense that nothing will ever run). If we did start at least one report,
        // _pending is set to true, and will be set to false further down once the first task
        // completed.
        let mut ipv4_pending = !v4_buf.is_empty();
        let mut ipv6_pending = !v6_buf.is_empty();

        while !v4_buf.is_empty() || !v6_buf.is_empty() {
            // We early-abort the tasks once we have at least `enough_relays` reports,
            // and at least one ipv4 and one ipv6 report completed (if they were started, see comment above).

            if reports.len() >= enough_relays && !ipv4_pending && !ipv6_pending {
                debug!("enough probes: {}", reports.len());
                cancel_v4.cancel();
                cancel_v6.cancel();
                break;
            }

            tokio::select! {
                biased;

                _ = shutdown_token.cancelled() => {
                    trace!("qad report cancelled");
                    break;
                }

                val = v4_buf.join_next(), if !v4_buf.is_empty() => {
                    let span = info_span!("QADv4");
                    let _guard = span.enter();
                    ipv4_pending = false;
                    match val {
                        Some(Ok(Some(Ok(res)))) => {
                            match res {
                                Ok((r, conn)) => {
                                    debug!(?r, "probe report");
                                    let url = r.relay.clone();
                                    reports.push(ProbeReport::QadIpv4(r));
                                    if self.qad_conns.v4.is_none() {
                                        self.qad_conns.v4.replace((url, conn));
                                    } else {
                                        conn.conn.close(QUIC_ADDR_DISC_CLOSE_CODE, QUIC_ADDR_DISC_CLOSE_REASON);
                                    }
                                }
                                Err(err) => {
                                    debug!("probe failed: {err:#}");
                                }
                            }
                        }
                        Some(Err(err)) => {
                            if err.is_panic() {
                                panic!("probe panicked: {err:#}");
                            }
                            warn!("probe failed: {err:#}");
                        }
                        Some(Ok(None)) => {
                            debug!("probe canceled");
                        }
                        Some(Ok(Some(Err(time::Elapsed { .. })))) => {
                            debug!("probe timed out");
                        }
                        None => {
                            trace!("report canceled");
                        }
                    }
                }
                val = v6_buf.join_next(), if !v6_buf.is_empty() => {
                    let span = info_span!("QADv6");
                    let _guard = span.enter();
                    ipv6_pending = false;
                    match val {
                        Some(Ok(Some(Ok(res)))) => {
                            match res {
                                Ok((r, conn)) => {
                                    debug!(?r, "probe report");
                                    let url = r.relay.clone();
                                    reports.push(ProbeReport::QadIpv6(r));
                                    if self.qad_conns.v6.is_none() {
                                        self.qad_conns.v6.replace((url, conn));
                                    } else {
                                        conn.conn.close(QUIC_ADDR_DISC_CLOSE_CODE, QUIC_ADDR_DISC_CLOSE_REASON);
                                    }
                                }
                                Err(err) => {
                                    debug!("probe failed: {err:#}");
                                }
                            }
                        }
                        Some(Err(err)) => {
                            if err.is_panic() {
                                panic!("probe panicked: {err:#}");
                            }
                            warn!("probe failed: {err:#}");
                        }
                        Some(Ok(None)) => {
                            debug!("probe canceled");
                        }
                        Some(Ok(Some(Err(time::Elapsed { .. })))) => {
                            debug!("probe timed out");
                        }
                        None => {
                            trace!("report canceled");
                        }
                    }
                }
                else => {
                    break;
                }
            }
        }

        // make sure to cancel all outstanding reports
        v4_buf.abort_all();
        v6_buf.abort_all();

        reports
    }

    /// Check if we have enough information to consider the current report "good enough".
    fn have_enough_reports(
        &self,
        state: &IfStateDetails,
        do_full: bool,
        num_relays: usize,
        refused: &BTreeSet<RelayUrl>,
        report: &Report,
    ) -> bool {
        // Zork patch: fallback relays only matter while no primary relay answers,
        // so a report in which every admitted primary answered is complete without
        // waiting for the (often far) fallbacks. Behind a fake-IP proxy QAD cannot
        // run and HTTPS probes are the only measurement; waiting for all relays
        // made every such report run into the reportgen timeout.
        if !self.fallback_relays.is_empty() {
            let primaries = self.admitted_primaries(refused);
            if !primaries.is_empty()
                && primaries.iter().all(|primary| {
                    report.relay_latency.iter().any(|(_, url, _)| url == primary)
                })
            {
                return true;
            }
        }

        #[cfg_attr(wasm_browser, allow(unused_mut))]
        let mut num_ipv4 = 0;
        #[cfg_attr(wasm_browser, allow(unused_mut))]
        let mut num_ipv6 = 0;
        let mut num_https = 0;
        for (typ, url, _) in report.relay_latency.iter() {
            // An independent QAD server says nothing about forwarding latency.
            if self.relay_map.get(url).is_none() { continue; }
            match typ {
                #[cfg(not(wasm_browser))]
                Probe::QadIpv4 => {
                    num_ipv4 += 1;
                }
                #[cfg(not(wasm_browser))]
                Probe::QadIpv6 => {
                    num_ipv6 += 1;
                }
                Probe::Https => {
                    num_https += 1;
                }
            }
        }

        if do_full {
            // Full report, require more probes
            match (state.have_v4, state.have_v6) {
                (true, true) => {
                    // Both IPv4 and IPv6 are expected to be available
                    if num_ipv4 >= 2 && num_ipv6 >= 1 || num_ipv6 >= 2 && num_ipv4 >= 1 {
                        return true;
                    }
                }
                (true, false) => {
                    // Just Ipv4 is expected
                    if num_ipv4 >= 2 {
                        return true;
                    }
                }
                (false, true) => {
                    // Just Ipv6 is expected
                    if num_ipv6 >= 2 {
                        return true;
                    }
                }
                (false, false) => {}
            }
            if num_https >= num_relays {
                // If we have at least one https probe per relay, we are happy
                return true;
            }
            false
        } else {
            // Incremental reports, here the requirements are reduced further
            match (state.have_v4, state.have_v6) {
                (true, true) => {
                    // Both IPv4 and IPv6 are expected to be available
                    if num_ipv4 >= 1 && num_ipv6 >= 1 {
                        return true;
                    }
                }
                (true, false) => {
                    // Just Ipv4 is expected
                    if num_ipv4 >= 1 {
                        return true;
                    }
                }
                (false, true) => {
                    // Just Ipv6 is expected
                    if num_ipv6 >= 1 {
                        return true;
                    }
                }
                (false, false) => {}
            }
            if num_https >= num_relays {
                // If we have at least one https probe per relay, we are happy
                return true;
            }
            false
        }
    }

    /// Adds `r` to the set of recent Reports and mutates `r.preferred_relay` to contain the best recent one.
    fn add_report_history_and_set_preferred_relay(&mut self, r: &mut Report) {
        let mut prev_relay = None;
        if let Some(ref last) = self.reports.last {
            prev_relay.clone_from(&last.preferred_relay);

            // If we don't have new information, copy this from the last report
            if r.mapping_varies_by_dest_ipv4.is_none() {
                r.mapping_varies_by_dest_ipv4 = last.mapping_varies_by_dest_ipv4;
            }
            if r.mapping_varies_by_dest_ipv6.is_none() {
                r.mapping_varies_by_dest_ipv6 = last.mapping_varies_by_dest_ipv6;
            }
        }

        let now = Instant::now();
        const MAX_AGE: Duration = Duration::from_secs(5 * 60);

        // relay ID => its best recent latency in last MAX_AGE
        let mut best_recent = RelayLatencies::default();

        // chain the current report as we are still mutating it
        let prevs_iter = self
            .reports
            .prev
            .iter()
            .map(|(a, b)| -> (&Instant, &Report) { (a, b) });

        let mut to_remove = Vec::new();
        for (t, pr) in prevs_iter {
            if now.duration_since(*t) > MAX_AGE {
                to_remove.push(*t);
                continue;
            }
            best_recent.merge(&pr.relay_latency);
        }
        // merge in current run
        best_recent.merge(&r.relay_latency);

        for t in to_remove {
            self.reports.prev.remove(&t);
        }

        // Then, pick which currently-alive relay server from the
        // current report has the best latency over the past MAX_AGE.
        let mut best_any = Duration::default();
        let mut old_relay_cur_latency = Duration::default();
        // Fallback relays compete only while no primary relay answered in this
        // report. Skipping them also lifts the hysteresis below when the previous
        // home was a fallback, so the primary is taken back as soon as it answers.
        //
        // A relay refusing admission (HTTP 429 on the relay connection) still
        // answers latency probes but cannot serve as home, so it is not alive
        // either, unless every answering relay refuses.
        let refused = self.admission.refused();
        let admitted = |url: &RelayUrl| self.relay_map.get(url).is_some() && !refused.contains(url);
        let skip_refused = r.relay_latency.iter().any(|(_, url, _)| admitted(url));
        let primary_alive = r
            .relay_latency
            .iter()
            .any(|(_, url, _)| admitted(url) && !self.fallback_relays.contains(url));
        {
            for (_, url, duration) in r.relay_latency.iter() {
                if self.relay_map.get(url).is_none() { continue; }
                if skip_refused && refused.contains(url) { continue; }
                if primary_alive && self.fallback_relays.contains(url) { continue; }
                if Some(url) == prev_relay.as_ref() {
                    old_relay_cur_latency = duration;
                }
                if let Some(best) = best_recent.get(url)
                    && (r.preferred_relay.is_none() || best < best_any)
                {
                    best_any = best;
                    r.preferred_relay.replace(url.clone());
                }
            }

            // If we're changing our preferred relay but the old one's still
            // accessible and the new one's not much better, just stick with
            // where we are.
            if prev_relay.is_some()
                && r.preferred_relay != prev_relay
                && !old_relay_cur_latency.is_zero()
                && best_any > old_relay_cur_latency / 3 * 2
            {
                r.preferred_relay = prev_relay;
            }
        }

        self.refused_at_last_report = refused;
        self.reports.prev.insert(now, r.clone());
        self.reports.last = Some(r.clone());
    }
}

#[cfg(not(wasm_browser))]
async fn run_probe_v4(
    relay: Arc<RelayConfig>,
    quic_client: QuicClient,
    dns_resolver: DnsResolver,
    shutdown_token: CancellationToken,
) -> n0_error::Result<(QadProbeReport, QadConn), QadProbeError> {
    use noq_proto::PathId;

    let relay_addr = reportgen::get_relay_addr_ipv4(&dns_resolver, &relay)
        .await
        .map_err(|source| e!(QadProbeError::GetRelayAddr { source }))?;

    trace!(?relay_addr, "resolved relay server address");
    let host = relay
        .url
        .host_str()
        .ok_or_else(|| e!(QadProbeError::MissingHost))?;
    let conn = quic_client
        .create_conn(relay_addr.into(), host)
        .await
        .map_err(|source| e!(QadProbeError::Quic { source }))?;

    let mut watcher = conn.observed_external_addr();

    // wait for an addr
    let addr = watcher
        .next()
        .await
        .ok_or_else(|| e!(QadProbeError::ReceiverDropped))?;
    let report = QadProbeReport {
        relay: relay.url.clone(),
        addr: SocketAddr::new(addr.ip().to_canonical(), addr.port()),
        latency: conn.rtt(PathId::ZERO).unwrap_or_default(),
    };

    let observer = Watchable::new(None);
    let endpoint = relay.url.clone();
    let handle = task::spawn(shutdown_token.run_until_cancelled_owned({
        let conn = conn.clone();
        let observer = observer.clone();
        async move {
            while let Some(val) = watcher.next().await {
                // if we've sent to an ipv4 address, but received an observed address
                // that is ivp6 then the address is an [IPv4-Mapped IPv6 Addresses](https://doc.rust-lang.org/beta/std/net/struct.Ipv6Addr.html#ipv4-mapped-ipv6-addresses)
                let val = SocketAddr::new(val.ip().to_canonical(), val.port());
                let latency = conn.rtt(PathId::ZERO).unwrap_or_default();
                observer
                    .set(Some(QadProbeReport {
                        relay: endpoint.clone(),
                        addr: val,
                        latency,
                    }))
                    .ok();
            }
        }
    }));
    let handle = AbortOnDropHandle::new(handle);

    Ok((
        report,
        QadConn {
            conn,
            observer,
            _handle: handle,
        },
    ))
}

#[cfg(not(wasm_browser))]
async fn run_probe_v6(
    relay: Arc<RelayConfig>,
    quic_client: QuicClient,
    dns_resolver: DnsResolver,
    shutdown_token: CancellationToken,
) -> n0_error::Result<(QadProbeReport, QadConn), QadProbeError> {
    use noq_proto::PathId;

    let relay_addr = reportgen::get_relay_addr_ipv6(&dns_resolver, &relay)
        .await
        .map_err(|source| e!(QadProbeError::GetRelayAddr { source }))?;

    trace!(?relay_addr, "resolved relay server address");
    let host = relay
        .url
        .host_str()
        .ok_or_else(|| e!(QadProbeError::MissingHost))?;
    let conn = quic_client
        .create_conn(relay_addr.into(), host)
        .await
        .map_err(|source| e!(QadProbeError::Quic { source }))?;

    let mut watcher = conn.observed_external_addr();

    // wait for an addr
    let addr = watcher
        .next()
        .await
        .ok_or_else(|| e!(QadProbeError::ReceiverDropped))?;
    let report = QadProbeReport {
        relay: relay.url.clone(),
        addr: SocketAddr::new(addr.ip().to_canonical(), addr.port()),
        latency: conn.rtt(PathId::ZERO).unwrap_or_default(),
    };

    let observer = Watchable::new(None);
    let endpoint = relay.url.clone();
    let handle = task::spawn(shutdown_token.run_until_cancelled_owned({
        let observer = observer.clone();
        let conn = conn.clone();
        async move {
            while let Some(val) = watcher.next().await {
                // if we've sent to an ipv4 address, but received an observed address
                // that is ivp6 then the address is an [IPv4-Mapped IPv6 Addresses](https://doc.rust-lang.org/beta/std/net/struct.Ipv6Addr.html#ipv4-mapped-ipv6-addresses)
                let val = SocketAddr::new(val.ip().to_canonical(), val.port());
                let latency = conn.rtt(PathId::ZERO).unwrap_or_default();
                observer
                    .set(Some(QadProbeReport {
                        relay: endpoint.clone(),
                        addr: val,
                        latency,
                    }))
                    .ok();
            }
        }
    }));
    let handle = AbortOnDropHandle::new(handle);

    Ok((
        report,
        QadConn {
            conn,
            observer,
            _handle: handle,
        },
    ))
}

#[cfg(test)]
mod test_utils {
    //! Creates a relay server against which to perform tests

    use iroh_relay::{RelayConfig, RelayQuicConfig, server};

    pub(crate) async fn relay() -> (server::Server, RelayConfig) {
        let server = server::Server::spawn(server::testing::server_config())
            .await
            .expect("should serve relay");
        let quic = Some(RelayQuicConfig::new(
            server.quic_addr().expect("server should run quic").port(),
        ));
        let endpoint_desc =
            RelayConfig::new(server.https_url().expect("should work as relay"), quic);

        (server, endpoint_desc)
    }

    /// Create a [`crate::RelayMap`] of the given size.
    ///
    /// This function uses [`relay`]. Note that the returned map uses internal order that will
    /// often _not_ match the order of the servers.
    pub(crate) async fn relay_map(relays: usize) -> (Vec<server::Server>, crate::RelayMap) {
        let mut servers = Vec::with_capacity(relays);
        let mut endpoints = Vec::with_capacity(relays);
        for _ in 0..relays {
            let (relay_server, endpoint) = relay().await;
            servers.push(relay_server);
            endpoints.push(endpoint);
        }
        (servers, crate::RelayMap::from_iter(endpoints))
    }
}

#[cfg(all(test, with_crypto_provider))]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};

    use iroh_base::RelayUrl;
    use iroh_dns::dns::DnsResolver;
    use iroh_relay::tls::{CaTlsConfig, default_provider};
    use n0_error::{Result, StdResultExt};
    use n0_tracing_test::traced_test;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::net_report::probes::Probe;

    #[tokio::test(flavor = "multi_thread")]
    #[traced_test]
    async fn test_basic() -> Result<()> {
        let (server, relay) = test_utils::relay().await;
        let client_config = iroh_relay::tls::make_dangerous_client_config();
        let ep = noq::Endpoint::client(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0)).anyerr()?;
        let quic_addr_disc = QuicConfig {
            ep: ep.clone(),
            client_config,
            ipv4: true,
            ipv6: true,
        };
        let relay_map = RelayMap::from(relay);

        let resolver = DnsResolver::new();
        let tls_config = CaTlsConfig::insecure_skip_verify()
            .client_config(default_provider())
            .expect("infallible");
        let opts = Options::new(tls_config).quic_config(Some(quic_addr_disc.clone()));
        let mut client = Client::new(
            resolver.clone(),
            relay_map.clone(),
            opts.clone(),
            Default::default(),
        );
        let if_state = IfStateDetails::fake();

        // Note that the ProbePlan will change with each iteration.
        for i in 0..5 {
            let cancel = CancellationToken::new();
            println!("--round {i}");
            let r = client
                .get_report(if_state.clone(), false, cancel.child_token())
                .await;

            assert!(r.has_udp(), "want UDP");
            dbg!(&r);
            assert!(
                !r.relay_latency.is_empty(),
                "expected at least 1 key in RelayLatency; got none",
            );
            assert!(
                r.relay_latency.iter().next().is_some(),
                "expected key 1 in RelayLatency; got {:?}",
                r.relay_latency
            );
            assert!(r.global_v4.is_some(), "expected globalV4 set");
            assert!(r.preferred_relay.is_some(),);
            cancel.cancel();
        }

        drop(client);
        ep.wait_idle().await;
        server.shutdown().await?;

        Ok(())
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn test_add_report_history_set_preferred_relay() -> Result {
        fn relay_url(i: u16) -> RelayUrl {
            format!("http://{i}.com").parse().unwrap()
        }

        // report returns a *Report from (relay host, Duration)+ pairs.
        fn report(a: impl IntoIterator<Item = (&'static str, u64)>) -> Option<Report> {
            let mut report = Report::default();
            for (s, d) in a {
                assert!(s.starts_with('d'), "invalid relay server key");
                let id: u16 = s[1..].parse().unwrap();
                report.relay_latency.update_relay(
                    relay_url(id),
                    Duration::from_secs(d),
                    Probe::QadIpv4,
                );
            }

            Some(report)
        }
        struct Step {
            /// Delay in seconds
            after: u64,
            r: Option<Report>,
        }
        struct Test {
            name: &'static str,
            steps: Vec<Step>,
            /// want PreferredRelay on final step
            want_relay: Option<RelayUrl>,
            // wanted len(c.prev)
            want_prev_len: usize,
        }

        let tests = [
            Test {
                name: "first_reading",
                steps: vec![Step {
                    after: 0,
                    r: report([("d1", 2), ("d2", 3)]),
                }],
                want_prev_len: 1,
                want_relay: Some(relay_url(1)),
            },
            Test {
                name: "with_two",
                steps: vec![
                    Step {
                        after: 0,
                        r: report([("d1", 2), ("d2", 3)]),
                    },
                    Step {
                        after: 1,
                        r: report([("d1", 4), ("d2", 3)]),
                    },
                ],
                want_prev_len: 2,
                want_relay: Some(relay_url(1)), // t0's d1 of 2 is still best
            },
            Test {
                name: "but_now_d1_gone",
                steps: vec![
                    Step {
                        after: 0,
                        r: report([("d1", 2), ("d2", 3)]),
                    },
                    Step {
                        after: 1,
                        r: report([("d1", 4), ("d2", 3)]),
                    },
                    Step {
                        after: 2,
                        r: report([("d2", 3)]),
                    },
                ],
                want_prev_len: 3,
                want_relay: Some(relay_url(2)), // only option
            },
            Test {
                name: "d1_is_back",
                steps: vec![
                    Step {
                        after: 0,
                        r: report([("d1", 2), ("d2", 3)]),
                    },
                    Step {
                        after: 1,
                        r: report([("d1", 4), ("d2", 3)]),
                    },
                    Step {
                        after: 2,
                        r: report([("d2", 3)]),
                    },
                    Step {
                        after: 3,
                        r: report([("d1", 4), ("d2", 3)]),
                    }, // same as 2 seconds ago
                ],
                want_prev_len: 4,
                want_relay: Some(relay_url(1)), // t0's d1 of 2 is still best
            },
            Test {
                name: "things_clean_up",
                steps: vec![
                    Step {
                        after: 0,
                        r: report([("d1", 1), ("d2", 2)]),
                    },
                    Step {
                        after: 1,
                        r: report([("d1", 1), ("d2", 2)]),
                    },
                    Step {
                        after: 2,
                        r: report([("d1", 1), ("d2", 2)]),
                    },
                    Step {
                        after: 3,
                        r: report([("d1", 1), ("d2", 2)]),
                    },
                    Step {
                        after: 10 * 60,
                        r: report([("d3", 3)]),
                    },
                ],
                want_prev_len: 1, // t=[0123]s all gone. (too old, older than 10 min)
                want_relay: Some(relay_url(3)), // only option
            },
            Test {
                name: "preferred_relay_hysteresis_no_switch",
                steps: vec![
                    Step {
                        after: 0,
                        r: report([("d1", 4), ("d2", 5)]),
                    },
                    Step {
                        after: 1,
                        r: report([("d1", 4), ("d2", 3)]),
                    },
                ],
                want_prev_len: 2,
                want_relay: Some(relay_url(1)), // 2 didn't get fast enough
            },
            Test {
                name: "preferred_relay_hysteresis_do_switch",
                steps: vec![
                    Step {
                        after: 0,
                        r: report([("d1", 4), ("d2", 5)]),
                    },
                    Step {
                        after: 1,
                        r: report([("d1", 4), ("d2", 1)]),
                    },
                ],
                want_prev_len: 2,
                want_relay: Some(relay_url(2)), // 2 got fast enough
            },
        ];
        let resolver = DnsResolver::new();
        let tls_config = CaTlsConfig::insecure_skip_verify()
            .client_config(default_provider())
            .expect("infallible");
        for mut tt in tests {
            println!("test: {}", tt.name);
            // Zork drops latencies of relays outside the map, so list the test relays.
            let relay_map: RelayMap = (1..=3).map(relay_url).collect();
            let opts = Options::new(tls_config.clone());
            let mut client = Client::new(resolver.clone(), relay_map, opts, Default::default());
            for s in &mut tt.steps {
                // trigger the timer
                tokio::time::advance(Duration::from_secs(s.after)).await;
                client.add_report_history_and_set_preferred_relay(s.r.as_mut().unwrap());
            }
            let last_report = tt.steps.last().unwrap().r.clone().unwrap();
            let got = client.reports.prev.len();
            let want = tt.want_prev_len;
            assert_eq!(got, want, "prev length");
            let got = &last_report.preferred_relay;
            let want = &tt.want_relay;
            assert_eq!(got, want, "preferred_relay");
        }

        Ok(())
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn test_fallback_relays_only_when_no_primary_answers() -> Result {
        fn relay_url(i: u16) -> RelayUrl {
            format!("http://{i}.com").parse().unwrap()
        }
        fn report(latencies: &[(u16, u64)]) -> Report {
            let mut report = Report::default();
            for (id, ms) in latencies {
                report.relay_latency.update_relay(
                    relay_url(*id),
                    Duration::from_millis(*ms),
                    Probe::QadIpv4,
                );
            }
            report
        }
        fn step(client: &mut Client, latencies: &[(u16, u64)]) -> Option<RelayUrl> {
            let mut r = report(latencies);
            client.add_report_history_and_set_preferred_relay(&mut r);
            r.preferred_relay
        }

        let tls_config = CaTlsConfig::insecure_skip_verify()
            .client_config(default_provider())
            .expect("infallible");
        // Relay 1 is primary; relays 2 and 3 are listed as fallbacks.
        let new_client = |fallbacks: Vec<RelayUrl>| {
            let mut opts = Options::new(tls_config.clone());
            opts.user_config.fallback_relays = fallbacks;
            let relay_map: RelayMap = (1..=3).map(relay_url).collect();
            Client::new(DnsResolver::new(), relay_map, opts, Default::default())
        };
        let fallbacks = || vec![relay_url(2), relay_url(3)];

        // A faster fallback does not win while the primary answers.
        let mut client = new_client(fallbacks());
        assert_eq!(step(&mut client, &[(1, 300), (2, 40)]), Some(relay_url(1)));
        tokio::time::advance(Duration::from_secs(30)).await;
        assert_eq!(step(&mut client, &[(1, 300), (2, 10)]), Some(relay_url(1)));

        // Primary silent: the fallback with the best recent latency takes over
        // (relay 2 measured 10ms in the previous report).
        tokio::time::advance(Duration::from_secs(30)).await;
        assert_eq!(step(&mut client, &[(2, 60), (3, 50)]), Some(relay_url(2)));

        // Primary answers again, even much slower: taken back in the same
        // report instead of the hysteresis keeping the fallback.
        tokio::time::advance(Duration::from_secs(30)).await;
        assert_eq!(
            step(&mut client, &[(1, 900), (2, 60), (3, 50)]),
            Some(relay_url(1))
        );

        // Only fallbacks answer from the first report.
        let mut client = new_client(fallbacks());
        assert_eq!(step(&mut client, &[(2, 80), (3, 90)]), Some(relay_url(2)));

        // Without fallbacks, latency alone decides (upstream behaviour).
        let mut client = new_client(Vec::new());
        assert_eq!(step(&mut client, &[(1, 300), (2, 40)]), Some(relay_url(2)));

        Ok(())
    }

    fn zork_relay_url(i: u16) -> RelayUrl {
        format!("http://{i}.com").parse().unwrap()
    }

    fn zork_report(latencies: &[(u16, u64)]) -> Report {
        let mut report = Report::default();
        for (id, ms) in latencies {
            report.relay_latency.update_relay(
                zork_relay_url(*id),
                Duration::from_millis(*ms),
                Probe::Https,
            );
        }
        report
    }

    /// Relay 1 is primary; `fallbacks` of relays 2 and 3 are fallbacks.
    fn zork_client(fallbacks: &[u16]) -> Client {
        let tls_config = CaTlsConfig::insecure_skip_verify()
            .client_config(default_provider())
            .expect("infallible");
        let mut opts = Options::new(tls_config);
        opts.user_config.fallback_relays = fallbacks.iter().map(|i| zork_relay_url(*i)).collect();
        let relay_map: RelayMap = (1..=3).map(zork_relay_url).collect();
        Client::new(DnsResolver::new(), relay_map, opts, Default::default())
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn test_relay_refusing_admission_is_not_alive_for_home() {
        fn step(client: &mut Client, latencies: &[(u16, u64)]) -> Option<RelayUrl> {
            let mut r = zork_report(latencies);
            client.add_report_history_and_set_preferred_relay(&mut r);
            r.preferred_relay
        }
        let mut client = zork_client(&[2, 3]);
        let admission = client.admission.clone();
        assert_eq!(step(&mut client, &[(1, 100), (2, 300)]), Some(zork_relay_url(1)));

        // The primary answers its latency probe but refuses the relay connection:
        // the fallback becomes home.
        admission.set_refused(&zork_relay_url(1), true);
        tokio::time::advance(Duration::from_secs(30)).await;
        assert_eq!(step(&mut client, &[(1, 100), (2, 300), (3, 500)]), Some(zork_relay_url(2)));
        tokio::time::advance(Duration::from_secs(30)).await;
        assert_eq!(step(&mut client, &[(1, 100), (2, 300), (3, 500)]), Some(zork_relay_url(2)));

        // Admitted again: the primary is taken back at once.
        admission.set_refused(&zork_relay_url(1), false);
        tokio::time::advance(Duration::from_secs(30)).await;
        assert_eq!(step(&mut client, &[(1, 100), (2, 300), (3, 500)]), Some(zork_relay_url(1)));

        // If every answering relay refuses, the best one is still chosen.
        admission.set_refused(&zork_relay_url(1), true);
        admission.set_refused(&zork_relay_url(2), true);
        tokio::time::advance(Duration::from_secs(30)).await;
        assert_eq!(step(&mut client, &[(1, 100), (2, 300)]), Some(zork_relay_url(1)));

        // Without fallbacks a refusing relay is skipped in favour of a slower one.
        let mut client = zork_client(&[]);
        client.admission.set_refused(&zork_relay_url(1), true);
        assert_eq!(step(&mut client, &[(1, 100), (2, 300)]), Some(zork_relay_url(2)));
    }

    #[tokio::test]
    async fn test_report_is_complete_once_admitted_primaries_answered() {
        let if_state = IfStateDetails { have_v4: true, have_v6: false };
        let refused = BTreeSet::new();
        let primary_only = zork_report(&[(1, 100)]);

        let client = zork_client(&[2, 3]);
        for do_full in [true, false] {
            assert!(client.have_enough_reports(&if_state, do_full, 3, &refused, &primary_only));
            assert!(!client.have_enough_reports(&if_state, do_full, 3, &refused, &zork_report(&[(2, 50)])));
        }
        assert_eq!(client.held_fallbacks(&refused), BTreeSet::from([zork_relay_url(2), zork_relay_url(3)]));

        // A refusing primary does not complete the report, and fallbacks are
        // probed without delay because they are needed.
        let refused = BTreeSet::from([zork_relay_url(1)]);
        assert!(!client.have_enough_reports(&if_state, true, 3, &refused, &primary_only));
        assert!(client.held_fallbacks(&refused).is_empty());

        // Without fallbacks every relay must answer (upstream behaviour).
        let client = zork_client(&[]);
        let refused = BTreeSet::new();
        assert!(!client.have_enough_reports(&if_state, true, 3, &refused, &primary_only));
        assert!(client.held_fallbacks(&refused).is_empty());
    }

    /// Behind a fake-IP proxy no QAD probe runs, so HTTPS probes decide. A
    /// fallback that never answers must not hold the report back once the primary
    /// answered (it used to run into the 5s reportgen timeout).
    #[tokio::test(flavor = "multi_thread")]
    #[traced_test]
    async fn test_report_does_not_wait_for_fallbacks() -> Result {
        let (server, primary) = test_utils::relay().await;
        // Accepts TCP but never answers: a probe to it can only time out.
        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.anyerr()?;
        let silent_url: RelayUrl = format!("http://{}", silent.local_addr().anyerr()?)
            .parse()
            .anyerr()?;
        let _silent = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = silent.accept().await {
                held.push(stream);
            }
        });
        let relay_map = RelayMap::from_iter([
            primary.clone(),
            iroh_relay::RelayConfig::from(silent_url.clone()),
        ]);
        let tls_config = CaTlsConfig::insecure_skip_verify()
            .client_config(default_provider())
            .expect("infallible");
        let mut opts = Options::new(tls_config);
        opts.user_config.fallback_relays = vec![silent_url.clone()];
        opts.user_config.captive_portal_check = false;
        let mut client = Client::new(DnsResolver::new(), relay_map, opts, Default::default());

        let start = Instant::now();
        let report = client
            .get_report(IfStateDetails::fake(), true, CancellationToken::new())
            .await;
        let took = start.elapsed();
        assert_eq!(report.preferred_relay, Some(primary.url.clone()));
        assert!(took < Duration::from_secs(1), "report took {took:?}");
        assert!(
            report.relay_latency.iter().all(|(_, url, _)| url != &silent_url),
            "held fallback was probed: {report:?}"
        );

        drop(client);
        server.shutdown().await?;
        Ok(())
    }
}
