//! TUF-driven pin refresh end to end (docs/REKOR-ZONE-KEY.md §10.2, §10.5).
//! Three layers: the **conformance** half runs the real Sigstore chain through
//! the real verifier; the **synthetic** half exercises rotation, thresholds,
//! expiry, rollback, tampering and revocation; the **resolver** half proves the
//! §10.2 rules through the whole refresh path with injected repositories.

mod common;

use std::sync::Arc;

use synch_net::{
    dns::{DnssecResolver, RekorPolicy, ResolverOptions},
    error::NetError,
    rekor::{self, LogKeys},
    sim::{SimLog, SimTuf, SimZone},
    tuf::{self, PinState, TufError, TufMetadata},
};

/// The fixed moment the synthetic repositories are built and verified at.
const NOW: i64 = 1_800_000_000;

// ------------------------------------------------------- real-chain fixtures

fn fixture(name: &str) -> Vec<u8> {
    common::fixture("tests/fixtures/tuf", name)
}

fn fixture_field(name: &str) -> String {
    common::fixture_field("tests/fixtures/tuf", name)
}

fn fixture_number(name: &str) -> u64 {
    fixture_field(name).parse().expect("a fixture number")
}

/// The checked-in Sigstore metadata: the embedded version plus the root history in front of it.
fn fixture_chain_metadata() -> TufMetadata {
    TufMetadata {
        roots: fixture_field("root_versions")
            .split(',')
            .map(|version| fixture(&format!("root-{version}.json")))
            .collect(),
        timestamp: fixture("timestamp.json"),
        snapshot: fixture("snapshot.json"),
        targets: fixture("targets.json"),
        trusted_root: fixture("trusted-root.json"),
    }
}

/// A stock client's state: the root chain anchored at the floor §10.2 states per release.
fn floor_state() -> PinState {
    let floor = fixture_number("chain_floor");
    PinState {
        root_version: floor,
        ..PinState::anchored(&fixture(&format!("root-{floor}.json")))
    }
}

/// The real Sigstore chain through the real verifier, ending in the pin set a
/// client would adopt — canonical JSON is where TUF implementations historically
/// break, so it is checked against bytes somebody else produced.
#[test]
fn the_real_sigstore_chain_verifies_and_yields_the_pin_set() {
    let update = tuf::update(
        &fixture_chain_metadata(),
        &floor_state(),
        fixture_number("verify_at"),
    )
    .expect("the real chain must verify");
    assert!(update.changed);
    assert_eq!(update.state.root_version, fixture_number("root_version"));
    assert_eq!(
        update.state.timestamp_version,
        fixture_number("timestamp_version")
    );
    assert_eq!(
        update.state.snapshot_version,
        fixture_number("snapshot_version")
    );
    assert_eq!(
        update.state.targets_version,
        fixture_number("targets_version")
    );
    assert_eq!(update.state.trusted_root, fixture("trusted-root.json"));

    // The pin set it yields is the production log set, derived a different way than the build-time snapshot.
    let derived = update.log_keys;
    for id in fixture_field("log_ids").split(',') {
        let id: [u8; 32] = hex::decode(id).unwrap().try_into().unwrap();
        assert!(derived.find(&id).is_some(), "pin set is missing {id:?}");
    }
    assert_eq!(
        derived.keys().len(),
        fixture_field("log_ids").split(',').count()
    );
    assert_eq!(
        derived,
        LogKeys::embedded(),
        "the TUF-derived pin set and the embedded bootstrap snapshot are the same logs today"
    );

    // A year after the fixture was fetched, the same bytes are refused: expiry gates the update.
    assert!(matches!(
        tuf::update(
            &fixture_chain_metadata(),
            &floor_state(),
            fixture_number("verify_at") + 365 * 86_400
        ),
        Err(TufError::Expiry(_))
    ));

    // The build ships the head of the chain the fixture ends at.
    let embedded: serde_json::Value = serde_json::from_str(tuf::EMBEDDED_TUF_ROOT).unwrap();
    assert_eq!(
        embedded["signed"]["version"].as_u64(),
        Some(fixture_number("root_version"))
    );
    assert_eq!(
        tuf::EMBEDDED_TUF_ROOT.as_bytes(),
        fixture(&format!("root-{}.json", fixture_number("root_version")))
    );
}

// -------------------------------------------------------------- synthetic

/// A synthetic repository whose trusted root names one log.
fn repo() -> (SimTuf, SimLog) {
    let log = SimLog::new("rekor.sim");
    (SimTuf::new(NOW, &[&log]), log)
}

/// Update the repository from its embedded state at the fixed moment.
fn accept(repo: &SimTuf) -> tuf::TufUpdate {
    tuf::update(&repo.metadata(), &repo.embedded_state(), NOW as u64)
        .expect("a well-formed chain verifies")
}

/// A chained rotation walks every version in order, including a full re-keying;
/// a client at the head accepts a chain carrying only the tail; a gap bridges nothing.
#[test]
fn root_rotation_walks_every_version_in_order() {
    let (mut repo, log) = repo();
    repo.rotate_root(false);
    repo.rotate_root(true);
    repo.rotate_root(true);
    assert_eq!(repo.root_version(), 4);
    let update = accept(&repo);
    assert!(update.changed);
    assert_eq!(update.state.root_version, 4);
    assert!(update.log_keys.find(&log.log_id()).is_some());

    let mut gapped = repo.metadata();
    gapped.roots.remove(1);
    assert!(matches!(
        tuf::update(&gapped, &repo.embedded_state(), NOW as u64),
        Err(TufError::Chain(_))
    ));
    tuf::update(&repo.metadata_from(4), &update.state, NOW as u64)
        .expect("the tail alone still verifies");
}

/// A root the old root did not sign is refused — the fork a relay that
/// minted its own root would produce.
#[test]
fn a_root_the_old_root_did_not_sign_is_refused() {
    let (mut repo, _log) = repo();
    let honest = repo.embedded_state();
    repo.rotate_root(true);
    let mut forged = repo.metadata();
    let mut root: serde_json::Value = serde_json::from_slice(&forged.roots[1]).unwrap();
    let keyids: Vec<String> = root["signed"]["roles"]["root"]["keyids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_string())
        .collect();
    root["signatures"] = serde_json::Value::Array(
        root["signatures"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|sig| keyids.contains(&sig["keyid"].as_str().unwrap().to_string()))
            .cloned()
            .collect(),
    );
    forged.roots[1] = root.to_string().into_bytes();
    assert!(matches!(
        tuf::update(&forged, &honest, NOW as u64),
        Err(TufError::Threshold(_))
    ));
    // The untouched chain verifies, so the refusal is the signature count.
    tuf::update(&repo.metadata(), &honest, NOW as u64).unwrap();
}

/// TUF checks only the *final* root's expiry (client workflow §5.3.11) — the
/// real Sigstore chain relies on it: root 14 was already expired when 15 was published.
#[test]
fn an_expired_root_is_refused_but_an_expired_intermediate_is_not() {
    let (mut repo, _log) = repo();
    let state = repo.embedded_state();
    repo.root_expires = NOW - 1;
    repo.rotate_root(false);
    let mut metadata = repo.metadata();
    assert!(matches!(
        tuf::update(&metadata, &state, NOW as u64),
        Err(TufError::Expiry(_))
    ));

    // Re-publish the head unexpired, leaving the intermediates as they are.
    repo.root_expires = NOW + 86_400;
    repo.rotate_root(false);
    metadata = repo.metadata();
    let update = tuf::update(&metadata, &state, NOW as u64).expect("only the head's expiry gates");
    assert_eq!(update.state.root_version, 3);
}

/// A mirror still serving the old chain after the repository moved on: valid material, refused.
#[test]
fn a_version_rollback_is_refused() {
    let (mut repo, _log) = repo();
    let first = accept(&repo).state;
    let stale = repo.metadata();
    repo.set_tlogs(&[&SimLog::new("rekor.sim")]);
    let newer = accept(&repo).state;
    assert!(newer.timestamp_version > first.timestamp_version);
    let error = tuf::update(&stale, &newer, NOW as u64);
    assert!(matches!(error, Err(TufError::Rollback(_))), "{error:?}");
}

/// The target's hash is bound by the snapshot, its length by the timestamp — tampering is caught at the covering level.
#[test]
fn a_tampered_target_is_refused() {
    let (repo, _log) = repo();
    let mut metadata = repo.metadata();
    metadata.trusted_root.push(b' ');
    let error = tuf::update(&metadata, &repo.embedded_state(), NOW as u64);
    assert!(matches!(error, Err(TufError::TargetHash(_))), "{error:?}");

    let mut metadata = repo.metadata();
    metadata.snapshot.push(b' ');
    // Trailing whitespace leaves the canonical form intact; the length check catches it.
    assert!(matches!(
        tuf::update(&metadata, &repo.embedded_state(), NOW as u64),
        Err(TufError::TargetHash(_))
    ));
}

/// Revocation, §10's other half: the pin set *replaces*, never unions, and an
/// empty pin set is never adopted — it would refuse every zone from then on.
#[test]
fn a_trusted_root_that_drops_a_shard_drops_it_from_the_pin_set() {
    let old = SimLog::new("rekor.old");
    let new = SimLog::new("rekor.new");
    let mut repo = SimTuf::new(NOW, &[&old, &new]);
    let both = accept(&repo);
    assert!(both.log_keys.find(&old.log_id()).is_some());
    assert!(both.log_keys.find(&new.log_id()).is_some());

    repo.set_tlogs(&[&new]);
    let after = tuf::update(&repo.metadata(), &both.state, NOW as u64).unwrap();
    assert_eq!(after.log_keys.keys().len(), 1);
    assert!(
        after.log_keys.find(&old.log_id()).is_none(),
        "a log Sigstore removed must leave the pin set"
    );
    assert!(after.log_keys.find(&new.log_id()).is_some());

    let mut empty = SimTuf::new(NOW, &[]);
    let error = tuf::update(&empty.metadata(), &empty.embedded_state(), NOW as u64);
    assert!(matches!(error, Err(TufError::Malformed(_))), "{error:?}");
    empty.set_tlogs(&[&SimLog::new("rekor.sim")]);
    tuf::update(&empty.metadata(), &empty.embedded_state(), NOW as u64).unwrap();
}

// -------------------------------------------------------- the pin-file clock

/// `updated_at` floors the clock the TUF expiry checks see, so a signed
/// comparison turning `u64::MAX` into `-1` would adopt ten-year-old hostile material.
#[test]
fn no_clock_value_makes_a_tuf_expiry_check_vacuous() {
    let repo = SimTuf::new(NOW, &[&SimLog::new("rekor.honest")]);
    let metadata = repo.metadata();
    let state = repo.embedded_state();
    tuf::update(&metadata, &state, NOW as u64).expect("fresh material verifies");
    for at in [NOW as u64 + 10 * 365 * 86_400, u64::MAX] {
        let refused = tuf::update(&metadata, &state, at);
        assert!(
            matches!(refused, Err(TufError::Expiry(_))),
            "material ten years expired must not be adopted at {at}: {refused:?}"
        );
    }
}

// --------------------------------------------------------------- resolver

/// A zone whose proof comes from a log the bootstrap pin set has never heard of, and the TUF repository that teaches it.
fn zone_learning_a_new_shard() -> (SimZone, SimLog, SimTuf, tempfile::NamedTempFile) {
    let mut zone = SimZone::new("cluster.example", common::member_records());
    let mut new_shard = SimLog::new("log2099-1.rekor.sim");
    let proof = new_shard.publish(&zone, "create");
    zone.rekor_txt = proof.to_txt().expect("encodes");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let repo = SimTuf::new(now, &[&new_shard]);
    // The root this client embeds — a file only so the test can seed a PinState from it.
    let embedded = common::write(&String::from_utf8(repo.embedded_root()).unwrap());
    (zone, new_shard, repo, embedded)
}

/// A repository that answers, and answers nonsense: every check `update` makes refuses it.
struct Garbage;

impl tuf::Repo for Garbage {
    fn get(&self, path: &str) -> Result<Option<Vec<u8>>, String> {
        // Whichever root a client trusts, this repository has one — the refusal is about the content.
        let junk = path.ends_with(".root.json") || path == "timestamp.json";
        Ok(junk.then(|| b"this is not TUF metadata".to_vec()))
    }
}

/// A repository that counts walks, so "walked once" and "walked every lookup" are distinguishable.
struct Counting {
    inner: SimTuf,
    walks: std::sync::atomic::AtomicUsize,
}

impl Counting {
    fn new(inner: SimTuf) -> Counting {
        Counting {
            inner,
            walks: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn walks(&self) -> usize {
        self.walks.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl tuf::Repo for Counting {
    fn get(&self, path: &str) -> Result<Option<Vec<u8>>, String> {
        // `timestamp.json` is fetched exactly once per walk — counting it counts walks.
        if path == "timestamp.json" {
            self.walks
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        self.inner.get(path)
    }
}

/// The options a refreshing client runs on: no static key file, pin refresh on, TUF repo injected.
fn refreshing(
    url: String,
    anchor: &std::path::Path,
    state_path: Option<std::path::PathBuf>,
    tuf_root: Option<&std::path::Path>,
) -> ResolverOptions {
    ResolverOptions {
        doh_url: Some(url),
        trust_anchor: Some(anchor.to_path_buf()),
        rekor: Some(RekorPolicy::Require),
        // No --rekor-key: the pin set starts as the embedded Sigstore bootstrap.
        rekor_key: None,
        rekor_state: state_path,
        rekor_config: None,
        tuf_url: None,
        no_tuf: false,
        // The harness mints its own TUF root; that is the anchor every persisted pin state records itself against.
        tuf_root: tuf_root.map(std::path::Path::to_path_buf),
    }
}

/// A zone whose membership key is logged, with an attach record naming `url`.
fn logged_zone_with_cp(url: &str) -> (SimZone, SimLog) {
    let mut zone = SimZone::new("cluster.example", common::member_records());
    let mut log = SimLog::new("rekor.sim");
    zone.rekor_txt = log.publish(&zone, "create").to_txt().expect("encodes");
    zone.cp_txt = vec![format!("v=synccp1 url={url}")];
    (zone, log)
}

/// A resolver pinned to `log`'s static key, serving `zone` over plaintext DoH.
async fn static_resolver(
    zone: SimZone,
    log: &SimLog,
) -> (DnssecResolver, tokio::task::JoinHandle<()>) {
    let anchor = common::write(&zone.anchor_record());
    let log_key = common::write(&log.key_pem());
    let (url, server) = zone.serve().await;
    let resolver = DnssecResolver::with_options(&ResolverOptions {
        rekor_key: Some(log_key.path().to_path_buf()),
        ..refreshing(url, anchor.path(), None, None)
    })
    .unwrap();
    (resolver, server)
}

/// Seeds a state file at root version 1 — what "a build that embeds this root" means to the resolver.
fn seed_state(path: &std::path::Path, embedded_root: &std::path::Path) {
    let root = std::fs::read(embedded_root).unwrap();
    let state = PinState {
        root_version: 1,
        ..PinState::anchored(&root)
    };
    state.save(path).unwrap();
}

/// The §10.2 flagship: a TUF walk teaches an unknown shard and verifies its proof in the same refresh.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_walked_chain_teaches_a_log_the_build_never_knew() {
    let (zone, new_shard, repo, embedded) = zone_learning_a_new_shard();
    let anchor = common::write(&zone.anchor_record());
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("rekor-pins.json");
    seed_state(&state_path, embedded.path());
    let (url, server) = zone.serve().await;

    let counting = Arc::new(Counting::new(repo));
    let resolver = DnssecResolver::with_options(&refreshing(
        url,
        anchor.path(),
        Some(state_path.clone()),
        Some(embedded.path()),
    ))
    .unwrap()
    .with_tuf_repo(counting.clone());
    assert!(
        resolver.log_keys().find(&new_shard.log_id()).is_none(),
        "the build must not already know this log, or the test proves nothing"
    );

    let (set, _ttl) = resolver
        .member_set("cluster.example")
        .await
        .expect("the TUF update and the proof verification run in one refresh");
    assert_eq!(set.bindings.len(), 1);
    assert!(resolver.log_keys().find(&new_shard.log_id()).is_some());

    // And it was persisted, so the next process starts where this one ended.
    let persisted = PinState::load_anchored(&state_path, &std::fs::read(embedded.path()).unwrap())
        .expect("the pin state is written and loads under its anchor");
    assert_eq!(persisted.root_version, 1);
    assert!(persisted.timestamp_version >= 1);
    assert!(rekor::LogKeys::default() != persisted.log_keys().unwrap());

    // Membership may re-resolve on a one-minute TTL; the repository still hears from this client once a day (§10.2).
    assert_eq!(counting.walks(), 1);
    resolver.member_set("cluster.example").await.unwrap();
    assert!(resolver.refresh_tuf().await.unwrap().is_none());
    assert_eq!(counting.walks(), 1, "a second lookup must not walk again");
    server.abort();
}

/// The control for the flagship: everything identical except `--no-tuf`, so the
/// log stays unknown, the refusal is the doctor-facing class, and nothing is walked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_same_zone_without_the_walk_fails_with_unknown_log() {
    let (zone, _shard, repo, embedded) = zone_learning_a_new_shard();
    let anchor = common::write(&zone.anchor_record());
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("rekor-pins.json");
    seed_state(&state_path, embedded.path());
    let (url, server) = zone.serve().await;

    let counting = Arc::new(Counting::new(repo));
    let resolver = DnssecResolver::with_options(&ResolverOptions {
        no_tuf: true,
        tuf_root: None,
        ..refreshing(url, anchor.path(), Some(state_path), Some(embedded.path()))
    })
    .unwrap()
    .with_tuf_repo(counting.clone());
    let error = resolver.member_set("cluster.example").await.unwrap_err();
    assert!(
        matches!(error, NetError::RekorUnknownLog { .. }),
        "without the walk the log stays unknown: {error}"
    );
    assert_eq!(counting.walks(), 0, "`--no-tuf` never walks the repository");
    server.abort();
}

/// §10.2's load-bearing rule: TUF trouble is never worse than not having asked —
/// the refresh succeeds, the pins do not move, nothing is persisted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_repository_serving_nonsense_never_fails_a_refresh() {
    let mut zone = SimZone::new("cluster.example", common::member_records());
    let mut log = SimLog::new("rekor.sim");
    zone.rekor_txt = log.publish(&zone, "create").to_txt().expect("encodes");
    let anchor = common::write(&zone.anchor_record());
    let log_key = common::write(&log.key_pem());
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("rekor-pins.json");
    let (url, server) = zone.serve().await;

    // A static --rekor-key disables TUF refresh entirely: no walk, no error, no file.
    let stat = DnssecResolver::with_options(&ResolverOptions {
        rekor_key: Some(log_key.path().to_path_buf()),
        ..refreshing(url.clone(), anchor.path(), Some(state_path.clone()), None)
    })
    .unwrap()
    .with_tuf_repo(Arc::new(Garbage));
    let (set, _ttl) = stat
        .member_set("cluster.example")
        .await
        .expect("a broken repository must not fail a refresh");
    assert_eq!(set.bindings.len(), 1);
    assert!(stat.refresh_tuf().await.unwrap().is_none());
    assert!(
        !state_path.exists(),
        "an explicit key file disables TUF refresh entirely"
    );

    // The refreshable variant says *which* way the chain broke — the class `synch doctor` reads.
    let client = DnssecResolver::with_options(&refreshing(
        url,
        anchor.path(),
        Some(state_path.clone()),
        None,
    ))
    .unwrap()
    .with_tuf_repo(Arc::new(Garbage));
    let before = client.log_keys();
    let error = client.refresh_tuf().await.unwrap_err();
    assert!(
        matches!(&error, NetError::Tuf { class, .. } if *class == "malformed"),
        "{error}"
    );
    assert_eq!(client.log_keys(), before, "the pins did not move");
    assert!(!state_path.exists(), "and nothing was persisted");
    server.abort();
}

/// The cloud-attach gate: under `RekorPolicy::Require`, a DNSSEC-valid
#[tokio::test]
async fn discovery_refuses_an_unlogged_zone_under_require() {
    let mut zone = SimZone::new("cluster.example", common::member_records());
    zone.cp_txt = vec!["v=synccp1 url=https://attacker.example".to_string()];
    let log = SimLog::new("rekor.sim");
    let (resolver, server) = static_resolver(zone, &log).await;
    let error = resolver
        .control_plane("cluster.example")
        .await
        .expect_err("an unlogged zone key must yield no endpoint under Require");
    // The sim serves an unsigned NODATA for the absent proof, so hickory
    // reports a bogus negative; a real control plane surfaces `RekorAbsent`.
    // Either way the endpoint is refused, which is what matters.
    assert!(
        matches!(&error, NetError::RekorAbsent { .. })
            || matches!(&error, NetError::Dns(msg) if msg.contains(rekor::REKOR_TXT_PREFIX)),
        "the membership answer's zone key must be gated before the attach record is trusted: {error}"
    );
    server.abort();

    // With the gate off, the same unlogged zone discovers its endpoint.
    let mut zone = SimZone::new("cluster.example", common::member_records());
    zone.cp_txt = vec!["v=synccp1 url=https://sync.example".to_string()];
    let anchor = common::write(&zone.anchor_record());
    let (url, server) = zone.serve().await;
    let off = DnssecResolver::with_options(&ResolverOptions {
        doh_url: Some(url),
        trust_anchor: Some(anchor.path().to_path_buf()),
        rekor: Some(RekorPolicy::Off),
        rekor_key: None,
        rekor_state: None,
        rekor_config: None,
        tuf_url: None,
        no_tuf: true,
        tuf_root: None,
    })
    .unwrap();
    let (records, _ttl) = off
        .control_plane("cluster.example")
        .await
        .expect("with the gate off, a DNSSEC-valid answer is enough");
    assert_eq!(urls(&records), ["https://sync.example"]);
    server.abort();
}

/// The positive control: the same zone, once its key is logged, discovers its
/// endpoint — without this the refusal above could be the harness failing to serve.
#[tokio::test]
async fn discovery_yields_the_endpoint_once_the_key_is_logged() {
    let (zone, log) = logged_zone_with_cp("https://sync.example");
    let (resolver, server) = static_resolver(zone, &log).await;
    let (records, _ttl) = resolver
        .control_plane("cluster.example")
        .await
        .expect("a logged zone key discovers its endpoint");
    assert_eq!(urls(&records), ["https://sync.example"]);
    server.abort();
}

/// The attach record does not get to name the apex it is judged against:
#[tokio::test]
async fn an_attach_record_cannot_name_the_apex_it_is_gated_against() {
    let mut zone = SimZone::new("cluster.example", common::member_records());
    let mut log = SimLog::new("rekor.sim");
    zone.rekor_txt = log.publish(&zone, "create").to_txt().expect("encodes");
    zone.cp_txt = vec!["v=synccp1 url=https://attacker.example apex=cluster.example".to_string()];
    let (evil, evil_signer) = zone.second_key();
    zone.add_dnskey(evil);
    zone.sign_cp_with(evil_signer);

    let (resolver, server) = static_resolver(zone, &log).await;
    let error = resolver
        .control_plane("cluster.example")
        .await
        .expect_err("the record under audit must not choose its own bound");
    assert!(
        matches!(&error, NetError::RekorBinding { .. }),
        "the gate must run against the membership apex: {error}"
    );
    server.abort();
}

#[tokio::test]
async fn discovery_refuses_an_attach_record_signed_by_an_unlogged_key() {
    let mut zone = SimZone::new("cluster.example", common::member_records());
    let mut log = SimLog::new("rekor.sim");
    zone.rekor_txt = log.publish(&zone, "create").to_txt().expect("encodes");
    zone.cp_txt = vec!["v=synccp1 url=https://attacker.example".to_string()];
    let (evil, evil_signer) = zone.second_key();
    zone.add_dnskey(evil);
    zone.sign_cp_with(evil_signer);

    let (resolver, server) = static_resolver(zone, &log).await;
    let error = resolver
        .control_plane("cluster.example")
        .await
        .expect_err("an attach record signed by an unlogged key must yield no endpoint");
    // The membership key really is logged, so the refusal is the second gate
    // on the attach answer's own signer, landing as a binding failure.
    assert!(
        matches!(&error, NetError::RekorBinding { .. }),
        "the attach record's own signer must be gated: {error}"
    );
    server.abort();
}

#[tokio::test]
async fn discovery_accepts_an_attach_record_signed_by_the_logged_key() {
    let (mut zone, log) = logged_zone_with_cp("https://sync.example");
    let (spare, _spare_signer) = zone.second_key();
    zone.add_dnskey(spare);

    let (resolver, server) = static_resolver(zone, &log).await;
    let (records, _ttl) = resolver
        .control_plane("cluster.example")
        .await
        .expect("an attach record signed by the logged key discovers its endpoint");
    assert_eq!(urls(&records), ["https://sync.example"]);
    server.abort();
}

/// A control plane is a fleet, so the apex names every node of it and every
/// name is returned — one tunnel each, because the registry of attached
/// daemons is one node's memory and a node nobody attached to answers
/// nothing.
///
/// Order is the zone's data and not the wire's: a caller that opens one
/// tunnel per endpoint must not open a different *set* each refresh because
/// an RRset arrived shuffled.
#[tokio::test]
async fn discovery_yields_every_endpoint_the_apex_names() {
    let mut zone = SimZone::new("cluster.example", common::member_records());
    let mut log = SimLog::new("rekor.sim");
    zone.rekor_txt = log.publish(&zone, "create").to_txt().expect("encodes");
    zone.cp_txt = vec![
        "v=synccp1 url=https://ns2.sync.example".to_string(),
        "v=synccp1 url=https://sync.example".to_string(),
        "v=synccp1 url=https://ns1.sync.example".to_string(),
    ];
    let (resolver, server) = static_resolver(zone, &log).await;
    let (records, _ttl) = resolver
        .control_plane("cluster.example")
        .await
        .expect("every named node is an endpoint");
    assert_eq!(
        urls(&records),
        [
            "https://ns1.sync.example",
            "https://ns2.sync.example",
            "https://sync.example"
        ]
    );
    server.abort();
}

/// One unreadable record must not sink the readable ones beside it — a
/// control plane mid-upgrade can leave an old-format record in the RRset —
/// and a set with nothing usable in it is still a refusal.
#[tokio::test]
async fn an_unreadable_record_does_not_sink_the_fleet() {
    let mut zone = SimZone::new("cluster.example", common::member_records());
    let mut log = SimLog::new("rekor.sim");
    zone.rekor_txt = log.publish(&zone, "create").to_txt().expect("encodes");
    zone.cp_txt = vec![
        "v=synccp0 url=https://old.sync.example".to_string(),
        "v=synccp1 url=https://sync.example".to_string(),
        // Duplicated at the same owner name: an RRset is a set, and one
        // tunnel is what a repeated endpoint deserves.
        "v=synccp1 url=https://sync.example".to_string(),
    ];
    let (resolver, server) = static_resolver(zone, &log).await;
    let (records, _ttl) = resolver
        .control_plane("cluster.example")
        .await
        .expect("a readable record survives an unreadable neighbour");
    assert_eq!(urls(&records), ["https://sync.example"]);
    server.abort();
}

/// The urls of a discovered fleet, for assertions.
fn urls(records: &[synch_net::dns::ControlPlaneRecord]) -> Vec<&str> {
    records.iter().map(|r| r.url.as_str()).collect()
}

/// A pin file naming a root this build never signed is not state: the *binary*
/// decides the anchor, so one writable file cannot choose the log keys outright.
#[test]
fn a_pin_file_anchored_somewhere_else_does_not_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rekor-pins.json");

    // An attacker's repository: their own keys, bumped past the honest version so a version comparison prefers it.
    let (mine, _log) = repo();
    let (mut theirs, _their_log) = repo();
    theirs.rotate_root(true);
    theirs.rotate_root(true);
    theirs.set_tlogs(&[&SimLog::new("rekor.attacker")]);
    let forged = accept(&theirs);
    assert!(
        forged.state.root_version > 1,
        "the forged state must look newer, or the test proves nothing"
    );
    forged.state.save(&path).unwrap();
    assert!(
        PinState::load_anchored(&path, &mine.embedded_root()).is_none(),
        "a root this build's anchor never signed must not load"
    );
    // Not the *versions* being refused: the forged state loads for the universe that minted it.
    assert!(
        PinState::load_anchored(&path, &theirs.embedded_root()).is_some(),
        "the forged state is well-formed; it is simply not ours"
    );
}

// -------------------------------------------------------------- fixtures

/// Refetches the conformance fixture from the live Sigstore repository, for
/// when the real chain rotates past what is checked in. Reaches the network
/// and rewrites the checked-in bytes, so it is gated by
/// `SYNCH_TUF_FIXTURE=write` and run with `--ignored`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn regenerate_the_shared_fixture() {
    assert_eq!(
        std::env::var("SYNCH_TUF_FIXTURE").ok().as_deref(),
        Some("write"),
        "refusing to rewrite the fixture without SYNCH_TUF_FIXTURE=write"
    );
    let base = std::env::var("CP_TUF_URL")
        .unwrap_or_else(|_| "https://tuf-repo-cdn.sigstore.dev".to_string());
    let client = reqwest::Client::new();
    let get = |path: String| {
        let client = client.clone();
        let url = format!("{base}/{path}");
        async move {
            let response = client.get(&url).send().await.expect("fetch");
            match response.status().is_success() {
                true => Some(response.bytes().await.expect("body").to_vec()),
                false => None,
            }
        }
    };
    let json = |bytes: &[u8]| -> serde_json::Value { serde_json::from_slice(bytes).expect("json") };

    // The root chain: walk versions upward until the repository has no more.
    let mut roots: Vec<(u64, Vec<u8>)> = Vec::new();
    for version in 1..1000u64 {
        match get(format!("{version}.root.json")).await {
            Some(bytes) => roots.push((version, bytes)),
            None => break,
        }
    }
    let head = roots.last().expect("at least one root").0;
    // Consistent snapshots: timestamp names the snapshot version, the
    // snapshot names the targets version, the targets name the target hash.
    let timestamp = get("timestamp.json".into()).await.expect("timestamp.json");
    let timestamp_version = json(&timestamp)["signed"]["version"]
        .as_u64()
        .expect("version");
    let snapshot_version = json(&timestamp)["signed"]["meta"]["snapshot.json"]["version"]
        .as_u64()
        .expect("the snapshot version");
    let snapshot = get(format!("{snapshot_version}.snapshot.json"))
        .await
        .expect("snapshot");
    let targets_version = json(&snapshot)["signed"]["meta"]["targets.json"]["version"]
        .as_u64()
        .expect("the targets version");
    let targets = get(format!("{targets_version}.targets.json"))
        .await
        .expect("targets");
    let digest = json(&targets)["signed"]["targets"][tuf::TRUSTED_ROOT_TARGET]["hashes"]["sha256"]
        .as_str()
        .expect("the trusted root's digest")
        .to_string();
    let trusted_root = get(format!("targets/{digest}.{}", tuf::TRUSTED_ROOT_TARGET))
        .await
        .expect("trusted root");

    // Three roots is enough chain to be a chain: the head and the two it
    // was rotated from, so the walk has real rotations to walk.
    let floor = head.saturating_sub(2).max(1);
    let chain: Vec<(u64, Vec<u8>)> = roots.into_iter().filter(|(v, _)| *v >= floor).collect();
    let walked = TufMetadata {
        roots: chain.iter().map(|(_, bytes)| bytes.clone()).collect(),
        timestamp: timestamp.clone(),
        snapshot: snapshot.clone(),
        targets: targets.clone(),
        trusted_root: trusted_root.clone(),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let state = PinState {
        root_version: floor,
        ..PinState::anchored(&chain[0].1)
    };
    let update = tuf::update(&walked, &state, now).expect("the fetched chain must verify");

    let dir = fixture_dir();
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        std::fs::remove_file(entry.path()).unwrap();
    }
    let put = |name: &str, bytes: &[u8]| std::fs::write(dir.join(name), bytes).unwrap();
    for (version, bytes) in &chain {
        put(&format!("root-{version}.json"), bytes);
    }
    put("timestamp.json", &timestamp);
    put("snapshot.json", &snapshot);
    put("targets.json", &targets);
    put("trusted-root.json", &trusted_root);
    let log_ids: Vec<String> = update
        .log_keys
        .keys()
        .iter()
        .map(|key| hex::encode(key.id))
        .collect();
    put(
        "meta.txt",
        format!(
            "source={base}\n\
             fetched_at={now}\n\
             verify_at={now}\n\
             root_versions={}\n\
             chain_floor={floor}\n\
             root_version={head}\n\
             timestamp_version={timestamp_version}\n\
             snapshot_version={snapshot_version}\n\
             targets_version={targets_version}\n\
             trusted_root_sha256={digest}\n\
             log_ids={}\n",
            chain
                .iter()
                .map(|(v, _)| v.to_string())
                .collect::<Vec<_>>()
                .join(","),
            log_ids.join(","),
        )
        .as_bytes(),
    );
    // The build embeds the head of the chain it was regenerated against.
    std::fs::write(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/sigstore_tuf_root.json"),
        &chain.last().expect("a head root").1,
    )
    .unwrap();
}

fn fixture_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tuf")
}
