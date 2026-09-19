//! Fixture-maintenance tooling for the Rekor zone-key suites — tooling, not
//! tests (they were `#[ignore]`d tests until the runbook outgrew the suite).
//!
//! Two operations, each refusing to run without its own explicit opt-in:
//!
//! - regenerate: rewrites the shared Gleam/Rust fixture under
//!   `control-plane/test/fixtures/rekor`. A zone key and a log key are minted
//!   here, so running it invalidates every byte downstream of them; it exists
//!   so the fixture can be regenerated when the format changes, deliberately
//!   and all at once.
//!
//!   `SYNCH_REKOR_FIXTURE=write cargo run -p synch-net --example regen_fixture -- regenerate`
//!
//! - publish: mints a fresh entry on the **real** Sigstore log and rewrites
//!   `tests/fixtures/rekor_v3` from what comes back. This writes to a
//!   permanent, public, append-only log; nothing about it can be undone. Run
//!   it only when the entry format changes in a way that invalidates the
//!   checked-in bytes. The apex stays under `.invalid` so that no real name
//!   is ever claimed in a log nobody can edit, and the chain is self-anchored
//!   for the same reason — we own no DNSSEC-signed domain, and minting a
//!   certificate naming somebody else's would be squatting. Afterwards the
//!   index and origin asserted in `tests/rekor_zone_key.rs` and in
//!   `crates/synch-monitor/tests/real_entry.rs` move with it, and
//!   PROVENANCE.txt is rewritten — including the log it names, which this run
//!   discovers rather than assumes.
//!
//!   `SYNCH_REKOR_PUBLISH=yes-write-to-the-real-log cargo run -p synch-net
//!   --example regen_fixture -- publish`

use synch_net::{
    chain,
    rekor::{self, HashedRekordBody, LogKeys, ProofError, RekorProof, ZoneKey},
    sim::{SimLog, SimZone},
    tuf,
    zonecert::OID_DNSSEC_CHAIN,
};

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("regenerate") => regenerate_the_shared_fixture(),
        Some("publish") => tokio::runtime::Runtime::new()
            .expect("a tokio runtime")
            .block_on(publish_a_real_entry()),
        _ => {
            eprintln!("usage: regen_fixture <regenerate|publish>");
            std::process::exit(2);
        }
    }
}

fn member_records() -> Vec<String> {
    vec![format!(
        "v=sync1 id=nas nk={} apex=cluster.example",
        iroh_base::SecretKey::generate().public().to_z32()
    )]
}

/// The trust anchors a client resolving a simulated zone holds: that zone's
/// own key, exactly as `--dnssec-anchor` would install it.
fn anchors(zone: &SimZone) -> hickory_resolver::proto::dnssec::TrustAnchors {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), zone.anchor_record()).unwrap();
    hickory_resolver::proto::dnssec::TrustAnchors::from_file(file.path()).unwrap()
}

/// The chain bytes a certificate carries, decoded only after
/// [`chain::authorize`] has accepted them.
fn carried_chain(
    body: &HashedRekordBody,
    anchors: &hickory_resolver::proto::dnssec::TrustAnchors,
) -> synch_net::zonecert::DnssecChain {
    chain::authorize(&body.certificate, anchors).expect("the carried chain must authorize");
    let value = body
        .certificate
        .extension(OID_DNSSEC_CHAIN)
        .expect("the certificate carries a chain");
    synch_net::zonecert::DnssecChain::decode(value).expect("the chain decodes")
}

fn verify(proof: &RekorProof, zone: &SimZone, log: &SimLog) -> Result<(), ProofError> {
    let apex = zone.apex();
    let rdata = zone.dnskey_rdata();
    let key = ZoneKey {
        domain: &apex,
        signing_zone: &apex,
        key_tag: zone.key_tag(),
        dnskey_rdata: &rdata,
    };
    rekor::verify(
        proof,
        &key,
        &LogKeys::parse(&log.key_pem()).unwrap(),
        &anchors(zone),
    )
    .map(|_| ())
}

/// Standard-alphabet base64, as protojson renders every `bytes` field.
fn b64(text: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .expect("protojson bytes are base64")
}

fn regenerate_the_shared_fixture() {
    assert_eq!(
        std::env::var("SYNCH_REKOR_FIXTURE").ok().as_deref(),
        Some("write"),
        "refusing to rewrite the fixture without SYNCH_REKOR_FIXTURE=write"
    );
    let zone = SimZone::new("sync.test", member_records());
    let mut log = SimLog::new("rekor.sim");
    // Neighbours, so the audit path is a real path and not an empty list.
    log.append(b"an entry logged before this zone's key");
    let statement = zone.zone_key_statement("create");
    let mut proof = log.log_statement(&zone, &statement);
    log.append(b"an entry logged after it");
    log.refresh(&mut proof);
    verify(&proof, &zone, &log).expect("the fixture must verify before it is written");
    let body = HashedRekordBody::parse(&proof.canonicalized_body).unwrap();

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../control-plane/test/fixtures/rekor");
    std::fs::create_dir_all(&dir).unwrap();
    let write_file = |name: &str, bytes: &[u8]| std::fs::write(dir.join(name), bytes).unwrap();
    write_file("proof.bin", &proof.encode().expect("the fixture encodes"));
    write_file("statement.json", &proof.statement);
    write_file("canonicalized-body.bin", &proof.canonicalized_body);
    write_file("certificate.der", &body.certificate_der);
    // The chain the certificate actually carries, not a fresh one: RRSIG
    // signing is randomized, so re-deriving it would write bytes the entry
    // does not contain.
    write_file(
        "dnssec-chain.der",
        &carried_chain(&body, &anchors(&zone)).encode(),
    );
    write_file("checkpoint.txt", &proof.checkpoint);
    write_file("log-id.bin", &proof.log_id);
    write_file("inclusion-path.bin", &proof.inclusion_path.concat());
    write_file("dnskey.bin", &zone.dnskey_rdata());
    write_file("log-key.pem", log.key_pem().as_bytes());
    write_file("anchor.key", zone.anchor_record().as_bytes());
    write_file(
        "meta.txt",
        format!(
            // `max_proof_parts` is not about this entry. It is one number
            // that has to be the same on both sides — the publisher refuses
            // to emit more parts than it, the client stops reading at it —
            // and until it landed here each suite asserted its own constant
            // against itself, which passes however far apart the two drift.
            "apex={}\nkey_tag={}\nlog_index={}\naction={}\nmax_proof_parts={}\n",
            zone.apex(),
            zone.key_tag(),
            proof.log_index,
            statement.action,
            synch_net::rekor::MAX_PROOF_PARTS,
        )
        .as_bytes(),
    );
    println!("rewrote {}", dir.display());
}

async fn publish_a_real_entry() {
    assert_eq!(
        std::env::var("SYNCH_REKOR_PUBLISH").ok().as_deref(),
        Some("yes-write-to-the-real-log"),
        "refusing to write to a permanent public log without being asked"
    );
    // Which shard to write to is discovered, not pinned here: a fixture
    // regeneration run after Sigstore rotates must reach the log that is
    // actually open, not POST into a closed one and report the 4xx as a
    // format problem. The log id follows from the same entry's key, which is
    // also the pin the verification below will select on.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_secs();
    let tlogs = tuf::tlogs(tuf::EMBEDDED_TRUSTED_ROOT.as_bytes())
        .expect("the embedded trusted root lists its logs");
    let open = tuf::current_tlog(&tlogs, now).expect("Sigstore has a shard open");
    let log = open.base_url.clone();
    let log_id = open.log_id;
    println!("publishing to {log}");

    let zone = SimZone::new("zone-key-transparency.demo.invalid", member_records());
    let statement = zone.zone_key_statement("rollover");
    let statement_json = statement.to_json();
    let signature = zone.sign_dsse(&statement_json);
    let certificate = zone.zone_key_certificate();
    let digest = rekor::sha256(&rekor::pae(rekor::DSSE_PAYLOAD_TYPE, &statement_json));

    // The protojson `CreateEntryRequest` the control plane sends, in the one
    // shape Rekor v2 takes.
    let request = format!(
        "{{\"hashedRekordRequestV002\":{{\"digest\":\"{}\",\"signature\":\
         {{\"content\":\"{}\",\"verifier\":{{\"x509Certificate\":\
         {{\"rawBytes\":\"{}\"}},\"keyDetails\":\"PKIX_ECDSA_P256_SHA_256\"}}}}}}}}",
        rekor::base64_encode(&digest),
        rekor::base64_encode(&signature),
        rekor::base64_encode(&certificate),
    );
    let http = reqwest::Client::builder()
        .user_agent("synch-net fixture publisher")
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .expect("http client");
    let response = http
        .post(format!("{log}/api/v2/log/entries"))
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .body(request)
        .send()
        .await
        .expect("the log must be reachable");
    let status = response.status();
    let text = response.text().await.expect("a response body");
    assert!(
        status.as_u16() == 200 || status.as_u16() == 201,
        "the log answered {status}: {text}"
    );
    let entry: serde_json::Value = serde_json::from_str(&text).expect("protojson");

    // protojson renders 64-bit integers as decimal strings.
    let log_index: u64 = entry["logIndex"]
        .as_str()
        .expect("logIndex")
        .parse()
        .expect("a decimal index");
    let canonicalized_body = b64(entry["canonicalizedBody"]
        .as_str()
        .expect("canonicalizedBody"));
    let checkpoint = entry["inclusionProof"]["checkpoint"]["envelope"]
        .as_str()
        .expect("checkpoint envelope")
        .as_bytes()
        .to_vec();
    let inclusion_path: Vec<[u8; 32]> = entry["inclusionProof"]["hashes"]
        .as_array()
        .expect("hashes")
        .iter()
        .map(|hash| {
            b64(hash.as_str().expect("a hash"))
                .try_into()
                .expect("32 bytes")
        })
        .collect();

    let proof = RekorProof {
        log_id,
        log_index,
        statement: statement_json.clone(),
        canonicalized_body,
        checkpoint,
        inclusion_path,
    };

    // Verified end to end before a byte is written: the entry the log stored
    // has to be one this build's own client accepts, or the fixture would
    // pin a shape nothing can read.
    let key = ZoneKey {
        domain: &zone.apex(),
        signing_zone: &zone.apex(),
        key_tag: zone.key_tag(),
        dnskey_rdata: &zone.dnskey_rdata(),
    };
    let verified = rekor::verify(&proof, &key, &LogKeys::embedded(), &anchors(&zone))
        .expect("the published entry must verify under the real log's key");
    assert_eq!(verified.log_index, log_index);
    let body = HashedRekordBody::parse(&proof.canonicalized_body).unwrap();

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rekor_v3");
    std::fs::create_dir_all(&dir).unwrap();
    let write_file = |name: &str, bytes: &[u8]| std::fs::write(dir.join(name), bytes).unwrap();
    write_file("statement.json", &proof.statement);
    write_file("canonicalized_body.json", &proof.canonicalized_body);
    write_file("certificate.der", &body.certificate_der);
    write_file("checkpoint.txt", &proof.checkpoint);
    write_file("proof.bin", &proof.encode().expect("the record encodes"));
    write_file("anchor.txt", zone.anchor_record().as_bytes());
    write_file("log_index.txt", format!("{log_index}\n").as_bytes());

    println!("published logIndex {log_index}");
    println!("apex     {}", zone.apex());
    println!("key tag  {}", zone.key_tag());
    println!("cert     {} bytes", body.certificate_der.len());
    println!("proof    {} bytes", proof.encode().unwrap().len());
    println!("DS       {}", zone.ds_field());
}
