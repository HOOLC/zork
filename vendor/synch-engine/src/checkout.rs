//! Continuous read-only checkout materialization of retained replica content.
//!
//! A checkout is the optional filesystem projection of one replica. Every pass
//! writes the unified tree's deterministic newest version. It has no selection
//! or fetching policy of its own: paths are materialized only after the owning
//! replica has acquired their content durably.
//!
//! Checked out trees are never indexed back into the local origin trie, and the
//! engine refuses overlapping space and checkout roots, so "no echo" is
//! structural rather than conventional.
//!
//! Materialization is deliberately conservative about names: when two published
//! paths collide under the target filesystem's folding, or a name is invalid on
//! the platform, the entry is **skipped and reported** — never silently
//! clobbered.
//!
//! A materialized file carries the metadata its entry published as well as its
//! bytes: the origin's mtime, and its advisory unix mode masked to the
//! permission bits (§4.2). A checkout is meant to be usable as the tree it
//! checkouts — `rsync`ed onward, served, executed — and a directory of
//! `0644 Just Now` files is a different tree from the one the origin published.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use synch_core::{ChunkRanges, EntryKind};
use synch_store::{EntryRow, ReplicaRow, VersionPolicy};

use crate::{
    error::{EngineError, Result},
    node::{paths_overlap, stored_root, CheckoutWrite, Node},
};

/// What one checkout pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckoutReport {
    /// Files written or refreshed.
    pub written: usize,
    /// Files already up to date.
    pub current: usize,
    /// Files whose bytes were already current but whose mode or mtime had
    /// drifted from the selected version, and were stamped back onto it.
    pub retouched: usize,
    /// Files removed: the selected version is a tombstone, or the path has
    /// left the unified tree entirely.
    pub removed: usize,
    /// Files whose bytes were not copied at all: the checkout shares a filesystem
    /// with the CAS, so the file and the object it came from are the same
    /// extents (`docs/DELTA-SYNC.md` §3.5). A subset of `written`.
    pub reflinked: usize,
    /// Entries skipped, with the reason, including content the replica has not
    /// acquired yet and metadata the filesystem refused.
    pub skipped: Vec<(String, String)>,
}

impl Node {
    /// Validates and canonicalizes a replica checkout directory.
    pub fn checkout_path(&self, space: &str, local_path: impl AsRef<Path>) -> Result<String> {
        let path = local_path.as_ref();
        std::fs::create_dir_all(path)?;
        let path = std::fs::canonicalize(path)?;
        for existing in self.store().sources()? {
            let Some(local_path) = existing.local_path.as_deref() else {
                continue;
            };
            if paths_overlap(&path, &stored_root(local_path)) {
                return Err(EngineError::invalid(format!(
                    "checkout {} overlaps source {}",
                    path.display(),
                    existing.space
                )));
            }
        }
        let key = path.to_string_lossy().into_owned();
        for existing in self.store().replicas()? {
            let Some(existing_path) = existing.checkout_path else {
                continue;
            };
            if existing.space != space && paths_overlap(&path, &stored_root(&existing_path)) {
                return Err(EngineError::invalid(format!(
                    "checkout {} overlaps replica checkout {}",
                    path.display(),
                    existing_path
                )));
            }
        }
        Ok(key)
    }

    /// Brings one replica checkout up to date with the newest unified view.
    pub async fn sync_checkout(&self, space: &str) -> Result<CheckoutReport> {
        let replica = {
            let store = self.store().clone();
            let space = space.to_string();
            crate::blocking::offload(move || {
                store
                    .replica(&space)?
                    .filter(|row| row.checkout_path.is_some())
                    .ok_or_else(|| {
                        EngineError::not_found(format!("replica {space} has no checkout"))
                    })
            })
            .await?
        };
        self.sync_checkout_row(&replica).await
    }

    async fn sync_checkout_row(&self, replica: &ReplicaRow) -> Result<CheckoutReport> {
        // Passes serialize node-wide: whether the standing loop or `synch
        // checkout sync` asked, two passes over one root would plan against
        // each other's half-written state.
        let _pass = self.lock_materialization().await;
        let root_dir = PathBuf::from(
            replica
                .checkout_path
                .as_deref()
                .expect("checkout rows are filtered before a pass"),
        );

        // A pass runs in three phases, because only the middle one is
        // asynchronous. Deciding what each path needs is filesystem work — an
        // lstat per ancestor, a whole-file hash for anything that might already
        // be current — and so is writing the answer; only fetching the content
        // that is missing is network work. Alternating between the two inside
        // one loop would put every one of those hashes on the runtime worker
        // that happened to poll this future, so the blocking halves are hoisted
        // out and run on the blocking pool instead (§10).
        //
        // Phase 1 keeps listing order, which is what the symlink-escape guard
        // depends on: a link the checkout writes for `sub` is on disk before
        // `sub/passwd` is judged, exactly as it would be with the two steps
        // interleaved. Phase 2 re-checks it immediately before each write, so
        // the gap between the check and the write it guards stays one write
        // wide rather than one pass wide.
        //
        // The listing joins phase 1 rather than preceding it: it is an
        // unlimited range scan over every path in the space, plus one
        // `VersionSet` per path, which is more SQLite work than anything the
        // plan itself does.
        let plan = {
            let node = self.clone();
            let space = replica.space.clone();
            let root_dir = root_dir.clone();
            crate::blocking::offload(move || {
                let listing = node.unified_listing(&space, "", None, None)?;
                plan_pass(&node, &root_dir, &listing)
            })
            .await?
        };
        let CheckoutPass {
            mut report,
            known,
            wanted,
        } = plan;
        let checkout_holder = replica.holder();

        // Phase 2: fetch what phase 1 could not satisfy locally — building each
        // new object in the CAS out of the old one where it can — and
        // materialize it as it lands.
        for want in wanted {
            let ready = {
                let store = self.store().clone();
                let (root, holder) = (want.content, checkout_holder.clone());
                crate::blocking::offload(move || {
                    let held = store
                        .pins_for(&root)?
                        .iter()
                        .any(|pin| pin.holder == holder);
                    let complete = store
                        .blob(&root)?
                        .is_some_and(|blob| blob.complete || blob.durable);
                    Ok(held && complete)
                })
                .await?
            };
            if !ready {
                report.skipped.push((
                    want.path,
                    "replica has not acquired this content yet".into(),
                ));
                continue;
            }
            // Cloned out of the CAS and renamed into place: a checkout of
            // multi-gigabyte objects must not hold one in memory, and a pass
            // interrupted halfway must not leave a truncated file wearing a
            // complete file's name (§7.2, §9.4).
            //
            // The escape guard is taken again here, in the same blocking step
            // as the write it protects. Phase 1 checked this path too, but a
            // fetch stands between the two and the whole point of the guard is
            // to describe the directory the write is about to land in.
            let root = root_dir.clone();
            let path = want.path.clone();
            let written_target = want.target.clone();
            let target = want.target.clone();
            let ready = crate::blocking::offload(move || {
                if escapes_via_symlink(&root, &path) {
                    return Ok(false);
                }
                // A directory standing where a file belongs is cleared first, if
                // it is empty. The tree has published this path as a file and the
                // write below would meet `EISDIR`; the sweep tidies the directory
                // in phase 3, *after* this, so on its own that only fixes the
                // pass after next. A non-empty directory still fails here, which
                // is correct — its children are swept in phase 3 and the pass
                // after that succeeds.
                if target.symlink_metadata().is_ok_and(|meta| meta.is_dir()) {
                    let _ = std::fs::remove_dir(&target);
                }
                Ok(true)
            })
            .await?;
            let outcome = if !ready {
                Written::Escaped
            } else {
                // A materialization that fails takes its path down with it and
                // nothing else: the target is untouched, and the next pass
                // tries again.
                let kind = match self
                    .materialize_blob(&want.content, want.size, want.target.clone())
                    .await
                {
                    Ok(kind) => kind,
                    Err(e) => {
                        report
                            .skipped
                            .push((want.path, format!("content could not be written: {e}")));
                        continue;
                    }
                };
                let target = want.target.clone();
                let content = want.content;
                let meta = want.meta;
                crate::blocking::offload(move || {
                    // The bytes are the file; its metadata is stamped on right
                    // after, and a filesystem that refuses the stamp — a mount
                    // that will not take the mode, a foreign owner — is reported
                    // rather than allowed to fail the whole pass.
                    let stamped = apply_metadata(&target, meta);
                    // No read-back: a successful write is trusted the way the CAS
                    // trusts its own payloads (§2.1). What later passes trust is
                    // the record anchored to the fresh stat — anything that moves
                    // the file moves the stat, and the next pass hashes again.
                    Ok(match CheckoutWrite::of(&target, content) {
                        Some(record) => match stamped {
                            Ok(()) => Written::Fully(kind, record),
                            Err(e) => Written::WithoutMetadata(kind, record, e.to_string()),
                        },
                        None => Written::Failed(
                            "the file was gone before its write could be recorded".into(),
                        ),
                    })
                })
                .await?
            };
            // Remembered whenever the bytes landed, so later passes can
            // believe the file's stat instead of re-hashing it
            // (`Node::note_checkout_write`).
            if let Written::Fully(_, record) | Written::WithoutMetadata(_, record, _) = &outcome {
                self.note_checkout_write(&written_target, record.clone());
            }
            match outcome {
                Written::Fully(kind, _) => {
                    report.written += 1;
                    report.reflinked += usize::from(kind == crate::CloneKind::Reflink);
                }
                Written::WithoutMetadata(kind, _, why) => {
                    report.written += 1;
                    report.reflinked += usize::from(kind == crate::CloneKind::Reflink);
                    report.skipped.push((
                        want.path,
                        format!("content written, but its metadata could not be reproduced: {why}"),
                    ));
                }
                Written::Escaped => report.skipped.push((
                    want.path,
                    "path resolves through a symlink; refusing to write outside the checkout"
                        .into(),
                )),
                Written::Failed(why) => report
                    .skipped
                    .push((want.path, format!("content could not be written: {why}"))),
            }
        }

        // Phase 3: a path that has left the unified tree altogether — every
        // origin's entry for it gone, tombstones included — leaves the checkout
        // too. The listing cannot report those, so the last step is to look at
        // what is on disk and drop whatever the tree no longer names.
        let removed = crate::blocking::offload(move || {
            let root = root_dir;
            sweep(&root, &root, &known)
        })
        .await?;
        report.removed += removed.len();
        // Whatever was proven about those targets proves nothing now: a file
        // that comes back under the same path starts unproven.
        for path in removed {
            self.forget_checkout_write(&path);
        }
        Ok(report)
    }

    /// Brings every configured checkout up to date, one pass each.
    ///
    /// A checkout whose pass fails is reported in its slot rather than stopping
    /// the rest: this is the standing loop's body, and one broken checkout must
    /// not starve the others.
    pub async fn sync_all_checkouts(&self) -> Result<Vec<(String, Result<CheckoutReport>)>> {
        let mut out = Vec::new();
        let node = self.clone();
        let replicas = crate::blocking::offload(move || Ok(node.store().replicas()?)).await?;
        for replica in replicas
            .into_iter()
            .filter(|row| row.checkout_path.is_some())
        {
            let path = replica.checkout_path.clone().expect("filtered");
            let report = self.sync_checkout_row(&replica).await;
            out.push((path, report));
        }
        Ok(out)
    }

    /// Runs the standing checkout loop until `shutdown` resolves (§7.2).
    ///
    /// A pass runs on every wake — a head flipping complete on any exchange,
    /// a local publish, a freshly added checkout — and one runs before the
    /// first wait, so a tree the node already holds materializes at startup
    /// rather than on the first change. The `checkout_interval` fallback is the
    /// backstop for drift nobody rang about: a `chmod` moves nothing a record
    /// holds, and only a pass repairs it.
    pub async fn run_checkouts(&self, shutdown: impl std::future::Future<Output = ()>) {
        crate::aae::run_standing(
            shutdown,
            self.checkout_wake(),
            self.config().checkout_interval,
            || self.sync_all_checkouts_logged(),
        )
        .await
    }

    /// One pass over every checkout, logged rather than streamed: the standing
    /// loop has no client on the other end. A quiet pass says nothing — a
    /// tree this size reports `current` on every interval, and that is not
    /// news.
    async fn sync_all_checkouts_logged(&self) {
        match self.sync_all_checkouts().await {
            Ok(reports) => {
                for (path, report) in reports {
                    match report {
                        Ok(report)
                            if report.written + report.removed + report.retouched > 0
                                || !report.skipped.is_empty() =>
                        {
                            tracing::info!(
                                path = %path,
                                written = report.written,
                                current = report.current,
                                retouched = report.retouched,
                                removed = report.removed,
                                skipped = report.skipped.len(),
                                "checkout pass"
                            );
                            for (skipped, reason) in &report.skipped {
                                tracing::info!(path = %path, skipped = %skipped, reason = %reason, "checkout skipped a path");
                            }
                        }
                        Ok(report) => {
                            tracing::debug!(path = %path, current = report.current, "checkout pass")
                        }
                        Err(e) => {
                            tracing::warn!(path = %path, error = %e, "checkout pass failed")
                        }
                    }
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not list checkouts"),
        }
    }
}

/// One path a pass has decided it must fetch and write.
#[derive(Debug)]
struct WantedContent {
    /// The path within the space, for the report.
    path: String,
    /// Where it goes on disk.
    target: PathBuf,
    /// The object to materialize.
    content: synch_core::Hash,
    /// Its size, which is what the fetch and the copy are bounded by.
    size: u64,
    /// The metadata to stamp on once the bytes are there.
    meta: Metadata,
}

/// How one write in phase 2 ended.
#[derive(Debug)]
enum Written {
    /// Bytes and metadata both, how the bytes got there, and the record
    /// later passes will trust.
    Fully(crate::CloneKind, CheckoutWrite),
    /// The bytes landed; the filesystem refused the metadata.
    WithoutMetadata(crate::CloneKind, CheckoutWrite, String),
    /// Refused by the symlink-escape guard: nothing was written.
    Escaped,
    /// The object could not be materialized. The target is as it was.
    Failed(String),
}

/// What one pass settled before any content was fetched.
#[derive(Debug, Default)]
struct CheckoutPass {
    /// Everything already accounted for: removals, symlinks, skips, and paths
    /// that were already current.
    report: CheckoutReport,
    /// Every path the unified tree still carries — what the sweep uses to
    /// recognize a file whose path has left the tree.
    known: HashSet<String>,
    /// What is left for the asynchronous half to fetch.
    wanted: Vec<WantedContent>,
}

/// Decides, and performs, everything one pass can settle without the network.
///
/// Blocking from end to end: [`Node::sync_checkout_row`] runs it on the blocking
/// pool.
fn plan_pass(
    node: &Node,
    root_dir: &Path,
    listing: &[synch_store::VersionSet],
) -> Result<CheckoutPass> {
    // Detect folding collisions before writing anything: the
    // lexicographically first path wins and the rest are reported.
    let mut claimed: HashMap<String, String> = HashMap::new();
    // One clock reading for the pass, so every path in it selects against the
    // same instant — and the store's reading rather than the bare clock, or a
    // node whose clock lags the cluster clamps every honest entry to it, ties,
    // and checkouts whichever version has the greater content hash
    // (`Store::read_instant`).
    let now = node.store().read_instant()?;
    let mut report = CheckoutReport::default();
    let mut known: HashSet<String> = HashSet::new();
    let mut wanted: Vec<WantedContent> = Vec::new();

    {
        for set in listing {
            known.insert(set.path.clone());
            // Before the target path is built, because building it is already
            // the damage: on Windows `Path::join` reads a backslash as a
            // separator and a drive-prefixed argument as a root, so a published
            // `..\..\Users\x` or `C:\Windows\x` names a file outside the
            // checkout root — and the branches below remove what they are given.
            // Both bytes are ordinary in a Unix filename, so they stay legal
            // where entries are published and are refused here, at the boundary
            // where they mean something.
            if let Some(reason) = unsafe_name(&set.path) {
                report.skipped.push((set.path.clone(), reason));
                continue;
            }
            let target = root_dir.join(&set.path);
            // Defense in depth against a peer that plants a symlink and a file
            // beneath it (`sub` -> `/etc`, then `sub/passwd`): materialized in
            // path order the symlink lands first, and a later write to
            // `sub/passwd` would resolve through it to `/etc/passwd`, outside
            // the checkout root. Refuse — for writes and removals alike — any
            // path whose ancestors include a symlink the checkout itself wrote.
            if escapes_via_symlink(root_dir, &set.path) {
                report.skipped.push((
                    set.path.clone(),
                    "path resolves through a symlink; refusing to write outside the checkout"
                        .into(),
                ));
                continue;
            }
            let selected = match set.select(&VersionPolicy::Newest, now) {
                synch_store::Selection::Selected(entry) => *entry,
                synch_store::Selection::Absent => {
                    report.removed += remove_if_present(&target)?;
                    node.forget_checkout_write(&target);
                    continue;
                }
                synch_store::Selection::Divergent => unreachable!("newest always selects"),
            };

            // A path leaves the checkout when newest is a tombstone — the
            // deletion is the assertion this checkout follows.
            if selected.kind == EntryKind::Tombstone {
                report.removed += remove_if_present(&target)?;
                node.forget_checkout_write(&target);
                continue;
            }
            if selected.kind == EntryKind::Dir {
                continue;
            }
            // Claim before dispatching on kind so symlinks participate in the
            // same folded-name collision rule as regular files (§7.2).
            if let Err(reason) = claim_folded_name(&mut claimed, &set.path) {
                report.skipped.push((set.path.clone(), reason));
                continue;
            }
            if selected.kind == EntryKind::Symlink {
                match materialize_symlink(&target, selected.symlink_target.as_deref()) {
                    Ok(true) => report.written += 1,
                    Ok(false) => report.current += 1,
                    Err(reason) => report.skipped.push((set.path.clone(), reason)),
                }
                continue;
            }
            let Some(content) = selected.content else {
                report
                    .skipped
                    .push((set.path.clone(), "entry has no content".into()));
                continue;
            };
            let meta = Metadata::of(&selected);
            // The currency check: is the file on disk already the selected
            // version? A record this process holds answers with a stat — the
            // checkout wrote or hashed the file itself, and length, stored
            // mtime, and platform identity all still match, past the racy
            // window — because nothing that moves the file leaves the stat
            // standing. Without a record the content speaks for itself: a
            // file of the right length is hashed, because that hash *is* the
            // answer, and the answer becomes the record.
            let recorded = node.checkout_write_was(&target);
            if let Some(recorded) = &recorded {
                if let Ok(stat) = std::fs::metadata(&target) {
                    if stat.is_file() && still_current(recorded, &stat, &content) {
                        // Believed without a read. Only the metadata can have
                        // drifted: a bare chmod moves nothing the record
                        // holds.
                        if metadata_matches(&target, meta) {
                            report.current += 1;
                        } else if let Err(e) = apply_metadata(&target, meta) {
                            report.skipped.push((
                                set.path.clone(),
                                format!(
                                    "content is current, but its metadata could not be reproduced: {e}"
                                ),
                            ));
                        } else {
                            // The stamp moved the mtime; re-anchor the record
                            // to the stat it leaves behind.
                            note_record(node, &target, content);
                            report.retouched += 1;
                        }
                        continue;
                    }
                }
            }

            let on_disk = same_size_root(&target, selected.size);
            if on_disk == Some(content) {
                // Right bytes, and possibly the wrong mode or mtime: a local
                // `chmod`, a file this checkout wrote before it stamped metadata
                // at all, or a mode the origin has since changed without
                // touching the content. Repairing it is a `stat` and a syscall
                // or two — refetching the object to fix a permission bit is
                // not.
                if metadata_matches(&target, meta) {
                    report.current += 1;
                } else if let Err(e) = apply_metadata(&target, meta) {
                    report.skipped.push((
                        set.path.clone(),
                        format!(
                            "content is current, but its metadata could not be reproduced: {e}"
                        ),
                    ));
                } else {
                    report.retouched += 1;
                }
                // The hash just proved the file; anchor the record to the
                // stat the pass leaves behind (a retouch moved the mtime).
                // This is also what graduates a racily-clean record: once the
                // proof is comfortably newer than the mtime it vouches for,
                // later passes trust the stat and skip this hash.
                note_record(node, &target, content);
                continue;
            }

            // The file is not the selected version. A file that is not there
            // proves nothing, and whatever was recorded about it is stale.
            if on_disk.is_none() {
                node.forget_checkout_write(&target);
            }

            wanted.push(WantedContent {
                path: set.path.clone(),
                target,
                content,
                size: selected.size,
                meta,
            });
        }
    }

    Ok(CheckoutPass {
        report,
        known,
        wanted,
    })
}

/// How many bytes of an object a set of chunk groups covers.
///
/// Counted in bytes rather than groups, and clamped to the object, so the tail
/// group of a 100-byte file does not report 16 KiB. Shared with `synch adopt tree`
/// (adopt_tree.rs), which reports the same reused-versus-fetched pair.
pub(crate) fn bytes_of(groups: &ChunkRanges, size: u64) -> u64 {
    groups
        .ranges
        .iter()
        .map(|r| {
            let end = r.end.saturating_mul(synch_core::CHUNK_GROUP_SIZE).min(size);
            end.saturating_sub(r.start.saturating_mul(synch_core::CHUNK_GROUP_SIZE))
        })
        .sum::<u64>()
}

/// Writes a symbolic link into a checkout, or explains why it could not be.
///
/// §7.2 has a checkout follow the version its policy selects, and a symlink's
/// version *is* its target — so on a platform with symbolic links the checkout
/// writes a real one. Returns whether anything changed on disk.
///
/// The link's own metadata is not reproduced: its mode is meaningless on every
/// unix that matters, and stamping a link's times without following it needs
/// `utimensat(AT_SYMLINK_NOFOLLOW)`, which `std` does not expose. Following the
/// link instead would stamp whatever it points at — including a file outside
/// the checkout — which is precisely what this module refuses to do everywhere
/// else.
///
/// Windows has symlinks too, but creating one needs either Developer Mode or
/// `SeCreateSymbolicLinkPrivilege`, which a background daemon cannot assume and
/// cannot usefully acquire. Materialization's rule there is the one it already
/// applies to names the platform refuses: skip and report, never guess (§7.2) —
/// writing the target's *contents* under the link's name would silently turn a
/// link into a file and hand the next scanner on that machine a change nobody
/// made.
pub(crate) fn materialize_symlink(
    target: &Path,
    link_target: Option<&str>,
) -> std::result::Result<bool, String> {
    let Some(link_target) = link_target else {
        return Err("symlink entry carries no target".into());
    };
    #[cfg(unix)]
    {
        if let Ok(existing) = std::fs::read_link(target) {
            if existing.to_string_lossy() == link_target {
                return Ok(false);
            }
        }
        if target.symlink_metadata().is_ok() {
            std::fs::remove_file(target).map_err(|e| e.to_string())?;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::os::unix::fs::symlink(link_target, target).map_err(|e| e.to_string())?;
        Ok(true)
    }
    #[cfg(not(unix))]
    {
        let _ = target;
        Err(format!(
            "symlink to {link_target}: creating symbolic links is not available to the daemon on \
             this platform, so the path is skipped rather than written as a plain file"
        ))
    }
}

/// The metadata a checkout reproduces alongside a file's bytes (§4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Metadata {
    /// The origin's observed mtime, in unix nanoseconds.
    mtime_ns: i64,
    /// The origin's advisory unix mode, or `None` where it published none —
    /// a Windows origin, or a row materialized before the view carried the
    /// column. Unknown means "leave the mode alone", never "reset it".
    unix_mode: Option<u32>,
}

impl Metadata {
    pub(crate) fn of(entry: &EntryRow) -> Metadata {
        Metadata {
            mtime_ns: entry.mtime_ns,
            unix_mode: entry.unix_mode,
        }
    }
}

/// The bits of a published mode a checkout will reproduce.
///
/// The permission bits only: setuid, setgid and the sticky bit are deliberately
/// dropped. A checkout writes bytes a *peer* chose under a name that peer chose,
/// and the daemon may be running as root; reproducing a setuid bit would turn
/// "publish a file" into "plant a setuid binary in someone else's tree". §4.2
/// calls the mode advisory and best-effort, so declining the three bits that
/// grant authority costs nothing materialization promised.
const MODE_MASK: u32 = 0o777;

/// How far a stored timestamp may sit below the published one and still count
/// as that timestamp.
///
/// Filesystems coarsen: ext4 keeps nanoseconds, HFS+ whole seconds, FAT two.
/// Demanding exact equality would make every pass over such a filesystem
/// "repair" every file it had itself just stamped, forever. A stamp is only
/// ever coarsened downward, so a stored value inside this window below the
/// published one is the published one.
pub(crate) const MTIME_GRANULARITY_NS: i64 = 2_000_000_000;

/// True if `target` already carries the metadata `meta` describes.
///
/// An unreadable target answers "no" and the pass tries to stamp it, which
/// reports the real error rather than this guess.
fn metadata_matches(target: &Path, meta: Metadata) -> bool {
    let Ok(on_disk) = std::fs::metadata(target) else {
        return false;
    };
    let stored = crate::scanner::mtime_nanos(&on_disk);
    if !(0..MTIME_GRANULARITY_NS).contains(&meta.mtime_ns.saturating_sub(stored)) {
        return false;
    }
    #[cfg(unix)]
    if let Some(mode) = meta.unix_mode {
        use std::os::unix::fs::PermissionsExt;
        if on_disk.permissions().mode() & MODE_MASK != mode & MODE_MASK {
            return false;
        }
    }
    true
}

/// Stamps an entry's metadata onto a materialized file.
///
/// Times first and mode second, with owner-write restored while the stamp
/// happens: setting a file's times needs a writable descriptor, so a file whose
/// published mode is read-only — `0444` is an ordinary mode for published media
/// — cannot be stamped once its own mode has been applied, and a pass that got
/// the mode right would break the pass after it.
pub(crate) fn apply_metadata(target: &Path, meta: Metadata) -> std::io::Result<()> {
    #[cfg(unix)]
    let stamped = {
        use std::os::unix::fs::PermissionsExt;
        let current = std::fs::metadata(target)?.permissions().mode() & 0o7777;
        let desired = meta.unix_mode.map(|m| m & MODE_MASK).unwrap_or(current);
        let borrowed_write = current & 0o200 == 0;
        if borrowed_write {
            std::fs::set_permissions(target, std::fs::Permissions::from_mode(current | 0o200))?;
        }
        // Held rather than propagated, so a failed stamp cannot leave the file
        // wearing the write bit this function lent it.
        let stamped = set_modified(target, meta.mtime_ns);
        if desired != current || borrowed_write {
            std::fs::set_permissions(target, std::fs::Permissions::from_mode(desired))?;
        }
        stamped
    };
    #[cfg(not(unix))]
    let stamped = set_modified(target, meta.mtime_ns);
    stamped
}

/// Sets a file's modification time to a unix-nanosecond stamp.
fn set_modified(target: &Path, mtime_ns: i64) -> std::io::Result<()> {
    let times = std::fs::FileTimes::new().set_modified(system_time(mtime_ns));
    std::fs::File::options()
        .write(true)
        .open(target)?
        .set_times(times)
}

/// A unix-nanosecond stamp as a [`SystemTime`], pre-epoch stamps included.
fn system_time(ns: i64) -> SystemTime {
    if ns >= 0 {
        UNIX_EPOCH + Duration::from_nanos(ns as u64)
    } else {
        UNIX_EPOCH - Duration::from_nanos(ns.unsigned_abs())
    }
}

/// The content root of the file at `target`, if it is the right length to be
/// the version being materialized.
///
/// This is the currency check's evidence of last resort, asked only of paths
/// no record vouches for — a daemon restart, a moved stat, a file the checkout
/// has never seen. Size first, because it settles almost every case for the
/// price of a `stat` — and a file of the wrong length is not hashed here at
/// all, because no hash of it could answer yes.
///
/// Streamed, because a checkout carries objects far larger than memory.
/// Anything unreadable answers `None`, and the pass rewrites it.
///
/// The root rather than a bare "current?": naming the object that is on the
/// disk is also what tells the pass whether the file is a version the descent
/// wanted and could not find (`docs/DELTA-SYNC.md` §3.2).
fn same_size_root(target: &Path, wanted_size: u64) -> Option<synch_core::Hash> {
    let metadata = std::fs::metadata(target).ok().filter(|m| m.is_file())?;
    if metadata.len() != wanted_size {
        return None;
    }
    hash_file(target)
}

/// True while a record and the stat describe the same file: same object,
/// same length, same stored mtime, same platform identity — and the record
/// is comfortably newer than that mtime, so no same-size in-place rewrite
/// can have shared the stamp (the scanner's racy window, scanner.rs).
/// Anything less and the file speaks for itself, hashed on this pass.
fn still_current(
    record: &CheckoutWrite,
    stat: &std::fs::Metadata,
    content: &synch_core::Hash,
) -> bool {
    record.content == *content
        && record.size == stat.len()
        && record.mtime_ns == crate::scanner::mtime_nanos(stat)
        && record.file_id == crate::scanner::file_identity(stat)
        && record.recorded_at.saturating_sub(record.mtime_ns) >= crate::scanner::RACY_WINDOW_NS
}

/// Anchors a target's record to the stat currently on disk, or drops the
/// record if the file is already gone.
fn note_record(node: &Node, target: &Path, content: synch_core::Hash) {
    match CheckoutWrite::of(target, content) {
        Some(record) => node.note_checkout_write(target, record),
        None => node.forget_checkout_write(target),
    }
}

/// Streams a file through BLAKE3 for its content root.
fn hash_file(target: &Path) -> Option<synch_core::Hash> {
    std::fs::File::open(target)
        .ok()
        .and_then(|file| synch_core::hash_reader(std::io::BufReader::new(file)).ok())
}

/// Identifies a donor the CAS has lost but the checkout is still sitting on
/// (`docs/DELTA-SYNC.md` §3.2).
/// Returns true if any ancestor of `rel` under `root` is a symlink, so that
/// writing or deleting at `root/rel` would resolve through it and escape the
/// root. The final component (the target itself) is not an ancestor and is
/// allowed to be a symlink.
///
/// Used by the checkout loop and by [`Node::adoption_target`](crate::Node): a
/// space root is canonicalized when it is added, but nothing canonicalizes its
/// *interior*, and a symlinked directory inside a space is an ordinary thing
/// for a user to have. Without this a client that can name a path can write and
/// delete through it, anywhere the daemon's uid reaches.
pub(crate) fn escapes_via_symlink(root: &Path, rel: &str) -> bool {
    let mut cur = root.to_path_buf();
    let mut components: Vec<&str> = rel.split('/').filter(|c| !c.is_empty()).collect();
    components.pop();
    for component in components {
        cur.push(component);
        match std::fs::symlink_metadata(&cur) {
            Ok(meta) if meta.file_type().is_symlink() => return true,
            _ => {}
        }
    }
    false
}

fn remove_if_present(target: &Path) -> Result<usize> {
    if target.is_file() || target.is_symlink() {
        std::fs::remove_file(target)?;
        return Ok(1);
    }
    Ok(0)
}

/// Removes files under a checkout root whose path the unified tree no longer
/// carries, and returns the targets that went.
fn sweep(root: &Path, dir: &Path, known: &HashSet<String>) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut removed = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        // `is_dir` follows links, and a materialized symlink pointing at a
        // directory would then be descended into and its *contents* swept.
        // The sweep only ever looks at what the checkout itself wrote.
        let is_dir = std::fs::symlink_metadata(&path)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        if is_dir {
            removed.extend(sweep(root, &path, known)?);
            // Remove an emptied directory so a later pass can materialize a
            // non-directory at the same path. Non-empty directories fail safely.
            let _ = std::fs::remove_dir(&path);
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let relative = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if !known.contains(&relative) {
            std::fs::remove_file(&path)?;
            removed.push(path);
        }
    }
    Ok(removed)
}

/// Names Windows refuses, plus trailing dots and spaces, plus reserved
/// characters. Checked on every platform so a checkout behaves identically
/// everywhere (§7.2).
pub(crate) fn unsafe_name(path: &str) -> Option<String> {
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    for component in path.split('/') {
        if component.ends_with('.') || component.ends_with(' ') {
            return Some(format!("component {component:?} ends with a dot or space"));
        }
        if component
            .chars()
            .any(|c| matches!(c, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*'))
        {
            return Some(format!("component {component:?} has a reserved character"));
        }
        let stem = component.split('.').next().unwrap_or(component);
        if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
            return Some(format!("component {component:?} is a reserved device name"));
        }
    }
    None
}

/// Folds a path the way a case-insensitive, normalizing filesystem would.
fn fold(path: &str) -> String {
    path.to_lowercase()
}

/// Claims `path`'s folded name, or reports the path that already holds it.
///
/// The §7.2 first-claimant-wins rule — two published paths that fold onto one
/// local name are not both materializable, and without the claim one would be
/// silently written over the other. One implementation, shared by the checkout
/// and tree adoption, because it is a policy rule that must not be able to differ
/// between them. `Err` carries the skip reason for the caller's report.
pub(crate) fn claim_folded_name(
    claimed: &mut HashMap<String, String>,
    path: &str,
) -> std::result::Result<(), String> {
    let folded = fold(path);
    match claimed.get(&folded) {
        Some(winner) if winner != path => Err(format!(
            "collides with {winner} under filesystem name folding"
        )),
        _ => {
            claimed.insert(folded, path.to_string());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use synch_core::{FileEntry, OriginId};
    use synch_store::ReplicaPolicy;

    #[tokio::test]
    async fn checkout_materializes_only_replica_held_content_and_follows_deletion() {
        let (_data, node) = crate::testkit::node().await;
        let target = tempfile::tempdir().unwrap();
        node.add_replica(
            "media",
            ReplicaPolicy::Current,
            Some(0),
            None,
            Some(target.path().to_string_lossy().into_owned()),
        )
        .unwrap();

        let origin = OriginId::named("nas", "x.example").unwrap();
        let content = b"held by the replica";
        let root = node.store().ingest_bytes(content, 1).unwrap();
        node.store()
            .put_entry(
                &origin,
                "media",
                "movie.txt",
                &FileEntry::file(content.len() as u64, 1, root, 1),
            )
            .unwrap();

        let report = node.sync_checkout("media").await.unwrap();
        assert_eq!(report.written, 0);
        assert_eq!(report.skipped.len(), 1);
        assert!(!target.path().join("movie.txt").exists());

        let holder = node.store().replica("media").unwrap().unwrap().holder();
        node.store().pin(&root, &holder, 2).unwrap();
        let report = node.sync_checkout("media").await.unwrap();
        assert_eq!(report.written, 1, "{report:?}");
        assert_eq!(
            std::fs::read(target.path().join("movie.txt")).unwrap(),
            content
        );

        node.store()
            .put_entry(
                &origin,
                "media",
                "movie.txt",
                &FileEntry::tombstone(2, 2, Some(root)),
            )
            .unwrap();
        let report = node.sync_checkout("media").await.unwrap();
        assert_eq!(report.removed, 1, "{report:?}");
        assert!(!target.path().join("movie.txt").exists());
        node.shutdown().await.unwrap();
    }
}
