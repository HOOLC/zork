//! The standing attach task: discover, dial out, prove, answer.
//!
//! One connection per membership domain, each with its own retry clock. The
//! tunnel is long-lived but expendable — on any failure the task starts over
//! from discovery, because the endpoint may have moved and the nonce certainly
//! has.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};
use synch_core::{EntryKind, Hash, OriginId};
use synch_store::{VersionPolicy, VersionSet};
use tokio::sync::{mpsc, Semaphore};
use tokio_tungstenite::tungstenite::Message;

use crate::{
    cloud::frame::{
        attach_signing_input, encode_chunk, settles_at, DelegationJson, Down, EntryJson,
        ReplicaSpaceJson, Up, VersionJson, MAX_CHUNK, MIN_PROTOCOL_VERSION, NONCE_LEN,
        PROTOCOL_VERSION, SPACE_UPDATES_VERSION,
    },
    error::{EngineError, Result},
    node::Node,
};

/// The path the attach connection is made against, appended to the URL the
/// zone published. Fixed by the protocol version, so the record names an
/// origin and never a path a compromised zone could point at something else.
const ATTACH_PATH: &str = "/agent/v1/attach";

/// The environment override that replaces discovery.
///
/// A test hook in the mould of `CP_REKOR_URL`, not a configuration knob:
/// production learns its endpoint from the zone it already validates, and
/// there is deliberately no `--url` for an operator to be talked into.
const URL_ENV: &str = "SYNCH_CLOUD_URL";

/// How often the supervisor re-reads the settings and the domain list.
const SUPERVISE_INTERVAL: Duration = Duration::from_secs(5);

/// The first reconnect delay, doubled per failure up to [`MAX_BACKOFF`].
const MIN_BACKOFF: Duration = Duration::from_secs(2);

/// The longest a disconnected node waits before trying again.
const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// The floor and ceiling on how often the endpoint list is re-read.
///
/// The record's TTL sets the cadence — it is the zone's own statement of how
/// long the answer stands — and these bound what the zone may ask for. The
/// floor keeps a one-second TTL from turning a fleet's DNS into a poll loop;
/// the ceiling keeps a day-long one from leaving a decommissioned replica
/// attached until the daemon restarts.
const MIN_REDISCOVERY: Duration = Duration::from_secs(60);
const MAX_REDISCOVERY: Duration = Duration::from_secs(3600);

/// How often each side sends a heartbeat.
const HEARTBEAT: Duration = Duration::from_secs(30);

/// How many heartbeats may go unanswered before the session is dead.
const HEARTBEAT_MISSES: u32 = 2;

/// How many frames may wait for the socket writer.
///
/// Small on purpose: content is credit-governed and everything else is one
/// frame, so a deep queue would only hide a stalled socket.
const WRITE_AHEAD: usize = 8;

/// How long one write to the socket may take before the session is torn down.
///
/// A control plane that stops reading (a zero-window stall, or a hostile peer
/// that never drains) blocks the writer; without this bound the write would
/// hang forever. Set above the heartbeat window so a merely slow-but-live link
/// is not killed, but finite so a wedged one is.
const WRITE_TIMEOUT: Duration = Duration::from_secs(60);

/// The most requests and streams one session may have in flight at once.
///
/// A hostile control plane can otherwise open unbounded LS/STAT/RESOLVE tasks
/// and READ streams, each doing blocking-pool store work. Over the cap a
/// request is refused with a coded error rather than queued. The control
/// plane's per-user cap does not protect the daemon — that is a different
/// process trusting a different thing — so the daemon holds its own ceiling.
const MAX_INFLIGHT: usize = 64;

/// How long discovery, the dial, and the three handshake frames get in total.
///
/// The heartbeat only starts once the session is serving, so without this a
/// control plane that accepts the connection and then says nothing holds the
/// attach task — and its domain's only slot — forever.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// The largest frame and message this daemon will read from the control plane.
///
/// Every `Down` frame is small control JSON; content flows the other way. The
/// tungstenite defaults (64 MiB message, 16 MiB frame) are sized for general
/// use and would let the peer make the daemon buffer far more than the
/// protocol ever needs.
const MAX_DOWN_FRAME: usize = 1 << 20;

/// The most unspent chunk credits one stream may hold.
///
/// Credit is a promise to read, not a buffer — the writer channel is what
/// bounds memory — so the cap exists to keep a peer from adding permits until
/// the semaphore's own ceiling panics the daemon.
const MAX_CREDIT: usize = 4096;

/// How many entries one listing page carries.
const PAGE_LIMIT: usize = 500;

/// How many flat rows one listing pass pulls from the store at a time.
const SCAN_BATCH: usize = 500;

impl Node {
    /// Runs cloud attach until the daemon stops.
    ///
    /// The loop supervises rather than connects: one child task per membership
    /// domain the node holds, stopped when the operator opts the feature out
    /// or the domain goes away. For an opted-out node it costs one settings
    /// read per interval and opens nothing at all.
    pub async fn run_cloud(
        &self,
        resolver: Option<Arc<synch_net::DnssecResolver>>,
        shutdown: impl std::future::Future<Output = ()>,
    ) {
        let shutdown = std::pin::pin!(shutdown);
        let mut shutdown = shutdown;
        let mut running: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
        loop {
            let wanted = self.attach_targets().await;
            running.retain(|domain, task| {
                let keep = wanted.iter().any(|d| d == domain) && !task.is_finished();
                if !keep {
                    task.abort();
                    self.forget_cloud_status(domain);
                }
                keep
            });
            for domain in wanted {
                if running.contains_key(&domain) {
                    continue;
                }
                let node = self.clone();
                let resolver = resolver.clone();
                let name = domain.clone();
                running.insert(
                    domain,
                    tokio::spawn(async move { attach_forever(node, resolver, name).await }),
                );
            }
            tokio::select! {
                _ = &mut shutdown => break,
                _ = tokio::time::sleep(SUPERVISE_INTERVAL) => {}
            }
        }
        for (_, task) in running {
            task.abort();
        }
    }

    /// The membership domains a tunnel should be open for right now.
    ///
    /// The zone that names this node, unless the operator has opted the feature
    /// out — which is the only thing that empties the list, there being no
    /// enablement to require. Empty for a key-identified node: there is no apex
    /// to take a control plane from, so there is nothing to discover and
    /// nothing to attach to.
    ///
    /// The settings read goes over to the blocking pool: this runs on the
    /// supervisor's runtime worker once per [`SUPERVISE_INTERVAL`], and a
    /// `config` read waits on the same connection mutex a publish batch or a GC
    /// pass holds (§10). The zone itself is the origin's, held in memory.
    async fn attach_targets(&self) -> Vec<String> {
        let node = self.clone();
        crate::blocking::offload(move || {
            Ok(match node.cloud_settings() {
                Ok(settings) if !settings.disabled => node.resolving_domains(),
                _ => Vec::new(),
            })
        })
        .await
        .unwrap_or_default()
    }
}

/// Keeps one domain's tunnels up: one per endpoint the apex names, each with
/// a retry clock of its own.
///
/// **A tunnel per endpoint, not per domain.** A control plane is a fleet, and
/// the registry of attached daemons is one node's memory — a node this daemon
/// holds no tunnel to can answer no browse question about it, however
/// faithfully the node's database replicated. So discovery yields a list and
/// every entry gets a child; one replica being down costs its own tunnel and
/// nothing else's, which is the whole point of there being several.
///
/// Discovery runs here rather than in the children so the answer is read once
/// per round instead of once per endpoint, and is re-read on a clock: the set
/// is zone data and a fleet gains and loses nodes without this process
/// restarting. A round that cannot resolve keeps the children it has — an
/// endpoint that was validated stays believed for as long as the last answer
/// says, and a resolver outage must not tear down working tunnels.
async fn attach_forever(
    node: Node,
    resolver: Option<Arc<synch_net::DnssecResolver>>,
    domain: String,
) {
    // `AbortOnDrop`, not a bare handle, because this task is itself stopped by
    // being aborted — `run_cloud` does exactly that when the operator opts out
    // or the domain goes away. An abort drops this future and everything it
    // owns; a bare `JoinHandle` dropped is a task that keeps running, so the
    // tunnels would outlive the supervisor that was told to stop them and go
    // on serving browse requests for a node that has opted out.
    let mut running: HashMap<String, AbortOnDrop> = HashMap::new();
    let mut backoff = MIN_BACKOFF;
    loop {
        let wait = match discover(resolver.as_deref(), &domain).await {
            Ok((endpoints, ttl)) => {
                backoff = MIN_BACKOFF;
                // The row a failed round left behind. Discovery works now, so
                // "no validated record" is no longer true of this domain, and
                // nothing else would ever remove it — `forget_cloud_endpoint`
                // only reaches rows that name an endpoint.
                node.forget_cloud_endpoint_none(&domain);
                running.retain(|url, task| {
                    let keep = endpoints.iter().any(|e| e == url) && !task.0.is_finished();
                    if !keep {
                        node.forget_cloud_endpoint(&domain, url);
                    }
                    keep
                });
                for url in endpoints {
                    if running.contains_key(&url) {
                        continue;
                    }
                    let node = node.clone();
                    let domain = domain.clone();
                    let endpoint = url.clone();
                    running.insert(
                        url,
                        AbortOnDrop(tokio::spawn(async move {
                            attach_endpoint_forever(node, domain, endpoint).await
                        })),
                    );
                }
                ttl.clamp(MIN_REDISCOVERY, MAX_REDISCOVERY)
            }
            Err(e) => {
                tracing::debug!(domain, error = %e, "control-plane discovery failed");
                // Only a domain with nothing running is worth reporting on:
                // with tunnels up, the endpoints' own rows are the truth and
                // a discovery hiccup is not news about any of them.
                if running.is_empty() {
                    node.set_cloud_status(&domain, None, false, Some(e.to_string()));
                }
                let wait = backoff;
                backoff = (backoff * 2).min(MAX_BACKOFF);
                wait
            }
        };
        tokio::time::sleep(crate::aae::jittered_floor(wait)).await;
    }
}

/// Keeps one endpoint's tunnel up, with exponential backoff and jitter.
async fn attach_endpoint_forever(node: Node, domain: String, endpoint: String) {
    let mut backoff = MIN_BACKOFF;
    loop {
        let started = Instant::now();
        match attach_once(&node, &domain, &endpoint).await {
            Ok(()) => tracing::info!(domain, endpoint, "cloud attach closed cleanly"),
            Err(e) => {
                tracing::debug!(domain, endpoint, error = %e, "cloud attach failed");
                node.set_cloud_status(&domain, Some(endpoint.clone()), false, Some(e.to_string()));
            }
        }
        // A session that stood for a while was healthy; only repeated fast
        // failures are worth backing off from.
        if started.elapsed() > MAX_BACKOFF {
            backoff = MIN_BACKOFF;
        }
        tokio::time::sleep(crate::aae::jittered_floor(backoff)).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Connects to one endpoint, proves, and serves one session to its end.
async fn attach_once(node: &Node, domain: &str, base: &str) -> Result<()> {
    let base = base.to_string();
    let url = format!("{base}{ATTACH_PATH}");
    node.set_cloud_status(domain, Some(base.clone()), false, None);

    // The whole handshake under one clock: the heartbeat that would otherwise
    // notice a silent peer does not start until the session is serving.
    let handshake = async {
        let mut config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default();
        config.max_message_size = Some(MAX_DOWN_FRAME);
        config.max_frame_size = Some(MAX_DOWN_FRAME);
        // Verify the control plane's certificate against the host's trust
        // store, not the roots tungstenite would otherwise compile in: an
        // operator running the control plane behind a private or enterprise CA
        // installs a root, and this dials it (`synch_net::tls`).
        let connector = tokio_tungstenite::Connector::Rustls(synch_net::tls::client_config()?);
        let (socket, _) = tokio_tungstenite::connect_async_tls_with_config(
            websocket_url(&url),
            Some(config),
            false,
            Some(connector),
        )
        .await
        .map_err(|e| EngineError::invalid(format!("{url}: {e}")))?;
        let (mut sink, mut stream) = socket.split();

        // Two store reads, so they go over together (§10).
        let held = current_held_spaces(node).await?;
        send(
            &mut sink,
            &Up::Hello {
                v: PROTOCOL_VERSION,
                network: domain.to_string(),
                origin: node.origin().canonical(),
                device: node.node_id().to_z32(),
                spaces: held.clone(),
            },
        )
        .await?;

        let nonce = match receive(&mut stream).await? {
            Down::Challenge { nonce } => decode_nonce(&nonce)?,
            Down::Err { code, message, .. } => {
                return Err(EngineError::invalid(brief(&format!("{code}: {message}"))))
            }
            other => {
                return Err(EngineError::invalid(brief(&format!(
                    "expected a challenge, got {other:?}"
                ))))
            }
        };
        // Signed here, in the process that holds the key: no RPC exists that
        // would hand this capability to another program on the node.
        let signature = node.sign_attach(&url, &nonce);
        send(
            &mut sink,
            &Up::Proof {
                sig: hex::encode(signature.to_bytes()),
                key: node.node_id().to_z32(),
            },
        )
        .await?;

        let settled = match receive(&mut stream).await? {
            // A range rather than equality. The control plane settles on this
            // daemon's version when it can, so the ordinary echo is ours; a
            // lower one means an older control plane, and serving under it
            // costs this end nothing but questions it will not be asked.
            Down::Attached { session, v } if settles_at(v) => {
                if v != PROTOCOL_VERSION {
                    tracing::info!(
                        domain,
                        session,
                        url,
                        settled = v,
                        speaks = PROTOCOL_VERSION,
                        "cloud attach established on an older tunnel version"
                    );
                } else {
                    tracing::info!(domain, session, url, "cloud attach established");
                }
                v
            }
            Down::Attached { v, .. } => {
                return Err(EngineError::invalid(format!(
                    "tunnel protocol mismatch: this daemon speaks v{MIN_PROTOCOL_VERSION} to \
                     v{PROTOCOL_VERSION}, the control plane settled on v{v}"
                )))
            }
            Down::Err { code, message, .. } => {
                return Err(EngineError::invalid(brief(&format!("{code}: {message}"))))
            }
            other => {
                return Err(EngineError::invalid(brief(&format!(
                    "expected an attach, got {other:?}"
                ))))
            }
        };
        Ok((sink, stream, held, settled))
    };
    let (sink, stream, held, settled) = tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake)
        .await
        .map_err(|_| {
        EngineError::invalid(format!("{url}: the handshake did not finish in time"))
    })??;
    node.set_cloud_status(domain, Some(base.clone()), true, None);

    let outcome = serve(node, sink, stream, held, settled).await;
    node.set_cloud_status(
        domain,
        Some(base),
        false,
        outcome.as_ref().err().map(|e| e.to_string()),
    );
    outcome
}

/// The spaces this node currently claims to hold.
///
/// A source or replica is explicit local participation, so either makes the
/// namespace routable. The session refreshes this claim when it changes; the
/// control plane may ask for anything, and this says where it will find it.
fn held_spaces(node: &Node) -> Result<Vec<String>> {
    let mut held: Vec<String> = node
        .store()
        .sources()?
        .into_iter()
        .map(|source| source.space)
        .collect();
    held.extend(
        node.store()
            .replicas()?
            .into_iter()
            .map(|replica| replica.space),
    );
    held.sort_unstable();
    held.dedup();
    Ok(held)
}

/// Reads the routing claim away from an async runtime worker.
async fn current_held_spaces(node: &Node) -> Result<Vec<String>> {
    let node = node.clone();
    crate::blocking::offload(move || held_spaces(&node)).await
}

/// Publishes a replacement routing claim when local participation changed.
///
/// A full writer queue leaves `advertised` untouched so the next heartbeat
/// retries. Against a pre-v4 control plane the only compatible replacement is
/// a fresh hello, so returning an error deliberately drives the reconnect loop.
fn refresh_space_claim(
    writes: &Writer,
    advertised: &mut Vec<String>,
    settled_version: u32,
    spaces: Vec<String>,
) -> Result<()> {
    if spaces == *advertised {
        return Ok(());
    }
    if settled_version < SPACE_UPDATES_VERSION {
        return Err(EngineError::invalid(
            "the routing claim changed; reconnecting to refresh an older control plane",
        ));
    }
    if writes
        .try_send(text(&Up::Spaces {
            spaces: spaces.clone(),
        })?)
        .is_ok()
    {
        *advertised = spaces;
    }
    Ok(())
}

impl Node {
    /// Signs an attach challenge with the active device key.
    ///
    /// The context is domain-separated from every other signature this key
    /// makes: an attach proof is not a head signature and cannot be made to
    /// verify as one.
    pub(crate) fn sign_attach(&self, url: &str, nonce: &[u8]) -> iroh_base::Signature {
        self.secret().sign(&attach_signing_input(url, nonce))
    }

    /// Signs a *write-tunnel* attach challenge with the active device key
    /// (`docs/CLOUD-WRITES.md` §5.2).
    ///
    /// Public, because the write tunnel is opened by an embedder — the cloud
    /// data plane — and the secret never leaves the engine. What is exposed is
    /// one signature under one fixed tag, never the key: a caller cannot make
    /// this sign a head, a browse proof, or anything of its own choosing.
    pub fn sign_write_attach(&self, url: &str, nonce: &[u8]) -> iroh_base::Signature {
        self.secret()
            .sign(&crate::cloud::frame::write_attach_signing_input(url, nonce))
    }
}

/// The endpoints this domain's base attaches to, and how long the answer
/// stands.
///
/// From the zone or from nowhere: no validated record means no connection
/// attempt at all, which is the shape a resolver outage has to degrade to.
/// Several, because the apex names every node of its control plane and a
/// tunnel reaches exactly one of them.
async fn discover(
    resolver: Option<&synch_net::DnssecResolver>,
    domain: &str,
) -> Result<(Vec<String>, Duration)> {
    if let Ok(configured) = std::env::var(URL_ENV) {
        // No TTL to take from an environment variable, and nothing that would
        // change it while the process runs: re-read on the slow clock.
        return Ok((overridden_endpoints(&configured)?, MAX_REDISCOVERY));
    }
    let resolver = resolver.ok_or_else(|| {
        EngineError::invalid(
            "this daemon runs no DNSSEC resolver, so it cannot discover a control plane",
        )
    })?;
    let (records, ttl) = resolver
        .control_plane(domain)
        .await
        .map_err(|e| EngineError::invalid(format!("{domain}: {e}")))?;
    let urls: Vec<String> = records.into_iter().map(|record| record.url).collect();
    tracing::debug!(
        domain,
        endpoints = urls.len(),
        ttl = ttl.as_secs(),
        "discovered a control plane"
    );
    Ok((urls, ttl))
}

/// The endpoints [`URL_ENV`] names, comma-separated.
///
/// A list rather than one URL because a fleet is what the record carries, and
/// an override that could only name one endpoint would not stand up the case
/// the supervisor exists for. Trailing slashes go, because the daemon signs
/// its attach proof over the URL it dials and both ends have to derive the
/// same bytes from the same origin.
fn overridden_endpoints(configured: &str) -> Result<Vec<String>> {
    let urls: Vec<String> = configured
        .split(',')
        .map(|url| url.trim().trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty())
        .collect();
    if urls.is_empty() {
        return Err(EngineError::invalid(format!("{URL_ENV} is empty")));
    }
    Ok(urls)
}

/// The WebSocket scheme for an HTTP origin.
/// Renders text the control plane chose, for a log line or an error message.
///
/// The peer picks these strings. Control characters would steer a terminal
/// reading the daemon's log, and the length is bounded by the frame size
/// rather than by anything the protocol needs, so both are cut here.
fn brief(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(200).collect()
}

fn websocket_url(url: &str) -> String {
    match url.split_once("://") {
        Some(("https", rest)) => format!("wss://{rest}"),
        Some(("http", rest)) => format!("ws://{rest}"),
        _ => url.to_string(),
    }
}

fn decode_nonce(text: &str) -> Result<Vec<u8>> {
    let bytes = hex::decode(text)
        .map_err(|e| EngineError::invalid(format!("the challenge nonce is not hex: {e}")))?;
    if bytes.len() != NONCE_LEN {
        return Err(EngineError::invalid(format!(
            "the challenge nonce is {} bytes, not {NONCE_LEN}",
            bytes.len()
        )));
    }
    Ok(bytes)
}

/// One end of the socket, as the tasks that write to it see it.
type Writer = mpsc::Sender<Message>;

/// A stream in flight: its credit, and the task producing for it.
#[derive(Debug)]
struct Stream {
    credit: Arc<Semaphore>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        // Closing the semaphore is what a cancelled read notices: the producer
        // is blocked on a permit far more often than it is between chunks.
        self.credit.close();
        self.task.abort();
    }
}

/// A message from a spawned task back to the session loop.
///
/// This is what keeps the session's bookkeeping truthful: a one-shot request
/// or a stream that has ended tells the loop so, rather than leaving a map
/// entry or an in-flight count that no longer means anything.
#[derive(Debug)]
enum Internal {
    /// A READ stream ended — success, error, or cancellation — so its map
    /// entry can go. Without this the entry leaks per completed download and
    /// the reused-id guard refuses the id forever.
    ReadDone(u32),
    /// A one-shot LS/STAT/RESOLVE task finished, so the in-flight count drops.
    RequestDone,
    /// The latest source/replica routing claim, read away from this loop.
    SpaceClaim(Result<Vec<String>>),
    /// The writer task gave up on the socket — a failed or stalled write — so
    /// the session is over.
    WriterFailed(String),
}

/// Aborts the writer task when the session ends, whichever way it ends.
#[derive(Debug)]
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Answers frames until the connection ends.
///
/// The socket write lives in a dedicated task, not in the select loop: a write
/// blocked on backpressure from a control plane that has stopped reading would
/// otherwise stall every other branch — the heartbeat that is meant to kill a
/// dead session could not fire, and the whole task would hang holding the read
/// half, the streams, and their blob handles. With the writer split off, the
/// loop keeps polling the heartbeat and the read half no matter how wedged the
/// write direction is, and the writer's own timeout tears the session down.
async fn serve<S, R>(
    node: &Node,
    sink: S,
    mut stream: R,
    mut advertised_spaces: Vec<String>,
    settled_version: u32,
) -> Result<()>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + Unpin
        + Send
        + 'static,
    R: futures_util::Stream<
            Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>,
        > + Unpin,
{
    let (writes, mut outgoing) = mpsc::channel::<Message>(WRITE_AHEAD);
    let (internal, mut events) = mpsc::channel::<Internal>(WRITE_AHEAD);

    // The writer owns the sink. Each write is bounded by `WRITE_TIMEOUT`, so a
    // peer that stops reading is a session that ends rather than a task that
    // hangs. When the loop returns, `writes` drops, `outgoing.recv()` yields
    // `None`, and the writer exits — the `AbortOnDrop` is only the backstop for
    // a write still in progress at that moment.
    let writer = {
        let internal = internal.clone();
        let mut sink = sink;
        tokio::spawn(async move {
            while let Some(message) = outgoing.recv().await {
                match tokio::time::timeout(WRITE_TIMEOUT, sink.send(message)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        let _ = internal
                            .send(Internal::WriterFailed(format!(
                                "the tunnel write failed: {e}"
                            )))
                            .await;
                        return;
                    }
                    Err(_) => {
                        let _ = internal
                            .send(Internal::WriterFailed(
                                "the control plane stopped reading; the write stalled".into(),
                            ))
                            .await;
                        return;
                    }
                }
            }
        })
    };
    let _writer_guard = AbortOnDrop(writer);

    let mut streams: HashMap<u32, Stream> = HashMap::new();
    // One-shot LS/STAT/RESOLVE tasks in flight; streams are counted separately
    // by the map, and the cap is on the sum.
    let mut requests: usize = 0;
    // First tick one period out, not immediately: a `tokio` interval fires at
    // once otherwise, which would ping on connect and, worse, spend a miss
    // before any time had passed — making the two-miss window 30s, not 60s.
    let mut beat = tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
    beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut unanswered = 0u32;
    // At most one store read is outstanding for this session. Its guard drops
    // the async waiter when the session ends; an already-running blocking
    // closure may finish, but can no longer publish a claim into this loop.
    let mut space_refresh: Option<AbortOnDrop> = None;

    loop {
        tokio::select! {
            event = events.recv() => {
                match event {
                    Some(Internal::WriterFailed(why)) => return Err(EngineError::invalid(why)),
                    // On completion, however it completed: the entry goes and
                    // the id is free to be reused.
                    Some(Internal::ReadDone(id)) => { streams.remove(&id); }
                    Some(Internal::RequestDone) => requests = requests.saturating_sub(1),
                    Some(Internal::SpaceClaim(spaces)) => {
                        space_refresh = None;
                        refresh_space_claim(
                            &writes,
                            &mut advertised_spaces,
                            settled_version,
                            spaces?,
                        )?;
                    }
                    // The loop holds a sender, so this only happens at shutdown.
                    None => return Ok(()),
                }
            }
            _ = beat.tick() => {
                if unanswered >= HEARTBEAT_MISSES {
                    return Err(EngineError::invalid(format!(
                        "the control plane missed {unanswered} heartbeats; the session is dead"
                    )));
                }

                if space_refresh.is_none() {
                    let node = node.clone();
                    let internal = internal.clone();
                    space_refresh = Some(AbortOnDrop(tokio::spawn(async move {
                        let spaces = current_held_spaces(&node).await;
                        let _ = internal.send(Internal::SpaceClaim(spaces)).await;
                    })));
                }
                unanswered += 1;
                // `try_send`, never `await`: a full channel means the writer is
                // behind, which the write timeout already covers — the loop must
                // not block here, or it stops polling the very branches that
                // detect a dead peer.
                let _ = writes.try_send(text(&Up::Ping)?);
            }
            incoming = stream.next() => {
                let Some(incoming) = incoming else { return Ok(()) };
                let incoming = incoming
                    .map_err(|e| EngineError::invalid(format!("the tunnel read failed: {e}")))?;
                match incoming {
                    Message::Text(body) => {
                        unanswered = 0;
                        let frame: Down = serde_json::from_str(&body).map_err(|e| {
                            EngineError::invalid(format!("malformed tunnel frame: {e}"))
                        })?;
                        handle(node, &writes, &internal, &mut streams, &mut requests, frame)?;
                    }
                    // Content only ever travels upward, so a binary frame from
                    // the control plane is a protocol violation rather than a
                    // request to interpret.
                    Message::Binary(_) => {
                        return Err(EngineError::invalid(
                            "the control plane sent a content frame; nothing travels down this \
                             tunnel but control frames",
                        ))
                    }
                    Message::Close(_) => return Ok(()),
                    Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => unanswered = 0,
                }
            }
        }
    }
}

/// Serves one control frame.
///
/// Never awaits: every answer is produced in a spawned task and delivered
/// through the writer channel, so serving a frame cannot block the session
/// loop. A `try_send` that finds the channel full is treated as backpressure
/// on the offending request, not on the session.
fn handle(
    node: &Node,
    writes: &Writer,
    internal: &mpsc::Sender<Internal>,
    streams: &mut HashMap<u32, Stream>,
    requests: &mut usize,
    frame: Down,
) -> Result<()> {
    match frame {
        Down::Ping => {
            let _ = writes.try_send(text(&Up::Pong)?);
        }
        Down::Pong => {}
        Down::Err { id, code, message } => {
            let (code, message) = (brief(&code), brief(&message));
            tracing::debug!(?id, code, message, "the control plane refused a request");
            if let Some(id) = id {
                streams.remove(&id);
            }
        }
        Down::Credit { id, n } => {
            if let Some(stream) = streams.get(&id) {
                let room = MAX_CREDIT.saturating_sub(stream.credit.available_permits());
                stream.credit.add_permits((n as usize).min(room));
            }
        }
        Down::Cancel { id } => {
            streams.remove(&id);
        }
        Down::Ls {
            id,
            space,
            path,
            cursor,
            all,
        } => {
            if over_capacity(writes, streams, *requests, id) {
                return Ok(());
            }
            *requests += 1;
            let node = node.clone();
            let writes = writes.clone();
            let internal = internal.clone();
            tokio::spawn(async move {
                let page = list_page(&node, &space, &path, cursor, all)
                    .await
                    .map(|frame| with_id(frame, id));
                answer(&writes, id, page).await;
                let _ = internal.send(Internal::RequestDone).await;
            });
        }
        Down::Delegations { id } => {
            if over_capacity(writes, streams, *requests, id) {
                return Ok(());
            }
            *requests += 1;
            let node = node.clone();
            let writes = writes.clone();
            let internal = internal.clone();
            tokio::spawn(async move {
                answer(&writes, id, delegations(&node, id).await).await;
                let _ = internal.send(Internal::RequestDone).await;
            });
        }
        Down::Replication { id } => {
            if over_capacity(writes, streams, *requests, id) {
                return Ok(());
            }
            *requests += 1;
            let node = node.clone();
            let writes = writes.clone();
            let internal = internal.clone();
            tokio::spawn(async move {
                answer(&writes, id, replication(&node, id).await).await;
                let _ = internal.send(Internal::RequestDone).await;
            });
        }
        Down::Stat { id, space, path } => {
            if over_capacity(writes, streams, *requests, id) {
                return Ok(());
            }
            *requests += 1;
            let node = node.clone();
            let writes = writes.clone();
            let internal = internal.clone();
            tokio::spawn(async move {
                answer(&writes, id, stat(&node, id, &space, &path).await).await;
                let _ = internal.send(Internal::RequestDone).await;
            });
        }
        Down::Resolve {
            id,
            space,
            path,
            from,
        } => {
            if over_capacity(writes, streams, *requests, id) {
                return Ok(());
            }
            *requests += 1;
            let node = node.clone();
            let writes = writes.clone();
            let internal = internal.clone();
            tokio::spawn(async move {
                answer(&writes, id, resolve(&node, id, &space, &path, from).await).await;
                let _ = internal.send(Internal::RequestDone).await;
            });
        }
        Down::Read {
            id,
            root,
            size,
            start,
            len,
            credit,
        } => {
            // A second stream on a live id is a protocol error, not a
            // replacement: the old one's chunks would be read as the new
            // one's. Refusing keeps the id space the control plane's problem.
            if streams.contains_key(&id) {
                let _ = writes.try_send(text(&Up::Err {
                    id: Some(id),
                    code: "invalid".into(),
                    message: format!("request {id} is already streaming"),
                })?);
                return Ok(());
            }
            if over_capacity(writes, streams, *requests, id) {
                return Ok(());
            }
            let permits = Arc::new(Semaphore::new((credit as usize).min(MAX_CREDIT)));
            let task = {
                let node = node.clone();
                let writes = writes.clone();
                let internal = internal.clone();
                let permits = permits.clone();
                tokio::spawn(async move {
                    if let Err(e) = read(&node, &writes, id, &root, size, start, len, permits).await
                    {
                        let _ = writes.try_send(Message::text(
                            serde_json::to_string(&Up::Err {
                                id: Some(id),
                                code: code_of(&e).to_string(),
                                message: e.to_string(),
                            })
                            .unwrap_or_default(),
                        ));
                    }
                    // Whichever way it ended, the session loop drops the entry.
                    let _ = internal.send(Internal::ReadDone(id)).await;
                })
            };
            streams.insert(
                id,
                Stream {
                    credit: permits,
                    task,
                },
            );
        }
        // Handshake frames after the handshake are noise, not instructions.
        Down::Challenge { .. } | Down::Attached { .. } => {}
    }
    Ok(())
}

/// Whether a new request would exceed the per-session ceiling, refusing it with
/// a coded error if so.
fn over_capacity(
    writes: &Writer,
    streams: &HashMap<u32, Stream>,
    requests: usize,
    id: u32,
) -> bool {
    if streams.len() + requests < MAX_INFLIGHT {
        return false;
    }
    if let Ok(message) = text(&Up::Err {
        id: Some(id),
        code: "unavailable".into(),
        message: format!("this session already has {MAX_INFLIGHT} requests in flight"),
    }) {
        let _ = writes.try_send(message);
    }
    true
}

/// Sends one request's answer, or the coded refusal it failed with.
async fn answer(writes: &Writer, id: u32, outcome: Result<Up>) {
    let frame = match outcome {
        Ok(frame) => frame,
        Err(e) => Up::Err {
            id: Some(id),
            code: code_of(&e).to_string(),
            message: e.to_string(),
        },
    };
    if let Ok(message) = text(&frame) {
        let _ = writes.send(message).await;
    }
}

/// The stable code an engine failure travels as.
///
/// The same vocabulary the local control socket puts in `x-synch-error-code`,
/// so a refusal reads the same whether it reached the caller over the socket
/// or over the tunnel.
fn code_of(e: &EngineError) -> &'static str {
    match e {
        EngineError::NotFound(_) => "not-found",
        EngineError::Divergent { .. } => "divergent",
        EngineError::InRecovery { .. } => "unavailable",
        EngineError::Invalid(_) | EngineError::Key(_) => "invalid",
        EngineError::NotInitialized => "not-initialized",
        _ => "internal",
    }
}

fn text(frame: &Up) -> Result<Message> {
    serde_json::to_string(frame)
        .map(Message::text)
        .map_err(|e| EngineError::invalid(format!("could not encode a tunnel frame: {e}")))
}

async fn send<S>(sink: &mut S, frame: &Up) -> Result<()>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    sink.send(text(frame)?)
        .await
        .map_err(|e| EngineError::invalid(format!("the tunnel write failed: {e}")))
}

async fn receive<R>(stream: &mut R) -> Result<Down>
where
    R: futures_util::Stream<
            Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>,
        > + Unpin,
{
    loop {
        let message = stream
            .next()
            .await
            .ok_or_else(|| EngineError::invalid("the control plane closed the connection"))?
            .map_err(|e| EngineError::invalid(format!("the tunnel read failed: {e}")))?;
        match message {
            Message::Text(body) => {
                return serde_json::from_str(&body)
                    .map_err(|e| EngineError::invalid(format!("malformed tunnel frame: {e}")))
            }
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            Message::Binary(_) | Message::Close(_) => {
                return Err(EngineError::invalid(
                    "the control plane ended the handshake early",
                ))
            }
        }
    }
}

/// One page of a directory of the unified tree.
async fn list_page(
    node: &Node,
    space: &str,
    path: &str,
    cursor: Option<String>,
    all: bool,
) -> Result<Up> {
    let node = node.clone();
    let space = space.to_string();
    let prefix = directory_prefix(path);
    crate::blocking::offload(move || collapse(&node, &space, &prefix, cursor, all)).await
}

/// The listing prefix for a directory, which always ends at a boundary.
fn directory_prefix(path: &str) -> String {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}/")
    }
}

/// Folds a flat listing into one directory's entries.
///
/// The unified tree is a flat set of paths, so a directory view is a fold over
/// it: paths with a separator left collapse into one entry for the
/// subdirectory. Because the flat listing is path-ordered, a subdirectory's
/// rows are contiguous — so once its entry is emitted the cursor can be
/// advanced *past the whole subtree* rather than through it, and no
/// subdirectory is ever emitted on two pages.
fn collapse(
    node: &Node,
    space: &str,
    prefix: &str,
    cursor: Option<String>,
    all: bool,
) -> Result<Up> {
    let mut entries: Vec<EntryJson> = Vec::new();
    let mut start_after = cursor;
    // The subtree whose rows this pass has already accounted for with one
    // directory entry, and is walking past.
    let mut inside: Option<String> = None;
    // Whether the listing stopped at the page limit rather than at its end.
    let mut more = false;
    // One reading for the whole answer, so every path in it selects against
    // the same instant.
    let now = node.store().read_instant()?;
    'pages: loop {
        let batch =
            node.unified_listing(space, prefix, start_after.as_deref(), Some(SCAN_BATCH))?;
        let exhausted = batch.len() < SCAN_BATCH;
        for set in &batch {
            if inside
                .as_ref()
                .is_some_and(|subtree| set.path.starts_with(subtree))
            {
                continue;
            }
            inside = None;
            start_after = Some(set.path.clone());
            if !set.exists() {
                // Every publisher has tombstoned it: the path has left the
                // tree, so the tree does not list it.
                continue;
            }
            let Some(rest) = set.path.strip_prefix(prefix) else {
                continue;
            };
            match rest.split_once('/') {
                Some((child, _)) => {
                    let full = format!("{prefix}{child}");
                    entries.push(EntryJson {
                        name: child.to_string(),
                        path: full.clone(),
                        kind: "dir".into(),
                        size: 0,
                        mtime_ns: 0,
                        versions: 0,
                        origin: String::new(),
                        root: None,
                        all: Vec::new(),
                    });
                    // The successor of `<full>/`: every path in the subtree
                    // sorts below it, and the next sibling sorts above. The
                    // rows already in hand are walked past rather than
                    // re-queried, and the cursor skips the rest of them — so a
                    // subdirectory is never emitted on two pages.
                    inside = Some(format!("{full}/"));
                    start_after = Some(format!("{full}0"));
                }
                None => {
                    // A path the policy refuses is left out rather than shown
                    // with one side's metadata, exactly as `List` does; a
                    // direct stat of it still says what is wrong.
                    let Ok(row) = node.resolve_set(set, &VersionPolicy::Newest, now) else {
                        continue;
                    };
                    entries.push(EntryJson {
                        name: rest.to_string(),
                        path: set.path.clone(),
                        kind: kind_name(row.kind).into(),
                        size: row.size,
                        mtime_ns: row.mtime_ns,
                        versions: set.version_count() as u32,
                        origin: row.origin.canonical(),
                        root: row.content.map(|root| root.to_hex().to_string()),
                        all: if all { versions_json(set) } else { Vec::new() },
                    });
                }
            }
            if entries.len() >= PAGE_LIMIT {
                more = true;
                break 'pages;
            }
        }
        if exhausted {
            break;
        }
    }
    Ok(Up::Page {
        // The id is filled in by the caller; this shape exists so the fold can
        // run on the blocking pool without carrying it.
        id: 0,
        entries,
        // Where the next page resumes: the seek key the last entry left
        // behind, which is past a subdirectory's whole subtree when the page
        // ended on one.
        cursor: more.then(|| start_after.clone()).flatten(),
    })
}

fn kind_name(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::File => "file",
        EntryKind::Dir => "dir",
        EntryKind::Symlink => "symlink",
        EntryKind::Tombstone => "tombstone",
        EntryKind::Socket => "socket",
    }
}

fn versions_json(set: &VersionSet) -> Vec<VersionJson> {
    set.versions
        .iter()
        .map(|version| VersionJson {
            root: version.content.map(|root| root.to_hex().to_string()),
            kind: kind_name(version.kind).into(),
            symlink_target: version.symlink_target.clone(),
            size: version.size,
            mtime_ns: version.mtime_ns,
            seq: version.seq,
            attestors: version
                .attestors
                .iter()
                .map(|origin| origin.canonical())
                .collect(),
        })
        .collect()
}

/// Every version of one path, with its attestors — `synch status` as frames.
///
/// Answered on the blocking pool, like every other request below: these run in
/// tasks spawned onto the daemon's runtime, and the store reads under them wait
/// on the same connection mutex the publisher and the anti-entropy rounds do
/// (§10).
async fn stat(node: &Node, id: u32, space: &str, path: &str) -> Result<Up> {
    let node = node.clone();
    let (space, path) = (space.to_string(), path.to_string());
    crate::blocking::offload(move || {
        let set = node.versions(&space, &path)?;
        Ok(Up::Versions {
            id,
            versions: versions_json(&set),
        })
    })
    .await
}

/// Every delegation this node honors, with the cascade already applied (§3.5).
///
/// Answers for the cluster rather than for this node: a delegation is a `d:`
/// record in its issuer's trie, replicated to every member, so whichever node
/// the control plane happens to be attached to holds them all. That is the
/// transitive-trust concession made legible — an operator can see from one
/// place who was admitted, by whom, and to what.
///
/// Lapsed rows travel alongside live ones, marked. A dashboard that showed
/// only what is live could not distinguish "never delegated" from "delegated,
/// and the issuer is gone", and those call for different actions.
async fn delegations(node: &Node, id: u32) -> Result<Up> {
    let node = node.clone();
    // One hop for both reads: the live set is the same query filtered by the
    // cascade, and nothing between them awaits.
    crate::blocking::offload(move || {
        let now = synch_core::now_ns();
        let live: std::collections::HashSet<(Vec<u8>, String)> = node
            .store()
            .delegations(now)?
            .into_iter()
            .map(|b| {
                (
                    b.node_id.as_bytes().to_vec(),
                    b.issuer.map(|i| i.canonical()).unwrap_or_default(),
                )
            })
            .collect();
        let delegations = node
            .delegations()?
            .into_iter()
            .map(|b| {
                let issuer = b.issuer.map(|i| i.canonical()).unwrap_or_default();
                DelegationJson {
                    key: b.node_id.to_z32(),
                    live: live.contains(&(b.node_id.as_bytes().to_vec(), issuer.clone())),
                    issuer,
                    spaces: b.spaces,
                    not_after: b.expires_at,
                    added_at: b.added_at,
                    note: b.note,
                }
            })
            .collect();
        Ok(Up::Delegations { id, delegations })
    })
    .await
}

/// What this node replicates, and how far behind it is
/// (`docs/REPLICATION.md` §8).
///
/// Answers for this node alone, which is the whole difference from
/// [`delegations`] above: a delegation is a `d:` record every member holds, so
/// any node speaks for the cluster, while replication is a decision each node
/// makes for itself. Two nodes of one network can disagree about whether
/// `media` is replicated and both be right, so an answer that did not say
/// *whose* it was would be worse than no answer.
///
/// Synchronization health is read once rather than per space. It is a property
/// of this node's whole picture — a pending head, or a bound origin it has
/// never synced — and asking per space would re-run the same two scans of
/// `heads` and `bindings` for every replica to reach the same verdict.
async fn replication(node: &Node, id: u32) -> Result<Up> {
    let node = node.clone();
    crate::blocking::offload(move || {
        let view = node.view_state()?;
        let floor = node.config().replica_release_floor;
        let mut spaces = Vec::new();
        for space in node.store().replicas()? {
            let policy = space.retention;
            let holder = space.holder();
            let coverage = node
                .store()
                .replica_coverage(&holder, crate::replica::UNREACHABLE_ATTEMPTS)?;
            spaces.push(ReplicaSpaceJson {
                space: space.space.clone(),
                policy: policy.render().to_string(),
                grace_secs: space.grace_secs(),
                budget: space.budget,
                held: coverage.held,
                held_bytes: coverage.held_bytes,
                releasing: coverage.releasing,
                releasing_bytes: coverage.releasing_bytes,
                wanted: coverage.wanted,
                wanted_bytes: coverage.wanted_bytes,
                unreachable: coverage.unreachable,
                unreachable_bytes: coverage.unreachable_bytes,
                // Meaningless where the policy never releases, and reported as
                // zero there rather than as a count: under `forever` nothing is
                // waiting on peers, so "too few peers to let these go" would
                // describe a release that is not pending.
                held_back: match policy.releases() {
                    true => node
                        .store()
                        .held_back_by_replication_floor(&holder, floor)?,
                    false => 0,
                },
                oldest_want: node
                    .store()
                    .oldest_want(&holder, crate::replica::UNREACHABLE_ATTEMPTS)?,
                next_release: node.store().next_release(&holder)?,
                view_complete: view.is_complete(),
                view_reason: view.reason().map(str::to_string),
            });
        }
        Ok(Up::Replication { id, spaces })
    })
    .await
}

/// Pins a path to one content root and names who holds it.
async fn resolve(
    node: &Node,
    id: u32,
    space: &str,
    path: &str,
    from: Option<String>,
) -> Result<Up> {
    let policy = match &from {
        Some(origin) => VersionPolicy::Origin(
            origin
                .parse::<OriginId>()
                .map_err(|e| EngineError::invalid(e.to_string()))?,
        ),
        None => VersionPolicy::Newest,
    };
    // One hop for the selection, the provider list and the local completeness
    // check together: they are three reads on the connection the first one
    // takes, and nothing between them awaits.
    let node = node.clone();
    let (space, path) = (space.to_string(), path.to_string());
    crate::blocking::offload(move || {
        let row = node.resolve(&space, &path, &policy)?;
        if row.kind == EntryKind::Tombstone {
            return Err(EngineError::not_found(format!(
                "{space}/{path} was deleted at seq {}",
                row.seq
            )));
        }
        let root = row
            .content
            .ok_or_else(|| EngineError::invalid(format!("{space}/{path} selects no content")))?;
        // Straight out of the replicated `b:` records: a routing hint the
        // control plane may act on or ignore, never a correctness input.
        let mut holders: Vec<String> = node
            .providers_for(&root, 0, row.size.max(1))?
            .into_iter()
            .map(|provider| provider.origin.canonical())
            .collect();
        if node.store().blob(&root)?.is_some_and(|blob| blob.complete) {
            holders.push(node.origin().canonical());
        }
        Ok(Up::Resolved {
            id,
            origin: row.origin.canonical(),
            root: root.to_hex().to_string(),
            size: row.size,
            seq: row.seq,
            holders,
        })
    })
    .await
}

/// Streams a byte range of a pinned content root under credit flow control.
///
/// Addressed by root rather than by path, so a publish landing between the
/// resolve and the read cannot swap the bytes mid-download: the reader gets
/// exactly the version the resolve named, or a clean error if it has gone.
#[allow(clippy::too_many_arguments)]
async fn read(
    node: &Node,
    writes: &Writer,
    id: u32,
    root: &str,
    size: u64,
    start: u64,
    len: Option<u64>,
    credit: Arc<Semaphore>,
) -> Result<()> {
    let root: Hash = root
        .parse()
        .map_err(|_| EngineError::invalid(format!("{root} is not an object root")))?;
    if start > size {
        return Err(EngineError::invalid(format!(
            "offset {start} is past the end of a {size}-byte object"
        )));
    }
    let end = match len {
        Some(len) => start.saturating_add(len).min(size),
        None => size,
    };
    // Whatever of the window this node does not hold is fetched from peers
    // first, bao-verified per range, so every byte below is verified content.
    let report = node.fetch_range(&root, size, start, end).await?;
    if !report.complete {
        return Err(EngineError::not_found(format!(
            "no provider could serve bytes {start}..{end} of {root}"
        )));
    }
    let _ = writes
        .send(text(&Up::Meta {
            id,
            size: end - start,
            root: root.to_hex().to_string(),
        })?)
        .await;

    let mut offset = start;
    let mut seq = 0u32;
    while offset < end {
        // Read-ahead happens only while credit is available: a browser that
        // stops reading stalls the read here, at the source, rather than
        // filling a buffer at any hop between.
        let permit = credit
            .acquire()
            .await
            .map_err(|_| EngineError::invalid("the stream was cancelled"))?;
        permit.forget();
        let take = (MAX_CHUNK as u64).min(end - offset);
        let bytes = node.cas_backend().read_range(root, offset, take).await?;
        if bytes.is_empty() {
            break;
        }
        offset += bytes.len() as u64;
        if writes
            .send(Message::binary(encode_chunk(id, seq, &bytes)))
            .await
            .is_err()
        {
            return Ok(());
        }
        seq = seq.wrapping_add(1);
    }
    let _ = writes.send(text(&Up::Done { id })?).await;
    Ok(())
}

/// Fills in a page's request id, which the blocking fold does not carry.
fn with_id(frame: Up, id: u32) -> Up {
    match frame {
        Up::Page {
            entries, cursor, ..
        } => Up::Page {
            id,
            entries,
            cursor,
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{cloud::frame::decode_chunk, testkit::node, NodeConfig};
    use synch_core::{FileEntry, Hash};

    fn origin(name: &str) -> OriginId {
        OriginId::named(name, "x.example").unwrap()
    }

    /// Encodes one control frame as the control plane would put it on the wire.
    fn down_msg(frame: &Down) -> Message {
        Message::text(serde_json::to_string(frame).unwrap())
    }

    /// The next text frame the node sent, decoded — skipping binary chunks so a
    /// caller waiting for a control answer is not tripped by content.
    async fn next_up(rx: &mut mpsc::UnboundedReceiver<Message>) -> Up {
        loop {
            match rx.recv().await.expect("the node sent a frame") {
                Message::Text(body) => {
                    let up: Up = serde_json::from_str(&body).expect("a valid Up");
                    // Skip the node's own heartbeat pings; a Pong answering the
                    // test's ping is a real frame and is returned.
                    if matches!(up, Up::Ping) {
                        continue;
                    }
                    return up;
                }
                Message::Binary(_) => continue,
                other => panic!("unexpected frame {other:?}"),
            }
        }
    }

    /// Plants a delegated binding as materializing an issuer's `d:` record does.
    fn delegate(node: &Node, issuer: &OriginId, subject: synch_core::NodeId, spaces: &[&str]) {
        node.store()
            .put_binding(&synch_store::Binding {
                origin: OriginId::Key(subject),
                node_id: subject,
                source: synch_store::BindingSource::Delegated,
                domain: None,
                issuer: Some(issuer.clone()),
                spaces: spaces.iter().map(|s| s.to_string()).collect(),
                note: Some("planted".into()),
                added_at: 1,
                expires_at: Some(synch_core::now_ns() + 86_400_000_000_000),
            })
            .unwrap();
    }

    /// A fresh node under a blocking scope, as the wire tests need.
    async fn scoped_node() -> (synch_core::BlockingScope, tempfile::TempDir, Node) {
        let blocking = synch_core::BlockingScope::enter();
        let (dir, node) = node().await;
        (blocking, dir, node)
    }

    /// Runs `serve` end to end over an in-process pair of channels, returning
    /// the Down half, the Up half, and the served task.
    fn serve_session(
        node: &Node,
    ) -> (
        mpsc::UnboundedSender<Message>,
        mpsc::UnboundedReceiver<Message>,
        tokio::task::JoinHandle<()>,
    ) {
        let (to_node, node_rx) = mpsc::unbounded_channel::<Message>();
        let (node_tx, from_node) = mpsc::unbounded_channel::<Message>();
        let stream = Box::pin(futures_util::stream::unfold(node_rx, |mut rx| async move {
            rx.recv()
                .await
                .map(|m| (Ok::<Message, tokio_tungstenite::tungstenite::Error>(m), rx))
        }));
        let sink = Box::pin(futures_util::sink::unfold(
            node_tx,
            |tx, m: Message| async move {
                tx.send(m)
                    .map_err(|_| tokio_tungstenite::tungstenite::Error::ConnectionClosed)?;
                Ok::<_, tokio_tungstenite::tungstenite::Error>(tx)
            },
        ));
        let node = node.clone();
        let served = tokio::spawn(async move {
            let _ = serve(&node, sink, stream, Vec::new(), PROTOCOL_VERSION).await;
        });
        (to_node, from_node, served)
    }

    /// The control plane can ask who the cluster admits, and is told which
    /// grants still hold (§3.5). The cascade is the point: both rows below
    /// are inside their own expiry and differ only in whether their issuer
    /// is still trusted here — the reporting hole the delegation work closed
    /// locally, closed over the tunnel.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn serve_answers_a_delegations_query_with_the_cascade_applied() {
        let (_blocking, _dir, node) = scoped_node().await;

        // A rooted issuer, and one this node holds no binding for at all.
        let rooted = origin("nas");
        let rooted_key = iroh_base::SecretKey::generate().public();
        node.store()
            .put_binding(&synch_store::Binding {
                origin: rooted.clone(),
                node_id: rooted_key,
                source: synch_store::BindingSource::Static,
                domain: None,
                issuer: None,
                spaces: Vec::new(),
                note: None,
                added_at: 0,
                expires_at: None,
            })
            .unwrap();
        let stranger = origin("gone");

        let held = iroh_base::SecretKey::generate().public();
        let orphaned = iroh_base::SecretKey::generate().public();
        delegate(&node, &rooted, held, &["photos"]);
        delegate(&node, &stranger, orphaned, &["finance"]);

        let (to_node, mut from_node, served) = serve_session(&node);

        to_node
            .send(down_msg(&Down::Delegations { id: 7 }))
            .unwrap();
        let Up::Delegations { id, delegations } = next_up(&mut from_node).await else {
            panic!("expected a delegations answer")
        };
        assert_eq!(id, 7);
        assert_eq!(delegations.len(), 2, "{delegations:?}");

        let live = delegations
            .iter()
            .find(|d| d.key == held.to_z32())
            .expect("the rooted issuer's delegation");
        assert!(live.live, "a rooted issuer's grant holds");
        assert_eq!(live.issuer, rooted.canonical());
        assert_eq!(live.spaces, ["photos"]);
        assert_eq!(live.note.as_deref(), Some("planted"));
        assert!(live.not_after.is_some(), "the end date travels too");

        let lapsed = delegations
            .iter()
            .find(|d| d.key == orphaned.to_z32())
            .expect("the stranger's delegation");
        assert!(!lapsed.live, "an untrusted issuer vouches for nobody");
        // Reported rather than hidden: "never delegated" and "delegated by
        // someone since cut off" are different states, different actions.
        assert_eq!(lapsed.issuer, stranger.canonical());
        assert_eq!(lapsed.spaces, ["finance"]);

        drop(to_node);
        let _ = served.await;
        node.shutdown().await.unwrap();
    }

    /// The control plane can ask what this node replicates, and is told about
    /// this node alone (`docs/REPLICATION.md` §8).
    ///
    /// Two properties, and both are why the answer is per node rather than per
    /// cluster: a space this node does not replicate is absent from the answer
    /// entirely, and the space it does replicate reports *its* queue. A
    /// dashboard that showed one node's backlog against another's name would be
    /// worse than showing nothing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn serve_answers_a_replication_query_for_this_node_alone() {
        let (_blocking, dir, node) = scoped_node().await;

        // One replica and one ordinary one. The ordinary one exists to
        // be missing from the answer.
        node.add_filesystem_source("local", dir.path().join("local"))
            .unwrap();
        node.add_api_source("media").unwrap();
        node.add_replica(
            "media",
            synch_store::ReplicaPolicy::Current,
            Some(3600),
            Some(1 << 30),
            None,
        )
        .unwrap();

        // A want the fetch loop has not reached yet, so the counts under test
        // are not all zero and `oldest_want` has something to carry.
        let wanted = synch_core::Hash::new(b"a version nobody has sent yet");
        node.store()
            .stage_want(
                &wanted,
                &synch_store::PinHolder::Replica("media".into()),
                4096,
                None,
                1_700_000_000_000_000_000,
            )
            .unwrap();

        let (to_node, mut from_node, served) = serve_session(&node);
        to_node
            .send(down_msg(&Down::Replication { id: 9 }))
            .unwrap();
        let Up::Replication { id, spaces } = next_up(&mut from_node).await else {
            panic!("expected a replication answer")
        };
        assert_eq!(id, 9);
        assert_eq!(spaces.len(), 1, "only the replica is reported: {spaces:?}");

        let row = &spaces[0];
        assert_eq!(row.space, "media");
        assert_eq!(row.policy, "current");
        assert_eq!(row.grace_secs, 3600);
        assert_eq!(row.budget, Some(1 << 30));
        assert_eq!(row.held, 0);
        assert_eq!(row.wanted, 1, "the staged want is the backlog");
        assert_eq!(row.wanted_bytes, 4096);
        assert_eq!(
            row.unreachable, 0,
            "a want that has not been attempted has not failed"
        );
        assert_eq!(row.oldest_want, Some(1_700_000_000_000_000_000));
        assert_eq!(row.next_release, None);
        assert!(
            row.view_complete && row.view_reason.is_none(),
            "a node with nothing pending has a complete view: {row:?}"
        );

        drop(to_node);
        let _ = served.await;
        node.shutdown().await.unwrap();
    }

    /// Runs `serve` end to end over an in-process socket: JSON control frames,
    /// the binary CHUNK codec, credit flow, and the request-id multiplexing
    /// across LS → RESOLVE → READ → PING. The attach handshake itself is
    /// `attach_once`, tested against the control plane's signing bytes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn serve_answers_ls_resolve_and_read_over_the_wire() {
        let (_blocking, dir, node) = scoped_node().await;
        node.add_filesystem_source("media", dir.path().join("media"))
            .unwrap();
        // Larger than one chunk, so the CHUNK framing and multi-chunk credit path run.
        let payload = vec![0xABu8; MAX_CHUNK + 4321];
        let root = node
            .store()
            .ingest_bytes(&payload, synch_core::now_ns())
            .unwrap();
        node.store()
            .put_entry(
                &origin("nas"),
                "media",
                "big.bin",
                &FileEntry::file(payload.len() as u64, 1, root, 1),
            )
            .unwrap();

        // Two halves of one socket: Down frames in, Up frames out.
        let (to_node, mut from_node, served) = serve_session(&node);

        // LS: the directory lists the file.
        to_node
            .send(down_msg(&Down::Ls {
                id: 1,
                space: "media".into(),
                path: String::new(),
                cursor: None,
                all: false,
            }))
            .unwrap();
        let Up::Page { id, entries, .. } = next_up(&mut from_node).await else {
            panic!("expected a page")
        };
        assert_eq!(id, 1);
        assert!(entries.iter().any(|e| e.name == "big.bin"));

        // RESOLVE: pin the version to its content root.
        to_node
            .send(down_msg(&Down::Resolve {
                id: 2,
                space: "media".into(),
                path: "big.bin".into(),
                from: None,
            }))
            .unwrap();
        let Up::Resolved { id, root, size, .. } = next_up(&mut from_node).await else {
            panic!("expected a resolution")
        };
        assert_eq!(id, 2);
        assert_eq!(size, payload.len() as u64);

        // READ by pinned root: META, ≤64 KiB chunks under the initial credit, then DONE.
        to_node
            .send(down_msg(&Down::Read {
                id: 3,
                root: root.clone(),
                size,
                start: 0,
                len: None,
                credit: 8,
            }))
            .unwrap();
        let Up::Meta {
            id,
            size: meta_size,
            root: meta_root,
        } = next_up(&mut from_node).await
        else {
            panic!("expected a meta frame before content")
        };
        assert_eq!(id, 3);
        assert_eq!(meta_size, payload.len() as u64);
        assert_eq!(meta_root, root);

        let mut got = Vec::new();
        loop {
            match from_node.recv().await.expect("a frame") {
                Message::Binary(bytes) => {
                    let (chunk_id, _seq, data) = decode_chunk(&bytes).expect("a content frame");
                    assert_eq!(chunk_id, 3);
                    assert!(data.len() <= MAX_CHUNK);
                    got.extend_from_slice(data);
                }
                Message::Text(body) => {
                    if let Up::Done { id } = serde_json::from_str(&body).unwrap() {
                        assert_eq!(id, 3);
                        break;
                    }
                }
                other => panic!("unexpected frame {other:?}"),
            }
        }
        assert_eq!(got, payload, "the streamed bytes match the file");

        // PING/PONG multiplexes over the same connection.
        to_node.send(down_msg(&Down::Ping)).unwrap();
        assert!(matches!(next_up(&mut from_node).await, Up::Pong));

        // Closing the Down half ends the session cleanly.
        drop(to_node);
        let _ = served.await;
        node.shutdown().await.unwrap();
    }

    /// No validated record, no connection: a resolver outage degrades the
    /// feature to off, never to "attach somewhere else".
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn discovery_without_a_resolver_attempts_nothing() {
        let _blocking = synch_core::BlockingScope::enter();
        let (_d, node) = node().await;
        let e = discover(None, "cluster.example").await.unwrap_err();
        assert!(e.to_string().contains("no DNSSEC resolver"), "{e}");
        node.shutdown().await.unwrap();
    }

    /// The override names a fleet the same way a zone does, and every entry
    /// gets a tunnel of its own.
    #[test]
    fn the_override_names_every_endpoint() {
        assert_eq!(
            overridden_endpoints("https://cp.example/ , https://ns1.cp.example").unwrap(),
            vec!["https://cp.example", "https://ns1.cp.example"]
        );
        // One is still the ordinary case.
        assert_eq!(
            overridden_endpoints("https://cp.example").unwrap(),
            vec!["https://cp.example"]
        );
        // Set but empty is a mistake worth naming, not a silent no-op.
        assert!(overridden_endpoints("").is_err());
        assert!(overridden_endpoints(" , ").is_err());
    }

    /// The task list follows the domains without being asked — the feature
    /// needs no enabling — and empties only when the operator opts out.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn attach_targets_follow_the_zone_and_the_opt_out() {
        // A key-identified node names no zone, so there is no apex to take a
        // control plane from — nothing to attach to, tunnel or no tunnel.
        let (_blocking, _d, node) = scoped_node().await;
        assert!(node.attach_targets().await.is_empty());
        node.shutdown().await.unwrap();

        // A node named by its zone attaches to that zone, and only the opt-out
        // empties the list.
        let dir = tempfile::tempdir().unwrap();
        Node::init_named_by_zone(
            dir.path(),
            OriginId::named("nas", "cluster.example").unwrap(),
        )
        .unwrap();
        let node = Node::open(NodeConfig::loopback(dir.path())).await.unwrap();
        assert_eq!(node.attach_targets().await, ["cluster.example"]);
        node.disable_cloud().unwrap();
        assert!(node.attach_targets().await.is_empty());
        node.enable_cloud().unwrap();
        assert_eq!(node.attach_targets().await, ["cluster.example"]);
        node.shutdown().await.unwrap();
    }

    /// A replica-only node is routable for what it holds.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_claim_covers_replica_only_spaces_too() {
        let (_blocking, dir, node) = scoped_node().await;
        node.add_filesystem_source("docs", dir.path().join("docs"))
            .unwrap();
        node.add_replica(
            "media",
            synch_store::ReplicaPolicy::Current,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(held_spaces(&node).unwrap(), ["docs", "media"]);

        // A space with both roles is one routing claim, not two.
        node.add_filesystem_source("media", dir.path().join("media"))
            .unwrap();
        assert_eq!(held_spaces(&node).unwrap(), ["docs", "media"]);
        node.shutdown().await.unwrap();
    }

    /// The hosted-data-plane startup order opens the tunnel before its first
    /// convergence pass adds replicas. That must update the live claim rather
    /// than leave the new space unroutable for the life of the connection.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_live_session_refreshes_its_routing_claim() {
        let (_blocking, _dir, node) = scoped_node().await;
        let (writes, mut outgoing) = mpsc::channel(2);
        let mut advertised = held_spaces(&node).unwrap();
        assert!(advertised.is_empty());

        node.add_replica(
            "media",
            synch_store::ReplicaPolicy::Current,
            None,
            None,
            None,
        )
        .unwrap();
        refresh_space_claim(
            &writes,
            &mut advertised,
            PROTOCOL_VERSION,
            held_spaces(&node).unwrap(),
        )
        .unwrap();

        let Message::Text(body) = outgoing.recv().await.unwrap() else {
            panic!("the routing update is a text frame")
        };
        let Up::Spaces { spaces } = serde_json::from_str(&body).unwrap() else {
            panic!("expected a replacement routing claim")
        };
        assert_eq!(spaces, ["media"]);
        assert_eq!(advertised, ["media"]);

        // A new daemon rolling out before its control plane cannot send the
        // v4 frame. It ends this session so its next v2/v3 hello still carries
        // the current replacement claim.
        node.add_replica(
            "docs",
            synch_store::ReplicaPolicy::Current,
            None,
            None,
            None,
        )
        .unwrap();
        let error = refresh_space_claim(
            &writes,
            &mut advertised,
            SPACE_UPDATES_VERSION - 1,
            held_spaces(&node).unwrap(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("reconnecting"), "{error}");
        assert_eq!(advertised, ["media"]);
        node.shutdown().await.unwrap();
    }

    /// Credit is the whole of the read-ahead bound: with none granted, no
    /// chunk is produced, however much of the object is already local.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_read_produces_nothing_without_credit() {
        let (_blocking, dir, node) = scoped_node().await;
        node.add_filesystem_source("media", dir.path().join("media"))
            .unwrap();
        let payload = vec![7u8; MAX_CHUNK * 3];
        let root = node
            .store()
            .ingest_bytes(&payload, synch_core::now_ns())
            .unwrap();
        node.store()
            .put_entry(
                &origin("nas"),
                "media",
                "f",
                &FileEntry::file(payload.len() as u64, 1, root, 1),
            )
            .unwrap();

        let (writes, mut rx) = mpsc::channel(16);
        let credit = Arc::new(Semaphore::new(0));
        let reading = {
            let node = node.clone();
            let credit = credit.clone();
            tokio::spawn(async move {
                read(
                    &node,
                    &writes,
                    3,
                    &root.to_hex().to_string(),
                    payload.len() as u64,
                    0,
                    None,
                    credit,
                )
                .await
            })
        };
        // The header, and then nothing: the producer is parked on a permit.
        let first = rx.recv().await.unwrap();
        assert!(matches!(first, Message::Text(_)), "{first:?}");
        let quiet = tokio::time::timeout(Duration::from_millis(200), rx.recv())
            .await
            .is_err();
        assert!(quiet);

        // One credit, one chunk, and then parked again.
        credit.add_permits(1);
        let chunk = tokio::time::timeout(Duration::from_millis(2000), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let Message::Binary(bytes) = chunk else {
            panic!("expected a content frame, got {chunk:?}")
        };
        let (id, seq, data) = decode_chunk(&bytes).unwrap();
        assert_eq!((id, seq, data.len()), (3, 0, MAX_CHUNK));
        let quiet = tokio::time::timeout(Duration::from_millis(200), rx.recv())
            .await
            .is_err();
        assert!(quiet);

        // And a cancelled stream ends the producer rather than leaking it.
        credit.close();
        assert!(reading.await.unwrap().is_err());
        node.shutdown().await.unwrap();
    }

    /// Credit is a promise to read, and the control plane may promise more
    /// than the semaphore can hold. Repeated grants stop at the ceiling
    /// instead of overflowing it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn credit_grants_stop_at_the_ceiling() {
        let (_blocking, _dir, node) = scoped_node().await;
        let (writes, _rx) = mpsc::channel(16);
        let (internal, _drain) = mpsc::channel(16);
        let credit = Arc::new(Semaphore::new(0));
        let mut streams = HashMap::from([(
            7,
            Stream {
                credit: credit.clone(),
                task: tokio::spawn(std::future::pending()),
            },
        )]);
        let mut requests = 0;
        for _ in 0..4 {
            handle(
                &node,
                &writes,
                &internal,
                &mut streams,
                &mut requests,
                Down::Credit { id: 7, n: u32::MAX },
            )
            .unwrap();
        }
        assert_eq!(credit.available_permits(), MAX_CREDIT);
        // An unopened stream is not a place to bank credit either.
        handle(
            &node,
            &writes,
            &internal,
            &mut streams,
            &mut requests,
            Down::Credit { id: 9, n: 1 },
        )
        .unwrap();
        streams.clear();
        node.shutdown().await.unwrap();
    }

    /// Log lines and error text carry strings the control plane chose, so
    /// escape sequences and unbounded length are cut before they land.
    #[test]
    fn peer_text_is_stripped_and_bounded() {
        assert_eq!(brief("plain \u{1b}[31mred\n"), "plain [31mred");
        assert_eq!(brief(&"x".repeat(10_000)).len(), 200);
    }

    /// A directory view is a fold over the flat tree, and a subdirectory is
    /// one row however many paths sit under it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_listing_collapses_subdirectories() {
        let (_blocking, dir, node) = scoped_node().await;
        node.add_filesystem_source("media", dir.path().join("media"))
            .unwrap();
        for path in [
            "a.txt",
            "photos/1.jpg",
            "photos/2.jpg",
            "photos/trips/x.jpg",
        ] {
            node.store()
                .put_entry(
                    &origin("nas"),
                    "media",
                    path,
                    &FileEntry::file(1, 4, Hash::new(path.as_bytes()), 1),
                )
                .unwrap();
        }
        let Up::Page {
            entries, cursor, ..
        } = list_page(&node, "media", "", None, false).await.unwrap()
        else {
            panic!("expected a page")
        };
        assert_eq!(cursor, None);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a.txt", "photos"]);
        assert_eq!(entries[1].kind, "dir");

        // And one level down, the nested directory collapses in turn.
        let Up::Page { entries, .. } = list_page(&node, "media", "photos", None, false)
            .await
            .unwrap()
        else {
            panic!("expected a page")
        };
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["1.jpg", "2.jpg", "trips"]);

        // Divergence is data the listing carries: two origins' versions report both.
        for (name, content) in [("nas", b"a"), ("laptop", b"b")] {
            node.store()
                .put_entry(
                    &origin(name),
                    "media",
                    "split",
                    &FileEntry::file(1, 1, Hash::new(content), 1),
                )
                .unwrap();
        }
        let Up::Page { entries, .. } = list_page(&node, "media", "", None, true).await.unwrap()
        else {
            panic!("expected a page")
        };
        let split = entries
            .iter()
            .find(|e| e.name == "split")
            .expect("the divergent path");
        assert_eq!(split.versions, 2);
        assert_eq!(split.all.len(), 2);

        let Up::Versions { versions, .. } = stat(&node, 1, "media", "split").await.unwrap() else {
            panic!("expected versions")
        };
        assert_eq!(versions.len(), 2);
        assert!(versions.iter().all(|v| v.attestors.len() == 1));
        node.shutdown().await.unwrap();
    }
}
