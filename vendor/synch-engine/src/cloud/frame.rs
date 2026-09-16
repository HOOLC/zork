//! The tunnel wire format: JSON control frames, binary content frames.
//!
//! Two shapes cross one WebSocket, multiplexed by request id. Control frames
//! are JSON text, tagged by `t`; content travels in binary frames behind an
//! eight-byte header, so file bytes are never JSON-encoded.
//!
//! **There is no frame that writes.** Nothing here encodes a put, an adopt, a
//! pin or a config append, so a control plane holding one end of this tunnel
//! cannot push bytes at a cluster whatever it sends — the read-only property
//! is a fact about this file, not a check somewhere else. A control plane that
//! wants a delegation *changed* has no frame for it and never will: delegating
//! is publishing a `d:` record under an origin's own key, which only that
//! origin can sign (§3.5).

use serde::{Deserialize, Serialize};

/// The newest tunnel protocol version this daemon speaks, carried in the hello.
///
/// v2 added the delegations query, v3 the replication one, and v4 live space
/// claims. Each is additive on the wire, and the number exists because
/// additive is not the same as safe:
/// a frame an end has not learnt fails to decode, and a failed decode ends the
/// connection rather than answering. The version is how the control plane
/// knows which questions this daemon can be asked at all.
pub(crate) const PROTOCOL_VERSION: u32 = 4;

/// The first version in which a daemon may refresh its routing claim without
/// reconnecting.
pub(crate) const SPACE_UPDATES_VERSION: u32 = 4;

/// The oldest settled version this daemon will serve under.
///
/// The control plane settles on the daemon's version when it can, so the usual
/// echo is `PROTOCOL_VERSION`. Accepting a *lower* settled version is what
/// makes this daemon work against a control plane older than it: this end only
/// answers questions, so serving at v2 costs it nothing — it is asked less.
/// Without the range, upgrading a node before its control plane would take
/// that node's tunnel down, which is the wrong way round for a fleet where
/// nodes belong to their operators.
///
/// Two, not one: v1 predates the delegations query, and a control plane that
/// settled on v1 would be old enough that nothing in this tree has met one.
pub(crate) const MIN_PROTOCOL_VERSION: u32 = 2;

/// Whether this daemon will serve under the version a control plane settled on.
///
/// Named rather than left inline in the handshake's match guard so the range
/// has one definition and a test can hold it to it: getting the ends wrong is
/// either a tunnel that drops on upgrade or one that stays up while a frame
/// goes undecoded, and neither announces itself.
pub(crate) fn settles_at(version: u32) -> bool {
    (MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION).contains(&version)
}

/// The domain-separation tag an attach proof signs under.
///
/// Distinct from `sync-head/1`, so a signature minted here can never be read
/// as a head signature, nor a head signature replayed as an attach proof.
pub(crate) const ATTACH_SIGNING_DOMAIN: &[u8] = b"synch-cloud-attach-v1";

/// The domain-separation tag a *write-tunnel* attach proof signs under
/// (`docs/CLOUD-WRITES.md` §5.2).
///
/// The write tunnel is the data plane's, not this module's — its frames live
/// in `synch-dp`, so that no daemon links a decoder for them — but the proof
/// is minted by the engine, because the device secret never leaves it. A
/// distinct tag from `ATTACH_SIGNING_DOMAIN` is what makes "a browse proof
/// cannot be replayed at the write endpoint" a statement about the signature
/// rather than about two URLs happening to differ.
pub const WRITE_ATTACH_SIGNING_DOMAIN: &[u8] = b"synch-cloud-write-v1";

/// How many bytes an attach nonce carries.
pub const NONCE_LEN: usize = 32;

/// The largest payload one binary content frame carries.
pub(crate) const MAX_CHUNK: usize = 64 * 1024;

/// The fixed header every binary content frame opens with: request id then
/// sequence, both big-endian `u32`.
pub(crate) const CHUNK_HEADER_LEN: usize = 8;

/// The exact bytes an attach proof covers:
///
/// ```text
/// "synch-cloud-attach-v1" || url || nonce
/// ```
///
/// The nonce is fixed-width and last, so no `(url, nonce)` pair produces the
/// same input as another — a length prefix would buy nothing a fixed-width
/// suffix does not already give. Binding the URL is what stops a proof minted
/// for one control plane being replayed at another: the daemon signs the
/// endpoint it actually dialed, and the endpoint checks against its own.
pub(crate) fn attach_signing_input(url: &str, nonce: &[u8]) -> Vec<u8> {
    signing_input(ATTACH_SIGNING_DOMAIN, url, nonce)
}

/// The exact bytes a write-tunnel attach proof covers:
///
/// ```text
/// "synch-cloud-write-v1" || url || nonce
/// ```
///
/// The same shape as `attach_signing_input` under its own tag, for the same
/// reasons: fixed-width nonce last, URL bound.
pub fn write_attach_signing_input(url: &str, nonce: &[u8]) -> Vec<u8> {
    signing_input(WRITE_ATTACH_SIGNING_DOMAIN, url, nonce)
}

fn signing_input(domain: &[u8], url: &str, nonce: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(domain.len() + url.len() + nonce.len());
    buf.extend_from_slice(domain);
    buf.extend_from_slice(url.as_bytes());
    buf.extend_from_slice(nonce);
    buf
}

/// Wraps a payload in its content-frame header.
pub(crate) fn encode_chunk(id: u32, seq: u32, data: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(CHUNK_HEADER_LEN + data.len());
    buf.extend_from_slice(&id.to_be_bytes());
    buf.extend_from_slice(&seq.to_be_bytes());
    buf.extend_from_slice(data);
    buf
}

/// Reads a content frame's header, returning the payload behind it.
///
/// The production decoder is the control plane's; this one exists so tests can
/// check what [`encode_chunk`] put on the wire.
#[cfg(test)]
pub(crate) fn decode_chunk(frame: &[u8]) -> Option<(u32, u32, &[u8])> {
    if frame.len() < CHUNK_HEADER_LEN {
        return None;
    }
    let id = u32::from_be_bytes(frame[0..4].try_into().ok()?);
    let seq = u32::from_be_bytes(frame[4..8].try_into().ok()?);
    Some((id, seq, &frame[CHUNK_HEADER_LEN..]))
}

/// What the control plane sends down the tunnel.
///
/// Every variant either asks a question about the unified tree, asks who the
/// cluster admits to it, or governs one stream. None of them changes anything
/// on the node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub(crate) enum Down {
    /// The 32-byte nonce, hex, that the attach proof must cover.
    Challenge {
        /// The nonce, hex-encoded.
        nonce: String,
    },
    /// The proof was accepted and the session is live.
    Attached {
        /// The control plane's id for this session, for logs on both sides.
        session: String,
        /// The protocol version the control plane settled on.
        v: u32,
    },
    /// One page of a directory of the unified tree.
    Ls {
        /// The request id.
        id: u32,
        /// The space.
        space: String,
        /// The directory within the space, empty for its root.
        path: String,
        /// Resume after this path, exclusive.
        #[serde(default)]
        cursor: Option<String>,
        /// Inline every version of every entry, with its attestors.
        #[serde(default)]
        all: bool,
    },
    /// Every version of one path, with attestors.
    Stat {
        /// The request id.
        id: u32,
        /// The space.
        space: String,
        /// The path within the space.
        path: String,
    },
    /// The version a selector picks, its content root, and who holds it.
    Resolve {
        /// The request id.
        id: u32,
        /// The space.
        space: String,
        /// The path within the space.
        path: String,
        /// Pin one origin's version; unset takes the newest.
        #[serde(default)]
        from: Option<String>,
    },
    /// A byte range of a content root a [`Down::Resolve`] pinned.
    Read {
        /// The request id.
        id: u32,
        /// The content root, hex.
        root: String,
        /// The object's full size, as the resolve reported it.
        size: u64,
        /// The first byte to read.
        #[serde(default)]
        start: u64,
        /// How many bytes, or unset to the end of the object.
        #[serde(default)]
        len: Option<u64>,
        /// How many chunks may be sent before the first credit arrives.
        #[serde(default)]
        credit: u32,
    },
    /// More chunks may be sent on one stream.
    Credit {
        /// The stream's request id.
        id: u32,
        /// How many further chunks are allowed.
        n: u32,
    },
    /// Abandon one stream.
    Cancel {
        /// The stream's request id.
        id: u32,
    },
    /// Every delegation this node honors (§3.5), answered with
    /// [`Up::Delegations`].
    ///
    /// Takes no argument: delegations are replicated to every member, so any
    /// attached node answers for the whole cluster and there is no per-origin
    /// filter that would mean anything.
    Delegations {
        /// The request id.
        id: u32,
    },
    /// What this node replicates and how far behind it is
    /// (`docs/REPLICATION.md` §8), answered with [`Up::Replication`].
    ///
    /// Takes no argument, and unlike [`Down::Delegations`] that is not because
    /// any node can answer for the cluster. Replication is a per-node decision
    /// — one node replicates `media`, its neighbour does not, and both are
    /// correct — so this reports the answering node alone. A control plane that
    /// wants the fleet's picture asks every attached daemon and says which said
    /// what.
    Replication {
        /// The request id.
        id: u32,
    },
    /// Liveness, answered with [`Up::Pong`].
    Ping,
    /// The answer to a [`Up::Ping`].
    Pong,
    /// A coded refusal, of one request or of the connection.
    Err {
        /// The request it refuses, or unset for the connection itself.
        #[serde(default)]
        id: Option<u32>,
        /// The stable code.
        code: String,
        /// What went wrong, in the words a person reads.
        message: String,
    },
}

/// What the node sends up the tunnel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub(crate) enum Up {
    /// The opening frame: who is attaching, for what, and speaking what.
    Hello {
        /// The protocol version this daemon speaks.
        v: u32,
        /// The membership domain this attach is for.
        network: String,
        /// This node's origin, canonically rendered.
        origin: String,
        /// The active device key, z-base-32.
        device: String,
        /// The spaces this node holds — published or replicated — as they stood
        /// when the session opened. A routing claim, not a boundary: the
        /// daemon serves whatever the control plane asks of it.
        spaces: Vec<String>,
    },
    /// The signed challenge.
    Proof {
        /// The signature, hex.
        sig: String,
        /// The device key that produced it, z-base-32.
        key: String,
    },
    /// A replacement for the session's routing claim.
    ///
    /// Sources and replicas can be added or removed while this tunnel is
    /// alive. This frame makes those changes routable without waiting for a
    /// network failure to force another hello.
    Spaces {
        /// Every space this node currently publishes or replicates.
        spaces: Vec<String>,
    },
    /// One page of a listing.
    Page {
        /// The request id.
        id: u32,
        /// The entries of this page.
        entries: Vec<EntryJson>,
        /// Where the next page resumes, or unset at the end of the listing.
        cursor: Option<String>,
    },
    /// Every version of one path.
    Versions {
        /// The request id.
        id: u32,
        /// The versions, newest-wins order last.
        versions: Vec<VersionJson>,
    },
    /// What a resolve settled on.
    Resolved {
        /// The request id.
        id: u32,
        /// The origin whose version was selected.
        origin: String,
        /// The content root, hex.
        root: String,
        /// The object's size.
        size: u64,
        /// The seq the version was published at.
        seq: u64,
        /// Origins currently advertising availability for that root.
        holders: Vec<String>,
    },
    /// The header of a content stream, before its first chunk.
    Meta {
        /// The stream's request id.
        id: u32,
        /// How many bytes this stream will carry.
        size: u64,
        /// The content root the bytes were verified against, hex.
        root: String,
    },
    /// The delegations this node honors, live and lapsed alike.
    Delegations {
        /// The request id.
        id: u32,
        /// One row per `(issuer, subject)` pair, in the store's order.
        delegations: Vec<DelegationJson>,
    },
    /// What this node replicates, one row per replica.
    Replication {
        /// The request id.
        id: u32,
        /// The replicas, in the store's order. Empty is an answer:
        /// this node replicates nothing, which is different from not having
        /// been asked.
        spaces: Vec<ReplicaSpaceJson>,
    },
    /// A stream ended, having sent everything it was asked for.
    Done {
        /// The stream's request id.
        id: u32,
    },
    /// Liveness.
    Ping,
    /// The answer to a [`Down::Ping`].
    Pong,
    /// A coded refusal, carrying the daemon's own error codes verbatim.
    Err {
        /// The request it refuses, or unset for the connection itself.
        #[serde(default)]
        id: Option<u32>,
        /// The stable code.
        code: String,
        /// What went wrong.
        message: String,
    },
}

/// One entry of a directory of the unified tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EntryJson {
    /// The entry's name within the directory listed.
    pub name: String,
    /// The full path within the space.
    pub path: String,
    /// `dir`, `file`, `symlink` or `tombstone`.
    pub kind: String,
    /// The selected version's size.
    pub size: u64,
    /// The selected version's mtime, unix nanoseconds.
    pub mtime_ns: i64,
    /// How many versions the path carries: more than one is divergence.
    pub versions: u32,
    /// The origin the newest-wins policy selected.
    pub origin: String,
    /// The selected version's content root, hex, for files.
    pub root: Option<String>,
    /// Every version, when the request asked for them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub all: Vec<VersionJson>,
}

/// One version of one path, as a listing or the inspector renders it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct VersionJson {
    /// The content root, hex, for content-carrying kinds.
    pub root: Option<String>,
    /// `file`, `dir`, `symlink` or `tombstone`.
    pub kind: String,
    /// The link target, for a symlink.
    pub symlink_target: Option<String>,
    /// The content length.
    pub size: u64,
    /// The greatest mtime any attestor published.
    pub mtime_ns: i64,
    /// The greatest seq any attestor published it at.
    pub seq: u64,
    /// Every origin asserting this version.
    pub attestors: Vec<String>,
}

/// One delegation, as the control plane renders it (§3.5).
///
/// Carries `live` rather than leaving the reader to compare `not_after` with a
/// clock: derived trust dies with its source, so a delegation whose issuer has
/// been removed or has lapsed from DNS is dead well before its own expiry, and
/// a date is not enough to tell. The date travels too, because "when does this
/// end" is a different question from "does it hold now".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DelegationJson {
    /// The delegated device key, z-base-32.
    pub key: String,
    /// The origin that issued it, canonically rendered.
    pub issuer: String,
    /// The spaces the grant covers.
    pub spaces: Vec<String>,
    /// Whether this node honors it *now*, cascade applied.
    pub live: bool,
    /// When the delegation expires, unix nanoseconds.
    pub not_after: Option<i64>,
    /// When this node first materialized the record, unix nanoseconds.
    pub added_at: i64,
    /// The issuer's note, if it published one.
    pub note: Option<String>,
}

/// One replica, as this node reports it (`docs/REPLICATION.md` §8).
///
/// The counts are the store's, not a summary: `wanted` includes `unreachable`,
/// because that is what the store means by it, and a reader that wants the
/// backlog alone subtracts. Folding them here would put the two numbers this
/// report exists for — a queue that is draining and a queue that is dead —
/// behind one that cannot tell them apart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReplicaSpaceJson {
    /// The space's id.
    pub space: String,
    /// `current` or `forever`.
    pub policy: String,
    /// The grace window a superseded root gets, in seconds. Reported under
    /// `forever` too, where it is inert: nothing is ever scheduled.
    pub grace_secs: i64,
    /// The ceiling on held bytes, if the space has one.
    pub budget: Option<u64>,
    /// Objects pinned for this space, and the bytes they account for.
    pub held: u64,
    /// Bytes those objects account for.
    pub held_bytes: u64,
    /// Held objects with a scheduled release.
    pub releasing: u64,
    /// Bytes those objects account for.
    pub releasing_bytes: u64,
    /// Objects wanted and not yet held, `unreachable` included.
    pub wanted: u64,
    /// Bytes those objects would add.
    pub wanted_bytes: u64,
    /// Wanted objects no provider has answered for.
    pub unreachable: u64,
    /// Bytes those objects would add.
    pub unreachable_bytes: u64,
    /// Objects the tree has stopped naming that this node holds anyway,
    /// because too few other origins advertise them (§4.3).
    pub held_back: u64,
    /// When the oldest outstanding want was first wanted, unix nanoseconds.
    pub oldest_want: Option<i64>,
    /// When the soonest scheduled release falls due, unix nanoseconds.
    pub next_release: Option<i64>,
    /// Whether there are no pending heads and every bound origin has a complete
    /// materialized baseline. Release sweeps do not gate on it.
    pub view_complete: bool,
    /// Why synchronization is incomplete, when it is.
    pub view_reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use synch_core::head_signing_input;

    /// A routing refresh is a replacement, not a delta: the control plane can
    /// update one session atomically and removals need no second opcode.
    #[test]
    fn the_live_space_claim_wire_layout_is_pinned() {
        let frame = Up::Spaces {
            spaces: vec!["docs".into(), "media".into()],
        };
        let json: serde_json::Value = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["t"], "spaces");
        assert_eq!(json["spaces"], serde_json::json!(["docs", "media"]));
        assert_eq!(SPACE_UPDATES_VERSION, PROTOCOL_VERSION);
    }

    /// The delegations answer's field names are the contract: the other side
    /// is written in another language, which decodes them by name — a rename
    /// here is a dashboard that quietly stops showing who the cluster admits.
    #[test]
    fn the_delegations_wire_layout_is_pinned() {
        let frame = Up::Delegations {
            id: 4,
            delegations: vec![DelegationJson {
                key: "abc".into(),
                issuer: "nas@cluster.example".into(),
                spaces: vec!["photos".into()],
                live: true,
                not_after: Some(1_700_000_000_000_000_000),
                added_at: 12,
                note: Some("laptop".into()),
            }],
        };
        let json: serde_json::Value = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["t"], "delegations");
        assert_eq!(json["id"], 4);
        let row = &json["delegations"][0];
        assert_eq!(row["key"], "abc");
        assert_eq!(row["issuer"], "nas@cluster.example");
        assert_eq!(row["spaces"][0], "photos");
        assert_eq!(row["live"], true);
        assert_eq!(row["not_after"], 1_700_000_000_000_000_000i64);
        assert_eq!(row["added_at"], 12);
        assert_eq!(row["note"], "laptop");

        let ask: serde_json::Value = serde_json::to_value(Down::Delegations { id: 4 }).unwrap();
        assert_eq!(ask["t"], "delegations");
        assert_eq!(ask["id"], 4);

        // A grant with no expiry and no note travels as null rather than
        // vanishing: the Gleam decoder reads both fields as nullable, and a
        // missing key is a different shape from a null one.
        let bare = serde_json::to_value(Up::Delegations {
            id: 1,
            delegations: vec![DelegationJson {
                key: "k".into(),
                issuer: String::new(),
                spaces: Vec::new(),
                live: false,
                not_after: None,
                added_at: 0,
                note: None,
            }],
        })
        .unwrap();
        assert!(bare["delegations"][0]["not_after"].is_null());
        assert!(bare["delegations"][0]["note"].is_null());
    }

    /// A daemon serves under an older control plane, and refuses a version
    /// neither end could have meant.
    ///
    /// The upgrade order in a real fleet is not chosen by anyone: nodes belong
    /// to their operators, so a node may be newer than the control plane it
    /// attaches to, or older. Both work, because this end only *answers* — a
    /// lower settled version costs it nothing but questions it is not asked.
    /// What must not happen is a settled version outside the range being taken
    /// as agreement, since the frames that follow are then anyone's guess.
    #[test]
    fn a_daemon_serves_the_versions_it_still_speaks() {
        assert!(settles_at(PROTOCOL_VERSION), "its own, the ordinary case");
        assert!(
            settles_at(MIN_PROTOCOL_VERSION),
            "and the oldest it still serves — an older control plane is a \
             control plane, not a fault"
        );
        assert!(
            !settles_at(MIN_PROTOCOL_VERSION - 1),
            "below the floor is a version this build has no frames for"
        );
        assert!(
            !settles_at(PROTOCOL_VERSION + 1),
            "and above its own is a control plane that will ask questions this \
             daemon cannot decode — which ends the tunnel, so it must not attach"
        );
    }

    /// The replication answer's field names are the contract, for the same
    /// reason the delegations answer's are: the decoder is in another language
    /// and reads them by name.
    #[test]
    fn the_replication_wire_layout_is_pinned() {
        let frame = Up::Replication {
            id: 7,
            spaces: vec![ReplicaSpaceJson {
                space: "media".into(),
                policy: "current".into(),
                grace_secs: 2_592_000,
                budget: Some(1 << 40),
                held: 12,
                held_bytes: 4096,
                releasing: 2,
                releasing_bytes: 512,
                wanted: 5,
                wanted_bytes: 900,
                unreachable: 1,
                unreachable_bytes: 100,
                held_back: 3,
                oldest_want: Some(1_700_000_000_000_000_000),
                next_release: Some(1_800_000_000_000_000_000),
                view_complete: false,
                view_reason: Some("nas@x.example is bound but has published nothing".into()),
            }],
        };
        let json: serde_json::Value = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["t"], "replication");
        assert_eq!(json["id"], 7);
        let row = &json["spaces"][0];
        assert_eq!(row["space"], "media");
        assert_eq!(row["policy"], "current");
        assert_eq!(row["grace_secs"], 2_592_000);
        assert_eq!(row["budget"], 1u64 << 40);
        assert_eq!(row["held"], 12);
        assert_eq!(row["held_bytes"], 4096);
        assert_eq!(row["releasing"], 2);
        assert_eq!(row["releasing_bytes"], 512);
        assert_eq!(row["wanted"], 5);
        assert_eq!(row["wanted_bytes"], 900);
        assert_eq!(row["unreachable"], 1);
        assert_eq!(row["unreachable_bytes"], 100);
        assert_eq!(row["held_back"], 3);
        assert_eq!(row["oldest_want"], 1_700_000_000_000_000_000i64);
        assert_eq!(row["next_release"], 1_800_000_000_000_000_000i64);
        assert_eq!(row["view_complete"], false);
        assert_eq!(
            row["view_reason"],
            "nas@x.example is bound but has published nothing"
        );

        let ask: serde_json::Value = serde_json::to_value(Down::Replication { id: 7 }).unwrap();
        assert_eq!(ask["t"], "replication");
        assert_eq!(ask["id"], 7);

        // The three optional fields travel as null rather than vanishing: the
        // Gleam decoder reads them as nullable, and a missing key is a
        // different shape from a null one.
        let bare = serde_json::to_value(Up::Replication {
            id: 1,
            spaces: vec![ReplicaSpaceJson {
                space: "docs".into(),
                policy: "forever".into(),
                grace_secs: 0,
                budget: None,
                held: 0,
                held_bytes: 0,
                releasing: 0,
                releasing_bytes: 0,
                wanted: 0,
                wanted_bytes: 0,
                unreachable: 0,
                unreachable_bytes: 0,
                held_back: 0,
                oldest_want: None,
                next_release: None,
                view_complete: true,
                view_reason: None,
            }],
        })
        .unwrap();
        assert!(bare["spaces"][0]["budget"].is_null());
        assert!(bare["spaces"][0]["oldest_want"].is_null());
        assert!(bare["spaces"][0]["next_release"].is_null());
        assert!(bare["spaces"][0]["view_reason"].is_null());

        // A node replicating nothing answers with an empty list, which must
        // still be a list: the panel says "replicates nothing" from it.
        let none = serde_json::to_value(Up::Replication {
            id: 2,
            spaces: Vec::new(),
        })
        .unwrap();
        assert_eq!(none["spaces"], serde_json::json!([]));
    }

    /// The two contexts a device key signs under must not overlap: a head
    /// signature verified as an attach proof lets anyone who saw a published
    /// head attach as that node.
    #[test]
    fn an_attach_proof_is_not_a_head_signature() {
        let key = iroh_base::SecretKey::generate();
        let node_id = key.public();
        let origin = synch_core::OriginId::named("nas", "cluster.example").unwrap();
        let url = "https://sync.example/agent/v1/attach";
        let nonce = [7u8; NONCE_LEN];

        let attach = attach_signing_input(url, &nonce);
        let head = head_signing_input(&origin, 7, &synch_core::Hash::new(b"root"), 1234, &node_id);
        assert_ne!(attach, head);

        // And the signatures themselves do not cross over.
        let attach_sig = key.sign(&attach);
        let head_sig = key.sign(&head);
        assert!(node_id.verify(&attach, &attach_sig).is_ok());
        assert!(node_id.verify(&head, &attach_sig).is_err());
        assert!(node_id.verify(&attach, &head_sig).is_err());
    }

    /// A browse proof and a write proof over the same URL and nonce differ,
    /// so neither endpoint accepts the other's signature.
    #[test]
    fn a_write_attach_proof_is_not_a_browse_attach_proof() {
        let nonce = [3u8; NONCE_LEN];
        let url = "https://sync.example/dp/v1/attach";
        assert_ne!(
            attach_signing_input(url, &nonce),
            write_attach_signing_input(url, &nonce)
        );
        assert!(write_attach_signing_input(url, &nonce).starts_with(WRITE_ATTACH_SIGNING_DOMAIN));
    }

    /// The URL is part of what is signed, so a proof cannot be forwarded to a
    /// second control plane by whoever holds the first one's connection.
    #[test]
    fn an_attach_proof_is_bound_to_the_endpoint_it_was_minted_for() {
        let nonce = [1u8; NONCE_LEN];
        assert_ne!(
            attach_signing_input("https://a.example/agent/v1/attach", &nonce),
            attach_signing_input("https://b.example/agent/v1/attach", &nonce)
        );
        // The boundary cannot be shifted: the nonce is a fixed-width tail, so
        // a URL ending in the first nonce byte is not a shifted-nonce input.
        let shifted = [&[0x2fu8][..], &nonce[..NONCE_LEN - 1]].concat();
        assert_ne!(
            attach_signing_input("https://a.example", &nonce),
            attach_signing_input("https://a.example/", &shifted)
        );
    }

    #[test]
    fn content_frames_round_trip_through_their_header() {
        let payload = vec![9u8; 1024];
        let frame = encode_chunk(0x0102_0304, 7, &payload);
        assert_eq!(frame.len(), CHUNK_HEADER_LEN + payload.len());
        let (id, seq, data) = decode_chunk(&frame).unwrap();
        assert_eq!(id, 0x0102_0304);
        assert_eq!(seq, 7);
        assert_eq!(data, &payload[..]);

        // An empty payload is a legal frame; a truncated header is not.
        let empty = encode_chunk(1, 0, &[]);
        let (id, seq, data) = decode_chunk(&empty).unwrap();
        assert_eq!((id, seq, data.len()), (1, 0, 0));
        assert!(decode_chunk(&[0, 0, 0, 1, 0, 0, 0]).is_none());
    }

    #[test]
    fn control_frames_round_trip_through_json() {
        let cases = vec![
            Down::Challenge {
                nonce: "aa".repeat(NONCE_LEN),
            },
            Down::Ls {
                id: 4,
                space: "media".into(),
                // A colon in a path is exactly what a text reference cannot
                // carry, which is why these frames are structured.
                path: "uploads/2026:07".into(),
                cursor: None,
                all: true,
            },
            Down::Read {
                id: 9,
                root: "ff".repeat(32),
                size: 4096,
                start: 512,
                len: Some(1024),
                credit: 4,
            },
            Down::Credit { id: 9, n: 2 },
            Down::Cancel { id: 9 },
            Down::Ping,
            Down::Err {
                id: None,
                code: "browse-disabled".into(),
                message: "no".into(),
            },
        ];
        for frame in cases {
            let text = serde_json::to_string(&frame).unwrap();
            assert_eq!(serde_json::from_str::<Down>(&text).unwrap(), frame);
        }

        let up = Up::Resolved {
            id: 1,
            origin: "nas@cluster.example".into(),
            root: "ab".repeat(32),
            size: 7,
            seq: 3,
            holders: vec!["nas@cluster.example".into()],
        };
        let text = serde_json::to_string(&up).unwrap();
        assert_eq!(serde_json::from_str::<Up>(&text).unwrap(), up);

        // Absent optional fields decode: a control plane one release behind
        // must not be a parse failure.
        let frame: Down =
            serde_json::from_str(r#"{"t":"ls","id":1,"space":"m","path":""}"#).unwrap();
        assert_eq!(
            frame,
            Down::Ls {
                id: 1,
                space: "m".into(),
                path: String::new(),
                cursor: None,
                all: false,
            }
        );
        let frame: Down =
            serde_json::from_str(r#"{"t":"read","id":1,"root":"ab","size":9}"#).unwrap();
        assert_eq!(
            frame,
            Down::Read {
                id: 1,
                root: "ab".into(),
                size: 9,
                start: 0,
                len: None,
                credit: 0,
            }
        );
    }

    /// There is no opcode that writes: a frame naming one fails to decode,
    /// not at some later gate.
    #[test]
    fn no_frame_encodes_a_write() {
        for attempt in [
            r#"{"t":"put","id":1,"space":"m","path":"a"}"#,
            r#"{"t":"take","id":1,"space":"m","path":"a"}"#,
            r#"{"t":"appendconfig","id":1,"key":"s3.x","record":"y"}"#,
        ] {
            assert!(serde_json::from_str::<Down>(attempt).is_err(), "{attempt}");
        }
    }
}
