//! The indexing pipeline for this node's sources (§7.1).
//!
//! A file is considered unchanged if `(size, mtime_ns, file_id)` matches the
//! `local_files` table — only then is hashing skipped. Changed files are
//! re-hashed with streaming BLAKE3, the outboard falls out as a by-product, the
//! CAS is updated, and a new `FileEntry` is staged.
//!
//! Correctness never depends on watcher completeness: the scanner is the source
//! of truth and the watcher only schedules it.

use std::path::{Path, PathBuf};

use synch_core::{EntryKind, FileEntry, Hash, blob_key, file_key, normalize_native_path, now_ns};
use synch_mpt::Trie;
use synch_store::LocalFile;

use crate::{
    error::{EngineError, Result},
    ignore::IgnoreSet,
    node::{Node, StagedChange},
};

/// How far past a file's mtime its hash must have been taken before the
/// `(size, mtime_ns, file_id)` stat check is proof of "unchanged".
///
/// Two seconds covers every timestamp granularity in practice: nanoseconds
/// where the kernel grants them, a scheduler tick (1–10 ms) where it uses the
/// coarse clock, and the full-second stamps of older filesystems. Inside the
/// window a same-size in-place rewrite can share the hashed write's mtime,
/// so the stat proves nothing and the bytes are hashed again. Checkout
/// reconciliation trusts a verified record under the same window.
pub(crate) const RACY_WINDOW_NS: i64 = 2_000_000_000;

/// What one scan found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// Paths restated because they looked changed: files whose bytes were
    /// re-hashed, and symlinks whose target or mtime moved.
    pub hashed: usize,
    /// Files skipped because `(size, mtime_ns, file_id)` matched.
    pub unchanged: usize,
    /// Paths that disappeared and were tombstoned.
    pub deleted: usize,
    /// Paths skipped by ignore rules.
    pub ignored: usize,
    /// Tombstones dropped because they outlived `tombstone_ttl` (§4.2).
    pub expired: usize,
    /// Paths skipped because they could not be indexed, with the reason.
    pub skipped: Vec<(String, String)>,
    /// The changes to publish.
    pub staged: Vec<StagedChange>,
}

impl ScanReport {
    /// True if the scan produced nothing to publish.
    pub fn is_empty(&self) -> bool {
        self.staged.is_empty()
    }

    fn merge(&mut self, other: ScanReport) {
        self.hashed += other.hashed;
        self.unchanged += other.unchanged;
        self.deleted += other.deleted;
        self.ignored += other.ignored;
        self.expired += other.expired;
        self.skipped.extend(other.skipped);
        self.staged.extend(other.staged);
    }
}

/// Re-indexes local paths against `origin` without requiring a bound node.
///
/// Node opening runs this before it creates an endpoint, so every await after
/// an endpoint exists transfers that endpoint directly into its owning Node.
pub(crate) fn reconcile_local_files_in(
    store: &synch_store::Store,
    origin: &synch_core::OriginId,
) -> Result<usize> {
    let root = store
        .complete_head(origin)?
        .map(|head| head.root)
        .unwrap_or(Hash::EMPTY);
    let trie = Trie::new(store);
    let mut dropped = 0;
    for space in store.sources()? {
        if space.local_path.is_none() {
            continue;
        }
        for row in store.local_file_rows(&space.space)? {
            let published = trie
                .get(root, &file_key(&space.space, &row.relpath)?)?
                .as_deref()
                .map(decode_entry)
                .transpose()?
                .and_then(|entry| published_signal(&entry));
            if published == row.content {
                continue;
            }
            tracing::debug!(
                space = %space.space,
                path = %row.relpath,
                "re-indexing a path the published root does not corroborate"
            );
            store.remove_local_file(&space.space, &row.relpath)?;
            dropped += 1;
        }
    }
    Ok(dropped)
}

impl Node {
    /// Whether a configured space deliberately has no local checkout.
    pub fn is_api_source(&self, space_id: &str) -> Result<bool> {
        self.store()
            .source(space_id)?
            .map(|space| space.local_path.is_none())
            .ok_or_else(|| EngineError::not_found(format!("space {space_id}")))
    }

    fn scan_space_with_ingest(
        &self,
        space_id: &str,
        ingest: &mut impl FnMut(&Path) -> Result<(Hash, u64)>,
    ) -> Result<ScanReport> {
        let space = self
            .store()
            .source(space_id)?
            .ok_or_else(|| EngineError::not_found(format!("space {space_id}")))?;
        let local_path = space.local_path.as_deref().ok_or_else(|| {
            EngineError::invalid(format!(
                "source {space_id} is API-only and cannot be scanned"
            ))
        })?;
        let root_dir = PathBuf::from(local_path);
        let seq = self.next_seq()?;

        // A vanished space root — an unmounted drive, a renamed mount, a
        // directory momentarily gone — must not read as "every file deleted".
        // `walk` treats a missing directory as empty, so without this guard the
        // deletion sweep below would tombstone the whole space and publish that
        // cluster-wide. Refuse to scan a root that is absent or not a directory.
        match std::fs::metadata(&root_dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(EngineError::invalid(format!(
                    "space {space_id} root {} is not a directory",
                    root_dir.display()
                )));
            }
            Err(e) => {
                return Err(EngineError::invalid(format!(
                    "space {space_id} root {} is unavailable: {e}",
                    root_dir.display()
                )));
            }
        }

        // After the root guard, not before. `.syncignore` under a root that is a
        // regular file returns `ENOTDIR`, which is not `NotFound`, so reading it
        // first answered "the ignore file exists but could not be read" for a
        // space whose actual problem the guard above names plainly.
        let ignore = IgnoreSet::for_space(&root_dir)?;
        let mut report = ScanReport::default();
        let mut found = Vec::new();
        walk(&root_dir, &root_dir, &ignore, &mut report, &mut found)?;

        // A set, not a list: the deletion sweep below asks "did the walk see
        // this?" once per row the scanner has ever recorded, and a linear
        // membership test makes a scan quadratic in the size of the space —
        // seconds of pure comparison on the 40 000-entry tree §4 uses as its
        // working example, and worse than the hashing on anything larger.
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for candidate in &found {
            let (_, rel, _) = candidate;
            // A path that vanished between the walk and here is skipped, not
            // fatal — the same tolerance `walk` above already applies to
            // `read_dir` and `symlink_metadata`, and for the same reason: a
            // live directory (a build tree, a browser profile) produces this
            // routinely.
            //
            // It stopped one function short of where it was needed, and the
            // consequence was not "one file missed". `index_file` commits its
            // `local_files` row as it goes and the batch it stages is published
            // only at the end, so an abort left every path indexed before it
            // durably marked as scanned and never published — and the next scan
            // reads that row, finds the stat matches, and reports the file
            // `unchanged`. The files stayed unpublished until the process was
            // restarted.
            //
            // Only failures about *this path* are tolerated. A store failure is
            // not one, and swallowing it would silently drop the file the same
            // way.
            match self.index_file(space_id, candidate, seq, &mut report, ingest) {
                Ok(()) => {}
                Err(EngineError::Io(e)) => {
                    report.skipped.push((rel.clone(), e.to_string()));
                }
                Err(e) => return Err(e),
            }
            seen.insert(rel.as_str());
        }

        // Anything the scanner previously recorded but did not see is gone.
        // Deletion propagates by the key vanishing from the new root; the
        // tombstone exists so `synch status`/`synch log` can tell "deleted at
        // seq N" from "never existed" (§4.2).
        //
        // Use the union of what the scanner recorded and what this origin has
        // published, not `local_files` alone. A staged tombstone removes the
        // local row before publication, while the published tree stays durable.
        // The published tree is durable and `local_files` is not, so the
        // published tree is what the sweep is anchored to; `local_files` still
        // contributes the paths this scan indexed but has not published yet.
        // A path the walk could not judge is not a path that is gone.
        //
        // `seen` is built from what the walk actually stat'd, so every path it
        // skipped — a child whose `symlink_metadata` failed `EACCES` because the
        // parent lost its execute bit, an `EIO` on a network space — was absent
        // from `seen` and therefore swept. The scan then reported the same path
        // in `skipped` *and* published a tombstone for it: checkouts deleted their
        // copies, GC became free to drop the origin's own object, and a later
        // successful scan republished it as a new entry with no `prev`, so every
        // peer re-fetched bytes it already had.
        //
        // The asymmetry is what gives it away: `index_file`'s failures are
        // tolerated *and* the path stays in `seen` a few lines above, for exactly
        // this reason. `walk`'s failures land in the same field and got the
        // opposite treatment.
        //
        // As a prefix, not an exact name. `walk` stats a `DirEntry` before it can
        // know whether it is a directory, so a *directory* it cannot stat is
        // skipped under its own path and never recursed into — and the published
        // paths at risk are the files beneath it, which no exact-match exemption
        // reaches.
        let unjudged: Vec<String> = report
            .skipped
            .iter()
            .map(|(path, _)| path.clone())
            .collect();

        let mut known_paths: Vec<String> = self.store().local_files(space_id)?;
        known_paths.extend(self.store().published_paths(self.origin(), space_id)?);
        known_paths.sort();
        known_paths.dedup();

        for known in known_paths {
            if seen.contains(known.as_str()) || unjudged_covers(&unjudged, &known) {
                continue;
            }
            let prev = self
                .store()
                .entry(self.origin(), space_id, &known)?
                .and_then(|e| e.content);
            let tombstone = FileEntry::tombstone(now_ns(), seq, prev);
            report.staged.push((
                file_key(space_id, &known)?,
                Some(synch_core::record::encode(&tombstone)?),
            ));
            self.store().remove_local_file(space_id, &known)?;
            report.deleted += 1;
        }
        Ok(report)
    }

    /// Indexes one symbolic link (§7.1).
    ///
    /// Symlinks are tracked exactly like files: a `local_files` row carrying
    /// the link's own (lstat) mtime and its target, so an unchanged symlink
    /// stages nothing. Republishing one every scan would defeat the property
    /// that an unchanged tree publishes no head — and, worse, leaving the row
    /// out would hide the path from the deletion sweep, so a removed symlink
    /// would never be tombstoned and would stay published forever.
    ///
    /// The target is the change signal, and it is carried in the row's
    /// `content` column as `blake3(target)`: content-addressing a link's
    /// target is exactly what that column already means for a file, and it
    /// costs no schema change. The staged entry keeps the link's real mtime,
    /// never `now_ns()`, so the §8 `newest` order compares stable values and a
    /// file-versus-symlink divergence resolves the same way on every node.
    fn index_symlink(
        &self,
        space_id: &str,
        path: &Path,
        rel: &str,
        seq: u64,
        report: &mut ScanReport,
    ) -> Result<()> {
        let metadata = std::fs::symlink_metadata(path)?;
        let mtime_ns = mtime_nanos(&metadata);
        let file_id = file_identity(&metadata);
        let target = std::fs::read_link(path)?.to_string_lossy().into_owned();
        let signal = symlink_signal(&target);
        let size = target.len() as u64;

        let known = self.store().local_file(space_id, rel)?;
        let unchanged = match &known {
            Some(known) => {
                known.content == Some(signal)
                    && known.mtime_ns == mtime_ns
                    && known.file_id == file_id
            }
            None => false,
        };
        if unchanged {
            report.unchanged += 1;
            return Ok(());
        }

        let mut entry = FileEntry::tombstone(mtime_ns, seq, None);
        entry.kind = EntryKind::Symlink;
        entry.symlink_target = Some(target);
        entry.size = size;
        report.staged.push((
            file_key(space_id, rel)?,
            Some(synch_core::record::encode(&entry)?),
        ));
        report.hashed += 1;

        self.store().put_local_file(&LocalFile {
            space: space_id.to_string(),
            relpath: rel.to_string(),
            size,
            mtime_ns,
            file_id,
            content: Some(signal),
            scanned_at: now_ns(),
        })?;
        Ok(())
    }

    fn index_file(
        &self,
        space_id: &str,
        candidate: &(PathBuf, String, bool),
        seq: u64,
        report: &mut ScanReport,
        ingest: &mut impl FnMut(&Path) -> Result<(Hash, u64)>,
    ) -> Result<()> {
        let (path, rel, is_symlink) = candidate;
        if *is_symlink {
            return self.index_symlink(space_id, path, rel, seq, report);
        }

        let metadata = std::fs::metadata(path)?;
        if metadata.is_dir() {
            let mtime_ns = mtime_nanos(&metadata);
            let mode = unix_mode(&metadata);
            if self
                .store()
                .entry(self.origin(), space_id, rel)?
                .is_some_and(|entry| {
                    entry.kind == EntryKind::Dir
                        && entry.mtime_ns == mtime_ns
                        && entry.unix_mode == mode
                })
            {
                report.unchanged += 1;
                return Ok(());
            }
            let mut entry = FileEntry::tombstone(mtime_ns, seq, None);
            entry.kind = EntryKind::Dir;
            entry.unix_mode = mode;
            report.staged.push((
                file_key(space_id, rel)?,
                Some(synch_core::record::encode(&entry)?),
            ));
            self.store().put_local_file(&LocalFile {
                space: space_id.into(),
                relpath: rel.into(),
                size: 0,
                mtime_ns,
                file_id: file_identity(&metadata),
                content: None,
                scanned_at: now_ns(),
            })?;
            report.hashed += 1;
            return Ok(());
        }
        let size = metadata.len();
        let mtime_ns = mtime_nanos(&metadata);
        let file_id = file_identity(&metadata);

        let known = self.store().local_file(space_id, rel)?;
        let stat_match = match &known {
            Some(known) => {
                known.size == size && known.mtime_ns == mtime_ns && known.file_id == file_id
            }
            None => false,
        };
        // A matching stat only proves "unchanged" once the hash it vouches for
        // was taken comfortably after the file's mtime. Filesystem timestamps
        // are granular — a jiffy on most Linux configurations, a full second
        // on older filesystems — so a same-size in-place rewrite landing
        // within one tick of the write we hashed is invisible to the stat.
        // Until the record has aged past that window, the stat is racily
        // clean (git's term for the same hazard) and the content must speak
        // for itself.
        let trusted = match &known {
            Some(known) => known.scanned_at.saturating_sub(known.mtime_ns) >= RACY_WINDOW_NS,
            None => false,
        };
        if stat_match && trusted {
            report.unchanged += 1;
            return Ok(());
        }

        let (content, size) = ingest(path)?;

        if stat_match && known.as_ref().is_some_and(|k| k.content == Some(content)) {
            // Racily clean and actually clean. Refreshing `scanned_at` is what
            // lets the stat become trustworthy: once it is two seconds past
            // the mtime it vouches for, no unnoticed rewrite can share that
            // mtime, and the next scan skips the hash again.
            self.store().put_local_file(&LocalFile {
                space: space_id.to_string(),
                relpath: rel.to_string(),
                size,
                mtime_ns,
                file_id,
                content: Some(content),
                scanned_at: now_ns(),
            })?;
            report.unchanged += 1;
            return Ok(());
        }
        // Counted only once the content actually differs: a racy re-hash that
        // came back identical is "unchanged" to the operator, and reporting it
        // as both hashed and unchanged made a no-op scan read as work.
        report.hashed += 1;

        let previous = self
            .store()
            .entry(self.origin(), space_id, rel)?
            .and_then(|e| e.content);
        // `prev` records one-step lineage so a UI can tell "adopted theirs on
        // top of X" from "changed independently" (§8).
        // The kind comes from a *local* declaration, never from a peer
        // (`docs/SOCKETS.md` §2.2). That is what makes `synch adopt path` of
        // someone's socket adopt its bytes and not its socket-ness: this node
        // publishes what it has declared, and it has declared nothing about a
        // path it merely received.
        let activated_socket = self.store().is_activated_socket(space_id, rel)?;
        let mut entry = if activated_socket {
            FileEntry::socket(size, mtime_ns, content, seq)
        } else {
            FileEntry::file(size, mtime_ns, content, seq)
        };
        entry.prev = previous.filter(|p| *p != content);
        entry.unix_mode = unix_mode(&metadata);

        // A content change under an activated path is a deployment: it
        // publishes like any other change, and the only thing that resets is
        // the per-socket map the old program minted.
        if activated_socket && previous.is_some_and(|p| p != content) {
            self.socket_content_deployed(space_id, rel, &content);
        }

        report.staged.push((
            file_key(space_id, rel)?,
            Some(synch_core::record::encode(&entry)?),
        ));
        if let Some(ad) = self.store().local_ad(&content)? {
            report
                .staged
                .push((blob_key(&content), Some(synch_core::record::encode(&ad)?)));
        }

        self.store().put_local_file(&LocalFile {
            space: space_id.to_string(),
            relpath: rel.to_string(),
            size,
            mtime_ns,
            file_id,
            content: Some(content),
            scanned_at: now_ns(),
        })?;
        Ok(())
    }

    /// Scans every configured space.
    pub(crate) fn scan_all(&self) -> Result<ScanReport> {
        if self.cas_backend().remote_upload_parts() {
            return Err(EngineError::invalid(
                "cloud-CAS scans must use the async backend-aware scan path",
            ));
        }
        let store = self.store().clone();
        self.scan_all_with_ingest(&mut |_, _| {}, &mut move |path| {
            Ok(store.ingest_file(path, now_ns())?)
        })
    }

    fn scan_all_with_ingest(
        &self,
        on_space: &mut impl FnMut(&str, &ScanReport),
        ingest: &mut impl FnMut(&Path) -> Result<(Hash, u64)>,
    ) -> Result<ScanReport> {
        let mut report = ScanReport::default();
        for space in self.store().sources()? {
            if space.local_path.is_none() {
                continue;
            }
            // One space's failure does not discard the others' work. It used
            // to: `scan_space_with_ingest` commits the `local_files` row removal for a
            // vanished path as it goes (the tombstone that replaces it only
            // enters `report.staged`), so a later space failing — an unmounted
            // removable disk is the ordinary case — dropped the tombstone while
            // the removal stayed committed. The path then existed in no source
            // of truth this node consults: not on disk, not in `local_files`,
            // so no later scan could re-derive it, and our origin kept
            // publishing it as a live file at its old content root forever.
            //
            // Reported rather than swallowed: the space appears in `skipped`,
            // which is what `synch source scan` prints and what `doctor` reads.
            match self.scan_space_with_ingest(&space.space, ingest) {
                Ok(one) => {
                    on_space(&space.space, &one);
                    report.merge(one);
                }
                Err(e) => {
                    tracing::warn!(space = %space.space, error = %e, "space skipped by this scan");
                    report.skipped.push((space.space.clone(), e.to_string()));
                }
            }
        }
        // A scan is also where an operator can force tombstone expiry (§4.2):
        // the removals ride the same batch as everything else the scan found.
        //
        // Never for a key this scan has already restated. `expired_tombstones`
        // reads `entries` as it stood *before* the scan, so a path whose
        // tombstone has aged out and which now exists on disk again appears in
        // both halves — the scan's live entry first, then a raw removal for the
        // same key. `publish` folds in order, so the removal won, and the origin
        // published nothing at all for the path: indistinguishable from "never
        // existed", gone from the unified tree and from every checkout. Worse, it
        // was self-perpetuating — `local_files` had already recorded the new
        // content, so every later scan called the path unchanged, and the
        // tombstone row the expiry deleted no longer showed up to be retired.
        // Only a restart repaired it, through `reconcile_local_files`.
        let expired = self.expired_tombstone_changes()?;
        let restated: std::collections::HashSet<&[u8]> = report
            .staged
            .iter()
            .map(|(key, _)| key.as_slice())
            .collect();
        let expired: Vec<StagedChange> = expired
            .into_iter()
            .filter(|(key, _)| !restated.contains(key.as_slice()))
            .collect();
        report.expired = expired.len();
        report.staged.extend(expired);
        if !report.staged.is_empty() {
            report.staged.push(self.manifest_change()?);
            // One record per space rather than a list inside the manifest, so
            // that what this node advertises about a space can be shown to a
            // peer delegated that space and to nobody else (§5.5).
            report.staged.extend(self.space_info_changes()?);
            // Coverage claims ride along with a publish this node is making
            // anyway, so their counts refresh without costing a head of their
            // own (`docs/REPLICATION.md` §4.1).
            report.staged.extend(self.replica_claim_changes()?);
        }
        Ok(report)
    }

    /// The trie-key removals that retire this node's aged-out tombstones
    /// (§4.2).
    ///
    /// A tombstone says "deleted at seq N" rather than "never existed", which
    /// is worth carrying for a while and not forever: after `tombstone_ttl`
    /// (default 90 days) the key is removed from a later root and the path goes
    /// back to reading as absent. Only this node's own tombstones are ever
    /// considered — a replicated trie belongs to its origin, and this node
    /// cannot rewrite it.
    pub(crate) fn expired_tombstone_changes(&self) -> Result<Vec<StagedChange>> {
        let ttl = self.config().tombstone_ttl.as_nanos().min(i64::MAX as u128) as i64;
        let cutoff = now_ns().saturating_sub(ttl);
        let mut changes = Vec::new();
        for row in self.store().expired_tombstones(self.origin(), cutoff)? {
            changes.push((file_key(&row.space, &row.path)?, None));
        }
        Ok(changes)
    }

    /// Reconciles published `b:` records with what this node actually holds
    /// (§6.3).
    ///
    /// An availability ad has to be retired explicitly. The scanner stages
    /// `blob_key(content) -> Some(ad)` on every hash and stages only `f:` keys
    /// as `None`, so without this every content root this origin has ever
    /// published would stay a leaf in its trie for good — replicated to every
    /// member, pinned against trie GC by the head that reaches it, and
    /// accumulating one leaf per edit per file forever.
    ///
    /// It is a correctness problem as much as a growth one: `gc_content`
    /// deletes a local payload once no entry references it, while a standing ad
    /// goes on telling every peer this node holds the object, so peers keep
    /// selecting it as a provider and keep failing.
    ///
    /// Loss detection can also demote a complete local row to a partial one
    /// while the last published head still says complete. Replacing that record
    /// is as important as deleting an ad after GC: until this pass lands, peers
    /// can select a provider which has already proved it cannot serve the bytes.
    /// Reconciliation is staged, so any number of corrections costs one head.
    pub(crate) fn ad_reconciliation_changes(&self) -> Result<Vec<StagedChange>> {
        let mut changes = Vec::new();
        for root in self.store().provider_roots_for_origin(self.origin())? {
            let published = self.store().provider_for_origin(&root, self.origin())?;
            match self.store().local_ad(&root)? {
                Some(local) if published.as_ref() != Some(&local) => {
                    changes.push((blob_key(&root), Some(synch_core::record::encode(&local)?)))
                }
                Some(_) => {}
                None => changes.push((blob_key(&root), None)),
            }
        }
        Ok(changes)
    }

    /// Stages corrected ads for objects whose local availability changed.
    ///
    /// Returns how many were staged.
    pub(crate) fn reconcile_ads(&self) -> Result<usize> {
        let changes = self.ad_reconciliation_changes()?;
        let corrected = changes.len();
        if corrected > 0 {
            self.stage(changes);
            tracing::info!(corrected, "staging corrected availability advertisements");
        }
        Ok(corrected)
    }

    /// Stages the removal of this node's aged-out tombstones (§4.2).
    ///
    /// Staged rather than published: expiry flows through the ordinary
    /// publisher, so it costs one head like any other batch. Returns how many
    /// tombstones were staged for removal.
    pub(crate) fn expire_tombstones(&self) -> Result<usize> {
        // Excluding whatever is already waiting to be published, for the reason
        // `scan_all_with_ingest` gives: a removal that lands in the same batch as a
        // live entry for the same key erases the path outright. Here the two are
        // not even ordered — the scanner and this pass stage into one buffer
        // concurrently — so the filter is what makes the outcome defined.
        let staged = self.publisher().staged_keys();
        let changes: Vec<StagedChange> = self
            .expired_tombstone_changes()?
            .into_iter()
            .filter(|(key, _)| !staged.contains(key))
            .collect();
        let expired = changes.len();
        if expired > 0 {
            self.stage(changes);
            tracing::info!(expired, "staging expired tombstones for removal");
        }
        Ok(expired)
    }

    /// Scans every space and publishes the result as one new root, without
    /// going through the publisher's batch.
    ///
    /// The recovery gate (§3.4) is checked *before* the scan, not only at the
    /// publish: a scan records what it hashed in `local_files`, so a scan whose
    /// publish is refused would leave the node believing it had already
    /// published files it never did.
    pub fn scan_and_publish(&self) -> Result<(ScanReport, Option<synch_core::SignedHead>)> {
        self.ensure_publishable()?;
        let report = self.scan_all()?;
        let head = self.publish(&report.staged)?;
        Ok((report, head))
    }

    /// Scans every space and stages the result for the publisher (§7.1), on
    /// the blocking pool.
    ///
    /// This is what a watcher hint and the periodic rescan use: a burst of
    /// saves becomes one batch and therefore one head. Nothing is published
    /// until the batch flushes. The recovery gate is taken here for the same
    /// reason it is taken in [`Node::scan_and_publish`] — the scan writes
    /// `local_files` either way.
    ///
    /// Always off the runtime. A scan walks every space,
    /// stats every path, and re-hashes whatever moved — work bounded by the
    /// size of the tree, not by anything the runtime can preempt. Run inline on
    /// a worker thread it stops that thread from polling for as long as it
    /// takes, which on a multi-gigabyte space is the daemon going quiet: no
    /// peer answered, no control request served, no timer fired on time (§10).
    pub(crate) async fn scan_and_stage_async(&self) -> Result<ScanReport> {
        Ok(self.scan_and_stage_async_with_reports().await?.0)
    }

    /// Backend-aware scan plus the per-space reports used by an explicit CLI
    /// scan's progress output.
    pub async fn scan_and_stage_async_with_reports(
        &self,
    ) -> Result<(ScanReport, Vec<(String, ScanReport)>)> {
        let node = self.clone();
        let backend = self.cas_backend().clone();
        crate::blocking::offload(move || {
            node.ensure_publishable()?;
            let runtime = tokio::runtime::Handle::current();
            let mut spaces = Vec::new();
            let report = node.scan_all_with_ingest(
                &mut |space, report| spaces.push((space.to_string(), report.clone())),
                &mut |path| {
                    let ingested =
                        runtime.block_on(backend.ingest_file(path.to_path_buf(), now_ns()))?;
                    Ok((ingested.root, ingested.size))
                },
            )?;
            node.stage(report.staged.iter().cloned());
            Ok((report, spaces))
        })
        .await
    }

    /// Scans one filesystem source and stages its changes.
    pub async fn scan_source_and_stage_async(&self, space: &str) -> Result<ScanReport> {
        let node = self.clone();
        let backend = self.cas_backend().clone();
        let space = space.to_string();
        crate::blocking::offload(move || {
            node.ensure_publishable()?;
            let runtime = tokio::runtime::Handle::current();
            let mut report = node.scan_space_with_ingest(&space, &mut |path| {
                let ingested =
                    runtime.block_on(backend.ingest_file(path.to_path_buf(), now_ns()))?;
                Ok((ingested.root, ingested.size))
            })?;
            node.stage(report.staged.iter().cloned());
            report.staged.clear();
            Ok(report)
        })
        .await
    }

    /// Re-indexes local paths whose staged changes never reached a root.
    ///
    /// A scan records `(size, mtime_ns, file_id)` in `local_files` as it
    /// indexes, and that record is what makes the *next* scan skip the file.
    /// Batching puts a window between the two: a daemon that dies with a batch
    /// still buffered would leave rows claiming a file is published while no
    /// root mentions it, and every later scan would skip it — silent, permanent
    /// drift.
    ///
    /// So on open, every `local_files` row is checked against this node's own
    /// current trie, and any row the trie does not corroborate is dropped: the
    /// next scan re-hashes that path and stages it again. Both tables are
    /// local, so the check costs one trie lookup per indexed file and no I/O
    /// beyond the database. Returns how many rows were dropped.
    pub fn reconcile_local_files(&self) -> Result<usize> {
        reconcile_local_files_in(self.store(), self.origin())
    }

    /// Adopts a peer's version of a path as our own (§8, `synch adopt path`).
    ///
    /// The bytes are written into the local filesystem-source directory and the ordinary
    /// indexing pipeline republishes them as this node's entry, with `prev`
    /// pointing at the content we replaced.
    pub fn adopt(&self, space_id: &str, path: &str, content: &[u8]) -> Result<PathBuf> {
        self.ensure_adoptable(space_id, path)?;
        let target = self.adoption_target(space_id, path)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, content)?;
        Ok(target)
    }

    /// Adopts one origin's version of a path, streaming it into the local
    /// filesystem-source directory (§8, `synch adopt path`).
    ///
    /// The bytes-in-hand [`Node::adopt`] is what a caller with a small payload
    /// uses; this is the form that never has the payload in hand. Adoption of a
    /// multi-gigabyte file would otherwise read the object into memory and hand
    /// the slice over — the object is fetched into the CAS either way, so the
    /// only thing that buffering buys is a copy the size of the file.
    pub async fn adopt_from(
        &self,
        origin: &synch_core::OriginId,
        space_id: &str,
        path: &str,
    ) -> Result<PathBuf> {
        let policy = synch_store::VersionPolicy::Origin(origin.clone());
        // Above the API-source branch, not inside the other one. An API-source
        // adoption writes no local file, but it crosses the network, promotes
        // the object to the backend's durable tier — a real, billable write on
        // a cloud CAS — and stages an `f:`/`b:` pair that a node in recovery
        // cannot publish: the buffer then re-fails on every quiesce tick and is
        // lost outright, durable blob orphaned, if the daemon restarts first.
        // The operator is told the adoption happened either way.
        //
        // `adopt_deletion` and `open_adoption` both gate above their own
        // API-source branches; this is the same place.
        let api_source = {
            let (node, space_id, path) = (self.clone(), space_id.to_string(), path.to_string());
            crate::blocking::offload(move || {
                node.ensure_adoptable(&space_id, &path)?;
                node.is_api_source(&space_id)
            })
            .await?
        };
        if api_source {
            // `prepare_range` fetches and verifies the complete selected
            // object. Before adoption names it as *our* version, promote it to
            // the backend's durable tier: after publication this node may be
            // the only holder peers associate with the new assertion, so a
            // scratch-only copy is not enough under any upload policy.
            let range = self.prepare_range(space_id, path, &policy, 0, None).await?;
            self.cas_backend().finalize(range.root, range.size).await?;
            let reported = PathBuf::from(format!("{space_id}/{path}"));
            let (node, space, path) = (self.clone(), space_id.to_string(), path.to_string());
            crate::blocking::offload(move || {
                node.stage_api_reference(&space, &path, range.root, range.size, now_ns(), None)
            })
            .await?;
            return Ok(reported);
        }
        // Resolving the target first means a path outside every filesystem source
        // is refused before anything is fetched. It reads the space row, so it
        // goes to the blocking pool like every other store read on an async
        // path (§10).
        {
            let (node, space_id, path) = (self.clone(), space_id.to_string(), path.to_string());
            crate::blocking::offload(move || node.adoption_target(&space_id, &path)).await?;
        }
        let range = self.prepare_range(space_id, path, &policy, 0, None).await?;
        // The write lands through the space-validated adoption: the target's
        // parent directory is resolved component by component at open and the
        // payload is cloned into the staging file it pinned, so a directory
        // swapped for a symlink by the fetch standing between the first check
        // and here cannot redirect the write outside the space (§7.2, §9.4).
        //
        // The object materializes into a daemon-owned staging file first —
        // the data dir is not attacker-writable — then the adoption clones it
        // into the pinned directory (`FICLONE` where the filesystem shares
        // extents, a copy elsewhere). Everything after the fetch is store
        // reads and file I/O, so it goes to the blocking pool like the fetch's
        // own halves (§10).
        let staged = {
            let node = self.clone();
            crate::blocking::offload(move || {
                Ok(node
                    .store()
                    .staging_dir()
                    .join(format!("take-{}.payload", synch_core::fs::unique_suffix())))
            })
            .await?
        };
        self.materialize_blob(&range.root, range.size, staged.clone())
            .await?;
        let target = {
            let (node, space_id, path, staged) =
                (self.clone(), space_id.to_string(), path.to_string(), staged);
            crate::blocking::offload(move || {
                let mut adoption = Adoption::into_space(&node, &space_id, &path)?;
                adoption.clone_from(&staged)?;
                let _ = std::fs::remove_file(&staged);
                adoption.commit()
            })
            .await?
        };
        Ok(target)
    }

    /// Adopts a peer's *deletion* of a path as our own (§8, `synch adopt path`).
    ///
    /// Deletions are adoptable exactly as content is: our local copy goes, and
    /// the next scan publishes our own tombstone through the ordinary indexing
    /// pipeline — the same path a deletion made with `rm` takes. Adoption is
    /// how all divergence ends, deletion divergence included: once every
    /// publisher tombstones the path, it leaves the unified tree.
    ///
    /// Returns the file that was removed, or `None` when there was nothing
    /// here to remove — which is not an error: the assertion being adopted is
    /// "this path is gone", and it already is.
    pub fn adopt_deletion(&self, space_id: &str, path: &str) -> Result<Option<PathBuf>> {
        // The publishability half only: an excluded path is exactly the stray
        // file a deletion is here to clear up (`refuse_if_ignored`).
        self.ensure_publishable()?;
        if self.is_api_source(space_id)? {
            let normalized = normalized_adoption_path(path)?;
            let previous = self
                .store()
                .entry(self.origin(), space_id, &normalized)?
                .and_then(|entry| entry.content);
            let tombstone = FileEntry::tombstone(now_ns(), self.next_seq()?, previous);
            let encoded = synch_core::record::encode(&tombstone)?;
            self.stage([(file_key(space_id, &normalized)?, Some(encoded))]);
            return Ok(previous.map(|_| PathBuf::from(format!("{space_id}/{normalized}"))));
        }
        let target = self.adoption_target(space_id, path)?;
        // `symlink_metadata`, so a symlink is removed as the link it is rather
        // than followed to whatever it points at.
        if std::fs::symlink_metadata(&target).is_err() {
            return Ok(None);
        }
        if target.is_dir() {
            // The path stays in the log and out of the message. This error
            // reaches an S3 client verbatim, and the daemon's on-disk layout —
            // the operator's home, the space roots — is not something a client
            // that guessed a key is owed.
            tracing::warn!(
                target = %target.display(),
                "refusing to remove a directory as if it were an object"
            );
            return Err(EngineError::invalid(format!(
                "{space_id}/{path} is a directory here; refusing to remove it"
            )));
        }
        std::fs::remove_file(&target)?;
        Ok(Some(target))
    }

    /// Opens a streamed write into a local space (§9.4).
    ///
    /// The bytes-in-hand form is [`Node::adopt`]; this is the form for a
    /// payload arriving a piece at a time — an S3 `PutObject` body relayed over
    /// the control socket — where holding the object in memory to call the
    /// other one is exactly what must not happen.
    pub fn open_adoption(&self, space_id: &str, path: &str) -> Result<Adoption> {
        if self.is_api_source(space_id)? {
            self.ensure_adoptable(space_id, path)?;
            let _ = normalized_adoption_path(path)?;
            let target = self.store().staging_dir().join(format!(
                "api-source-{}.payload",
                synch_core::fs::unique_suffix()
            ));
            return Adoption::open(target);
        }
        Adoption::into_space(self, space_id, path)
    }

    /// Ingests a committed API-source staging file and stages its records.
    ///
    /// The durable CAS row lands before the `f:` and `b:` records enter the
    /// publisher. Until the caller acknowledges, `source` is unacknowledged
    /// backend-private scratch and may be discarded after this returns.
    pub async fn commit_api_file(
        &self,
        space_id: &str,
        path: &str,
        source: &Path,
        mtime_ns: i64,
    ) -> Result<(Hash, u64)> {
        self.commit_api_file_with_metadata(space_id, path, source, mtime_ns, None)
            .await
    }

    /// The socket-writer variant, carrying host-originated permission bits.
    pub(crate) async fn commit_api_file_with_metadata(
        &self,
        space_id: &str,
        path: &str,
        source: &Path,
        mtime_ns: i64,
        unix_mode: Option<u32>,
    ) -> Result<(Hash, u64)> {
        let (node, checked_space, checked_path, checked_source) = (
            self.clone(),
            space_id.to_string(),
            path.to_string(),
            source.to_path_buf(),
        );
        let (normalized, size) = crate::blocking::offload(move || {
            // The API-source half of `ensure_adoptable`, and the last adoption
            // entry point that trusted its caller for it. Both callers do check
            // — the `put` handler and `complete_upload` — but both then do the
            // work in a spawned task, so an inbound `Hello` can floor this node
            // in between, and an embedder calling this `pub fn` has no check at
            // all. What follows is a durable-tier promotion (a billable write
            // on a cloud CAS) and an `f:`/`b:` pair staged for a publish that
            // would be refused, re-failing on every quiesce tick and orphaning
            // the blob if the daemon restarts first.
            //
            // `refuse_if_ignored` is the other half and is a no-op here: a
            // API source has no directory to hold a `.syncignore`.
            node.ensure_publishable()?;
            if !node.is_api_source(&checked_space)? {
                return Err(EngineError::invalid(format!(
                    "space {checked_space} has a local checkout"
                )));
            }
            Ok((
                normalized_adoption_path(&checked_path)?,
                std::fs::metadata(&checked_source)?.len(),
            ))
        })
        .await?;

        let ingested = self
            .cas_backend()
            .ingest_file(source.to_path_buf(), now_ns())
            .await?;
        debug_assert_eq!(ingested.size, size);
        let (node, space) = (self.clone(), space_id.to_string());
        crate::blocking::offload(move || {
            node.stage_api_reference(
                &space,
                &normalized,
                ingested.root,
                ingested.size,
                mtime_ns,
                unix_mode,
            )?;
            Ok((ingested.root, ingested.size))
        })
        .await
    }

    /// Stages an API-source file entry and its durable-holder advertisement.
    pub(crate) fn stage_api_reference(
        &self,
        space_id: &str,
        path: &str,
        root: Hash,
        size: u64,
        mtime_ns: i64,
        unix_mode: Option<u32>,
    ) -> Result<()> {
        let normalized = normalized_adoption_path(path)?;
        let previous = self
            .store()
            .entry(self.origin(), space_id, &normalized)?
            .and_then(|entry| entry.content);
        let mut entry = FileEntry::file(size, mtime_ns, root, self.next_seq()?);
        entry.unix_mode = unix_mode;
        entry.prev = previous.filter(|previous| *previous != root);
        let entry = synch_core::record::encode(&entry)?;
        let ad = self.store().local_ad(&root)?.ok_or_else(|| {
            EngineError::invalid(format!(
                "the durable ingest of {root} produced no local advertisement"
            ))
        })?;
        let ad = synch_core::record::encode(&ad)?;
        self.stage([
            (file_key(space_id, &normalized)?, Some(entry)),
            (blob_key(&root), Some(ad)),
        ]);
        Ok(())
    }

    /// The gates every adoption into a filesystem-source directory takes, before it
    /// touches anything.
    ///
    /// Publishability first. A node in key-loss recovery cannot publish (§3.4),
    /// and an adoption that writes before finding that out has destroyed the
    /// local copy and can tell nobody — no version, no `prev`, no trace. That
    /// is the reasoning `Node::delete_object` already states over the very same
    /// `adopt_deletion` this guards; `synch adopt path` reached the gate only at its
    /// publish, which is after the file is gone.
    ///
    /// Then the space's own ignore rules.
    pub(crate) fn ensure_adoptable(&self, space_id: &str, path: &str) -> Result<()> {
        self.ensure_publishable()?;
        self.refuse_if_ignored(space_id, path)
    }

    /// Refuses a write at a path the space's own ignore rules exclude.
    ///
    /// Taken by every writer into a filesystem-source directory — `synch adopt path` in both its
    /// forms, and the streamed write an S3 `PutObject` becomes — because a file
    /// written where the scanner will never look is worse than a refused write:
    /// it is never published, never swept (it is in neither `local_files` nor
    /// the published tree), and it sits in the operator's own directory
    /// indefinitely. `create_upload` takes the same check at *initiate* as
    /// well, so a multipart client learns before it streams gigabytes rather
    /// than after.
    ///
    /// Deliberately not on the deletion path: a delete of an excluded key is
    /// how a stray file like that gets cleaned up, and S3 `DELETE` is
    /// idempotent besides.
    pub(crate) fn refuse_if_ignored(&self, space_id: &str, path: &str) -> Result<()> {
        let Some(space) = self.store().source(space_id)? else {
            return Ok(());
        };
        let Some(local_path) = space.local_path.as_deref() else {
            return Ok(());
        };
        let normalized =
            synch_core::normalize_path(path).map_err(|e| EngineError::invalid(e.to_string()))?;
        if IgnoreSet::for_space(Path::new(local_path))?.excludes_path(&normalized) {
            return Err(EngineError::invalid(format!(
                "{space_id}/{path} matches an ignore rule, so it could never be published"
            )));
        }
        Ok(())
    }

    /// Where a path lives locally, refusing anything outside a configured
    /// space.
    ///
    /// The guard is the same for content and for deletions: `synch adopt path` may
    /// only ever write inside a filesystem source, because outside one
    /// nothing would publish the adoption and the write would be a silent
    /// no-op with a filesystem side effect.
    pub(crate) fn adoption_target(&self, space_id: &str, path: &str) -> Result<PathBuf> {
        Ok(self.adoption_target_checked(space_id, path)?.0)
    }

    /// [`Self::adoption_target`], plus the escape-guard inputs the commit
    /// re-verifies: the space's local root and the normalized relative path.
    pub(crate) fn adoption_target_checked(
        &self,
        space_id: &str,
        path: &str,
    ) -> Result<(PathBuf, (PathBuf, String))> {
        let space = self
            .store()
            .source(space_id)?
            .ok_or_else(|| EngineError::not_found(format!("space {space_id}")))?;
        let local_path = space.local_path.ok_or_else(|| {
            EngineError::invalid(format!(
                "source {space_id} is API-only and has no filesystem adoption target"
            ))
        })?;
        let root = PathBuf::from(local_path);
        let (target, normalized) = target_within_checked(&root, space_id, path)?;
        Ok((target, (root, normalized)))
    }
}

/// [`Node::adoption_target`], plus the escape-guard inputs the commit
/// re-verifies: the space's local root and the normalized relative path.
///
/// The checked half of [`target_within`] — same resolution, and the inputs
/// that let a later step re-check the guard against the directory the write
/// is about to land in.
pub(crate) fn target_within_checked(
    root: &Path,
    space_id: &str,
    path: &str,
) -> Result<(PathBuf, String)> {
    let normalized = normalized_adoption_path(path)?;
    // Lexical safety is still not enough. A space root is canonicalized when
    // it is added but its *interior* never is, so a symlinked directory
    // inside the space resolves through to wherever it points, and the write
    // or the delete lands outside every space as whatever uid the daemon
    // runs as. Checkout reconciliation checks this; every other writer
    // needs the same check, and a deletion needs it as much as a write does.
    if crate::checkout::escapes_via_symlink(root, &normalized) {
        return Err(EngineError::invalid(format!(
            "{space_id}/{path} resolves through a symlinked directory and would leave the space"
        )));
    }
    Ok((root.join(&normalized), normalized))
}

/// The guard itself, over a space root already in hand.
///
/// [`Node::adoption_target`] is the form that reads the space row first; this
/// is the form for a caller holding the root already and asking it of many
/// paths in a row (`synch adopt tree`, adopt_tree.rs), where re-reading that row per path
/// would be one store acquisition per file in the space.
pub(crate) fn target_within(root: &Path, space_id: &str, path: &str) -> Result<PathBuf> {
    Ok(target_within_checked(root, space_id, path)?.0)
}

/// Normalizes a write path and applies the host platform's relative-path rules.
pub(crate) fn normalized_adoption_path(path: &str) -> Result<String> {
    let normalized =
        synch_core::normalize_path(path).map_err(|e| EngineError::invalid(e.to_string()))?;
    // And the platform has to agree the result is purely relative before it
    // is joined onto the space root.
    //
    // `normalize_path` works in the protocol's own path language, where `/`
    // is the only separator — deliberately, so a trie key means the same
    // thing on every node. On Windows the *platform* reads more than that: a
    // key of `..\..\evil.txt` is one `Normal` component to the protocol and
    // a traversal to `Path::join`, and one of `C:/Windows/Temp/evil.txt`
    // carries a drive prefix, which makes `join` discard the space root
    // entirely. Either writes outside every space, as the daemon user, from
    // any key the S3 gateway accepts.
    //
    // A no-op on POSIX, where none of those parse as anything but `Normal`.
    // Checkout `unsafe_name` already refuses these on the way out; this
    // is the way in.
    if Path::new(&normalized)
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(EngineError::invalid(format!(
            "path {path} is not a plain relative path on this platform"
        )));
    }
    Ok(normalized)
}

/// How much an [`Adoption::append_file`] fallback moves per read/write pair.
///
/// Only reached on a filesystem without `copy_file_range`, where the cost is a
/// bounce through user space and the buffer is the whole of what this process
/// holds of a part.
const APPEND_CHUNK: u64 = 1024 * 1024;

/// The suffix a streamed write's staging file carries.
///
/// Matched by a built-in ignore rule ([`crate::ignore::BUILTIN_DEFAULTS`]), so
/// a scan that runs while an upload is still arriving walks straight past it.
pub(crate) const PART_SUFFIX: &str = ".synch-part";

/// A streamed write into a local space that has not landed yet (§9.4).
///
/// Bytes go to a staging file beside the target, and the target only appears
/// once the payload is complete. A client that hangs up mid-body therefore
/// leaves the space exactly as it was, rather than leaving this node to publish
/// half an object as its own assertion — and since the assertion is signed and
/// broadcast, "rather than" is doing real work there. Dropping an `Adoption`
/// without committing removes the staging file.
#[derive(Debug)]
pub struct Adoption {
    target: PathBuf,
    staging: PathBuf,
    file: Option<std::fs::File>,
    written: u64,
    /// Set when the target is inside an filesystem source, and `None` when it is a
    /// staging file the daemon owns.
    ///
    /// This is what makes the gates a property of the *write* rather than of
    /// each of the eight places that start one. Every adoption ends here, in
    /// `commit`, at a rename that destroys what it lands on — and until this
    /// field existed `commit` was the one step that could not check anything,
    /// because it held nothing to ask. Each caller therefore had to re-take the
    /// gates immediately before it, and the review history of this branch is
    /// mostly the record of callers that did not.
    space: Option<SpaceWrite>,
    /// The directory the staging file and the target live in, resolved
    /// component by component at open — `O_NOFOLLOW` on Unix, a rooted
    /// `NtCreateFile` chain with `FILE_OPEN_REPARSE_POINT` on Windows — so
    /// the commit rename resolves relative to it. A directory swapped for a
    /// symlink or junction — before the open or while the body streams —
    /// cannot redirect a rename that never re-resolves a path.
    #[cfg(unix)]
    parent: Option<rustix::fd::OwnedFd>,
    /// The space root this write was pinned against (Unix), so the commit can
    /// re-walk the intended path after the rename and confirm the object is
    /// there — a pinned parent directory that a local actor *renamed* (rather
    /// than replaced with a symlink) mid-stream would otherwise take the
    /// commit with it, landing the object where the rename was, not where the
    /// key says it is.
    #[cfg(unix)]
    root: Option<std::path::PathBuf>,
    #[cfg(windows)]
    parent: Option<std::os::windows::io::OwnedHandle>,
}

/// What a write into an filesystem source needs to re-check before it lands.
#[derive(Debug)]
struct SpaceWrite {
    node: Node,
    space: String,
    path: String,
}

impl Adoption {
    /// Stages a write at an arbitrary path.
    ///
    /// [`Node::open_adoption`] is the form that resolves a `<space>/<path>`
    /// first; this one is for a target that is already known — a checkout file
    /// (§7.2), which by construction lives outside every filesystem source.
    pub fn at(target: impl Into<PathBuf>) -> Result<Adoption> {
        Adoption::open(target.into())
    }

    /// The same, for a target inside an filesystem source: the write carries the
    /// gates with it and re-takes them at `commit`.
    fn into_space(node: &Node, space_id: &str, path: &str) -> Result<Adoption> {
        node.ensure_adoptable(space_id, path)?;
        // The space-validated open: the target is resolved one component at a
        // time with `O_NOFOLLOW` from the space root, so the staging file is
        // created inside the directory the commit rename will resolve against
        // — a directory swapped for a symlink cannot redirect either.
        let (_target, (root, rel)) = node.adoption_target_checked(space_id, path)?;
        let mut adoption = Adoption::in_space(&root, &rel)?;
        adoption.space = Some(SpaceWrite {
            node: node.clone(),
            space: space_id.to_string(),
            path: path.to_string(),
        });
        Ok(adoption)
    }

    fn open(target: PathBuf) -> Result<Adoption> {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // The staging name has to be unique per write: two clients putting the
        // same key at once must not share one file and interleave their bytes.
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "object".into());
        let staging = target.with_file_name(format!(
            ".{name}.{}{PART_SUFFIX}",
            synch_core::fs::unique_suffix()
        ));
        // Read *and* write: the payload is written here, and a multipart
        // completion reads it straight back to take the object's root before
        // the rename. `File::create` alone is write-only, and the read then
        // fails with `EBADF` on a file this very process is holding open.
        //
        // Not world-readable: the staging file holds the in-flight bytes, and
        // a local actor who can read them gets a copy of an upload before the
        // rename publishes it.
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&staging)?;
        Ok(Adoption {
            target,
            staging,
            file: Some(file),
            written: 0,
            space: None,
            #[cfg(unix)]
            parent: None,
            #[cfg(unix)]
            root: None,
            #[cfg(windows)]
            parent: None,
        })
    }

    /// Opens a write at `rel` under a space root, resolving the target's
    /// parent directory one component at a time with `O_NOFOLLOW` (Unix).
    ///
    /// No component of the path is ever followed, so a directory swapped for
    /// a symlink — by the local actor the open-time check exists to stop —
    /// cannot redirect where the staging file is created or where the commit
    /// rename lands: the rename resolves against the held directory, not
    /// against anything a path can still mean. Missing parents are created,
    /// like the path-based open.
    #[cfg(unix)]
    pub(crate) fn in_space(root: &Path, rel: &str) -> Result<Adoption> {
        let parent = open_parent_no_follow(root, rel)?;
        // The staging name has to be unique per write: two clients putting the
        // same key at once must not share one file and interleave their bytes.
        let name = rel.rsplit('/').next().unwrap_or(rel);
        let staging_name = format!(".{name}.{}{PART_SUFFIX}", synch_core::fs::unique_suffix());
        // Read *and* write: the payload is written here, and a multipart
        // completion reads it straight back to take the object's root before
        // the rename. `File::create` alone is write-only, and the read then
        // fails with `EBADF` on a file this very process is holding open.
        //
        // Created relative to the pinned parent — a pre-placed symlink at the
        // staging name is refused (`NOFOLLOW`) rather than followed — and not
        // world-readable: the staging file holds the in-flight bytes, and a
        // local actor who can read them gets a copy of an upload before the
        // rename publishes it.
        let file = rustix::fs::openat(
            &parent,
            &staging_name,
            rustix::fs::OFlags::RDWR
                | rustix::fs::OFlags::CREATE
                | rustix::fs::OFlags::TRUNC
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::from_bits_retain(0o600),
        )
        .map_err(|e| {
            EngineError::invalid(format!(
                "could not create the staging file beside {}: {e}",
                rel
            ))
        })?;
        let staging = root.join(rel).with_file_name(&staging_name);
        Ok(Adoption {
            target: root.join(rel),
            staging,
            file: Some(std::fs::File::from(file)),
            written: 0,
            space: None,
            parent: Some(parent),
            root: Some(root.to_path_buf()),
        })
    }

    /// The Windows shape of [`Self::in_space`]: the parent directory is
    /// resolved with a rooted `NtCreateFile` chain, one component at a time
    /// with `FILE_OPEN_REPARSE_POINT`, so a junction is never followed; the
    /// staging file is created relative to the resolved parent, and the
    /// commit renames the staging file relative to that same parent. No path
    /// is re-resolved by the commit, so a directory swapped for a junction
    /// while the body streamed cannot redirect the write outside the space.
    ///
    /// On Windows the *protocol* separator `/` and the *platform* separator
    /// `\` are both treated as separators, so a key such as `a\b` pins `a`
    /// rather than being handed to `NtCreateFile` as one opaque component
    /// that its own path parsing would then walk through.
    #[cfg(windows)]
    pub(crate) fn in_space(root: &Path, rel: &str) -> Result<Adoption> {
        let parent = nt::open_parent_no_follow(root, rel)?;
        // The staging name has to be unique per write: two clients putting the
        // same key at once must not share one file and interleave their bytes.
        let name = rel.rsplit(['/', '\\']).next().unwrap_or(rel);
        let staging_name = format!(".{name}.{}{PART_SUFFIX}", synch_core::fs::unique_suffix());
        // Created relative to the pinned parent — a pre-placed reparse point
        // at the staging name is opened as itself (`OPEN_REPARSE_POINT`)
        // rather than followed, and overwritten in place.
        let file = nt::create_staging(&parent, &staging_name)?;
        let staging = root.join(rel).with_file_name(&staging_name);
        Ok(Adoption {
            target: root.join(rel),
            staging,
            file: Some(file),
            written: 0,
            space: None,
            parent: Some(parent),
        })
    }

    /// Fills the staging file from `source`, sharing its extents if it can
    /// (`docs/DELTA-SYNC.md` §3.5).
    ///
    /// The atomicity invariant is unchanged — the bytes land in a staging file
    /// that is renamed over the target, so a reader sees the old file or the
    /// new one — but the staging file arrives already full. This is how an
    /// object becomes a checkout file: the source is the CAS payload, and a
    /// 100 GB image is materialized without moving 100 GB.
    ///
    /// `FICLONE` first. On btrfs, XFS and bcachefs it shares the source's
    /// extents copy-on-write, so the write is O(1) and consumes no space until
    /// one of the two files is written to. Everywhere else — a target on a
    /// different filesystem from the source, ext4, a kernel or platform without
    /// the ioctl — it falls back to a plain copy, which on Linux is itself a
    /// kernel-side `copy_file_range` rather than a bounce through user space.
    fn clone_from(&mut self, source: &Path) -> Result<CloneKind> {
        let mut file = self
            .file
            .take()
            .ok_or_else(|| EngineError::invalid("a fresh staging file has no handle"))?;
        match std::fs::File::open(source).and_then(|src| reflink_file(&src, &file)) {
            Ok(()) => {
                self.file = Some(file);
                return Ok(CloneKind::Reflink);
            }
            Err(e) => {
                tracing::debug!(source = %source.display(), error = %e, "reflink unavailable");
            }
        }
        // The fallback copies through the open handle rather than by path:
        // the staging file was created relative to the directory pinned at
        // open, and writing through the handle keeps it there — a path-based
        // copy would re-resolve the staging path through whatever the tree
        // looks like now.
        let mut src = std::fs::File::open(source)?;
        std::io::copy(&mut src, &mut file)?;
        self.file = Some(file);
        Ok(CloneKind::Copy)
    }

    /// Sets the staging file's length, for a clone of a file whose size the new
    /// version does not share.
    pub fn set_len(&mut self, len: u64) -> Result<()> {
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| EngineError::invalid("this write has already been committed"))?;
        file.set_len(len)?;
        self.written = len;
        Ok(())
    }

    /// Reads one range from the staged payload without changing its logical
    /// length.
    pub(crate) fn read_at(&mut self, offset: u64, len: u64) -> Result<Vec<u8>> {
        use std::io::{Read, Seek};
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| EngineError::invalid("this write has already been committed"))?;
        file.seek(std::io::SeekFrom::Start(offset))?;
        let len = usize::try_from(len)
            .map_err(|_| EngineError::invalid("the staged read length is too large"))?;
        let mut bytes = vec![0; len];
        let count = file.read(&mut bytes)?;
        bytes.truncate(count);
        Ok(bytes)
    }

    /// Writes one range into the staged payload, growing it as necessary.
    pub(crate) fn write_at(&mut self, offset: u64, bytes: &[u8]) -> Result<()> {
        use std::io::{Seek, Write};
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| EngineError::invalid("this write has already been committed"))?;
        if bytes.is_empty() {
            return Ok(());
        }
        file.seek(std::io::SeekFrom::Start(offset))?;
        file.write_all(bytes)?;
        self.written = self.written.max(offset.saturating_add(bytes.len() as u64));
        Ok(())
    }

    /// Appends one piece of the payload.
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        use std::io::{Seek, Write};
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| EngineError::invalid("this write has already been committed"))?;
        if bytes.is_empty() {
            return Ok(());
        }
        file.seek(std::io::SeekFrom::End(0))?;
        file.write_all(bytes)?;
        self.written = self.written.saturating_add(bytes.len() as u64);
        Ok(())
    }

    /// Preserves metadata already supplied by the host on the staged inode.
    pub(crate) fn set_metadata(
        &mut self,
        unix_mode: Option<u32>,
        mtime_ns: Option<i64>,
    ) -> Result<()> {
        let file = self
            .file
            .as_ref()
            .ok_or_else(|| EngineError::invalid("this write has already been committed"))?;
        #[cfg(unix)]
        if let Some(mode) = unix_mode {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(mode & 0o777))?;
        }
        #[cfg(not(unix))]
        let _ = unix_mode;
        if let Some(mtime_ns) = mtime_ns {
            let modified = if mtime_ns >= 0 {
                std::time::UNIX_EPOCH + std::time::Duration::from_nanos(mtime_ns as u64)
            } else {
                std::time::UNIX_EPOCH - std::time::Duration::from_nanos(mtime_ns.unsigned_abs())
            };
            file.set_times(std::fs::FileTimes::new().set_modified(modified))?;
        }
        Ok(())
    }

    /// How many bytes have arrived so far.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// The object root of what has been staged so far.
    ///
    /// What a multipart completion answers with. Taking the root here rather
    /// than reading it back off the published entry is the difference between
    /// describing the bytes this call assembled and describing whatever the
    /// tree holds for that key by the time the scan reaches it — which a
    /// concurrent write to the same key wins.
    pub(crate) fn hash_staged(&mut self) -> Result<synch_core::Hash> {
        use std::io::{Seek, Write};
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| EngineError::invalid("this write has already been committed"))?;
        file.flush()?;
        file.seek(std::io::SeekFrom::Start(0))?;
        let root = synch_core::hash_reader(std::io::BufReader::new(file.try_clone()?))?;
        // The handle is left at the end, where an append expects it.
        file.seek(std::io::SeekFrom::End(0))?;
        Ok(root)
    }

    /// Appends a whole file to the staging payload (§9.4).
    ///
    /// What assembles a multipart upload: each part is its own staged payload,
    /// and completing the upload is this call once per part in ascending
    /// order. The bytes move inside the kernel — `copy_file_range` shares
    /// extents outright on a filesystem that can, and copies without a bounce
    /// through user space on one that cannot — so a 50 GiB object assembled
    /// from 8 MiB parts never passes through this process.
    ///
    /// `FICLONE` is deliberately not used here even though `Adoption::clone_from`
    /// prefers it: it is a *whole file* clone that replaces the destination,
    /// which is the one thing an append must not do. The range form needs
    /// block-aligned offsets that arbitrary part sizes do not have.
    ///
    /// Blocking, like every other method here — the caller runs it off the
    /// runtime.
    pub(crate) fn append_file(&mut self, source: &Path) -> Result<u64> {
        use std::io::{Read, Seek, Write};
        let mut src = std::fs::File::open(source)?;
        let len = src.metadata()?.len();
        let dest = self
            .file
            .as_mut()
            .ok_or_else(|| EngineError::invalid("this write has already been committed"))?;
        // `written` advances as the bytes move, not once at the end: a failure
        // part-way through still leaves the staging file longer than it was,
        // and a `written` that under-counts it makes every later size check —
        // and every error message quoting it — wrong.
        let mut moved = 0u64;
        #[cfg(target_os = "linux")]
        while moved < len {
            let take = usize::try_from(len - moved).unwrap_or(usize::MAX);
            // `None` for both offsets advances each file's own cursor, which is
            // exactly the append semantics wanted: the destination cursor is
            // already at the end of everything appended so far.
            match rustix::fs::copy_file_range(&src, None, &*dest, None, take) {
                // Short of the length the metadata reported: the source is
                // being written under us, or the filesystem refuses to say
                // more. The fallback below reads it and reports the real error.
                Ok(0) => break,
                Ok(count) => {
                    moved += count as u64;
                    self.written += count as u64;
                }
                // Swallowed only as a signal to fall back — `EXDEV` and
                // `ENOSYS` are the expected ones — but logged, because an
                // `ENOSPC` on the destination would otherwise be reported by
                // the fallback as whatever it happens to hit next.
                Err(e) => {
                    tracing::debug!(error = %e, "copy_file_range unavailable; copying by hand");
                    break;
                }
            }
        }
        if moved < len {
            // `copy_file_range` may have consumed part of the source already,
            // so the fallback resumes from where it stopped rather than from
            // the start.
            src.seek(std::io::SeekFrom::Start(moved))?;
            let mut buffer = vec![0u8; APPEND_CHUNK.min(len - moved) as usize];
            while moved < len {
                let take = (APPEND_CHUNK.min(len - moved)) as usize;
                let piece = &mut buffer[..take];
                src.read_exact(piece)?;
                dest.write_all(piece)?;
                moved += take as u64;
                self.written += take as u64;
            }
        }
        Ok(moved)
    }

    /// Flushes the payload and moves it into place, returning the target.
    ///
    /// The rename is what makes the write atomic from the scanner's point of
    /// view: it sees the old file or the new one, never a partial one. That is
    /// a claim about crashes, so the directory entry the rename created is
    /// flushed too — otherwise the contents survive a power cut and the name
    /// they arrived under does not, which is the old file or *no* file rather
    /// than the old file or the new one.
    /// The rename itself can fail — the target is a directory, the filesystem
    /// filled up under the fsync — and when it does, this is the only chance to
    /// unlink the staging file: [`Drop`] only cleans up while the handle is
    /// live, and committing has to let go of it to flush and rename it. Leaving
    /// it stranded would leave a full-size copy of the object beside the target
    /// under a name the scanner's built-in ignore rules skip, unnoticed and
    /// uncollected forever, on a path that is reached precisely when the disk is
    /// already in trouble.
    pub fn commit(mut self) -> Result<PathBuf> {
        match self.commit_inner() {
            Ok(()) => Ok(self.target.clone()),
            Err(e) => {
                let _ = std::fs::remove_file(&self.staging);
                Err(e)
            }
        }
    }

    fn commit_inner(&mut self) -> Result<()> {
        // Immediately before the rename, because this is the moment the gates
        // are about: a write that opened minutes or days ago — an S3 body
        // streaming at the client's pace, a multipart upload assembled out of
        // parts — commits into a node that may have been floored by an inbound
        // `Hello` since, or into a space whose ignore rules have moved. The
        // rename destroys what it lands on, and a refusal after it is a file
        // gone with nothing published to show for it.
        //
        // Taken here rather than by each caller: `commit` is the one step every
        // adoption reaches, so a gate here cannot be the one somebody forgets.
        if let Some(space) = &self.space {
            space.node.ensure_adoptable(&space.space, &space.path)?;
        }
        let file = self
            .file
            .take()
            .ok_or_else(|| EngineError::invalid("this write has already been committed"))?;
        file.sync_all()?;
        self.rename_into_place(file)?;
        synch_core::fs::fsync_parent(&self.target);
        Ok(())
    }

    /// The staging file is renamed over the target with no path
    /// re-resolution (Unix): the target's parent directory was resolved
    /// component by component with `O_NOFOLLOW` at open, so a directory
    /// swapped for a symlink — before the open or while the body streamed —
    /// cannot redirect the write. A write that was not opened against a
    /// source root (checkout materializations are elsewhere) has nothing pinned and
    /// keeps the plain rename, exactly as before; its own guard runs in the
    /// same blocking step ([`crate::checkout`]).
    #[cfg(unix)]
    fn rename_into_place(&mut self, file: std::fs::File) -> Result<()> {
        match self.parent.take() {
            Some(dir) => {
                let staging = self
                    .staging
                    .file_name()
                    .ok_or_else(|| EngineError::invalid("the staging path has no file name"))?;
                let target = self
                    .target
                    .file_name()
                    .ok_or_else(|| EngineError::invalid("the target path has no file name"))?;
                // The staging file's identity, taken while the handle is
                // live: the verification below compares it against what now
                // sits at the intended target.
                let staged = rustix::fs::fstat(&file).map_err(|e| {
                    EngineError::invalid(format!("could not stat the staging file: {e}"))
                })?;
                rustix::fs::renameat(&dir, staging, &dir, target)
                    .map_err(|e| EngineError::invalid(format!("rename into place failed: {e}")))?;
                // The pinned directory pins an inode, not a place in the
                // tree: a local actor who *renames* the parent directory
                // (staging file still inside) to a location outside the
                // space while the body streams takes the commit along with
                // it — `renameat` succeeds inside the moved directory and
                // the write would report the old in-space target. Verify the
                // object is where the key says it is, and remove it through
                // the pinned parent when it is not.
                self.verify_landing(&dir, &staged)?;
            }
            None => std::fs::rename(&self.staging, &self.target)?,
        }
        Ok(())
    }

    /// After the commit rename, confirms the object is at the intended
    /// target by re-resolving the target's parent from the space root with
    /// the same `O_NOFOLLOW` walk and comparing `dev:ino` with the staging
    /// file's identity — a parent directory moved out from under the write
    /// resolves to nothing (or to a symlink, which the walk refuses), and
    /// the object is removed where the moved directory actually is and the
    /// commit fails closed. Only reached for writes pinned against a space
    /// root.
    #[cfg(unix)]
    fn verify_landing(&self, dir: &rustix::fd::OwnedFd, staged: &rustix::fs::Stat) -> Result<()> {
        let Some(root) = &self.root else {
            return Ok(());
        };
        let rel = self
            .target
            .strip_prefix(root)
            .map_err(|_| EngineError::invalid("the commit target is not under the space root"))?;
        let rel = rel.to_str().ok_or_else(|| {
            EngineError::invalid("the commit target is not valid UTF-8 (space paths are)")
        })?;
        // Re-resolving with the open-time walk, including its create-missing-
        // parents behavior: a parent the actor moved away comes back as an
        // empty directory chain, which then has no object to show. A symlink
        // in the intended path fails the walk — the open-time resolver
        // refuses symlinks — which is a mismatch too, not an error to
        // propagate: the object needs removing either way.
        let target = self
            .target
            .file_name()
            .ok_or_else(|| EngineError::invalid("the target path has no file name"))?;
        let matched = match open_parent_no_follow(root, rel) {
            Ok(rewalked) => {
                match rustix::fs::statat(&rewalked, target, rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
                    // The renamed file is the staging file — `rename`
                    // preserves the inode — so a matching `dev:ino` means
                    // the object sits at the intended target, inside the
                    // space.
                    Ok(landed) => landed.st_dev == staged.st_dev && landed.st_ino == staged.st_ino,
                    Err(_) => false,
                }
            }
            Err(_) => false,
        };
        if matched {
            return Ok(());
        }
        // The object is where the moved directory is, not where the key says
        // it is: remove it through the pinned parent (the `renameat` above
        // used the same handle, so this finds it) and fail closed.
        let _ = rustix::fs::unlinkat(dir, target, rustix::fs::AtFlags::empty());
        Err(EngineError::invalid(
            "the write's parent directory was moved while the body streamed; \
             the object was removed and the write failed closed",
        ))
    }

    /// The staging file is renamed over the target (Windows): `MoveFileExW`
    /// with the full paths, followed by the containment check that verifies
    /// the file that landed is the staging file itself, still at the
    /// intended name inside the pinned directory — so a directory swapped
    /// for a junction — before the open or while the body streamed — cannot
    /// silently redirect the write outside the space, and a redirected
    /// payload is removed wherever it actually landed. A write that was not
    /// opened against a space root keeps the plain rename, exactly as
    /// before.
    #[cfg(windows)]
    fn rename_into_place(&mut self, file: std::fs::File) -> Result<()> {
        match self.parent.take() {
            Some(dir) => {
                let staging = self.staging.clone();
                let target = self.target.clone();
                // The staging handle stays open through the move: after it,
                // it refers to the moved file wherever it landed, which is
                // what the containment check compares against — and what
                // delete-on-close removes when the check fails.
                nt::rename_into(&dir, &file, &staging, &target)?;
                drop(file);
            }
            None => {
                drop(file);
                std::fs::rename(&self.staging, &self.target)?;
            }
        }
        Ok(())
    }
}

/// Resolves `rel`'s parent directory under `root`, one component at a time,
/// opening each with `O_NOFOLLOW` and creating missing ones (Unix).
///
/// The resolution is the enforcement: no component is ever followed, so a
/// symlink swapped in anywhere along the way — by the local actor with write
/// access to the space — fails the open instead of redirecting it. The
/// returned handle is what the staging file is created against and what the
/// commit rename resolves against.
#[cfg(unix)]
fn open_parent_no_follow(root: &Path, rel: &str) -> Result<rustix::fd::OwnedFd> {
    use rustix::fs::OFlags;
    let mut dir = rustix::fs::open(
        root,
        OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|e| {
        EngineError::invalid(format!(
            "could not open the space root {}: {e}",
            root.display()
        ))
    })?;
    let mut components: Vec<&str> = rel.split('/').filter(|c| !c.is_empty()).collect();
    components.pop(); // the file name itself is not resolved, only its parents
    for component in components {
        let opened = rustix::fs::openat(
            &dir,
            component,
            OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        );
        match opened {
            Ok(next) => dir = next,
            Err(rustix::io::Errno::NOENT) => {
                // The path-based open creates missing parents; so does this
                // one, with the same component-by-component no-follow rules
                // applying to everything after.
                rustix::fs::mkdirat(&dir, component, rustix::fs::Mode::from_bits_retain(0o777))
                    .map_err(|e| {
                        EngineError::invalid(format!(
                            "could not create the write's parent directory {component}: {e}"
                        ))
                    })?;
                dir = rustix::fs::openat(
                    &dir,
                    component,
                    OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    rustix::fs::Mode::empty(),
                )
                .map_err(|e| {
                    EngineError::invalid(format!(
                        "could not open the write's parent directory {component}: {e}"
                    ))
                })?;
            }
            Err(e) => {
                return Err(EngineError::invalid(format!(
                    "could not open the write's parent directory {component}: {e}"
                )));
            }
        }
    }
    Ok(dir)
}

/// The Windows half of the pinned write: rooted, handle-relative directory
/// resolution and rename (see [`Adoption::in_space`]).
///
/// The user-mode NT functions windows-sys does not carry are declared
/// directly against ntdll; their ABI is stable and documented (ntifs.h).
#[cfg(windows)]
mod nt {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};

    use windows_sys::Win32::Foundation::{HANDLE, NTSTATUS};
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;

    use crate::error::{EngineError, Result};

    #[link(name = "ntdll")]
    extern "system" {
        fn NtCreateFile(
            file_handle: *mut HANDLE,
            desired_access: u32,
            object_attributes: *const OBJECT_ATTRIBUTES,
            io_status_block: *mut IO_STATUS_BLOCK,
            allocation_size: *const i64,
            file_attributes: u32,
            share_access: u32,
            create_disposition: u32,
            create_options: u32,
            ea_buffer: *mut core::ffi::c_void,
            ea_length: u32,
        ) -> NTSTATUS;
        fn NtSetInformationFile(
            file_handle: HANDLE,
            io_status_block: *mut IO_STATUS_BLOCK,
            file_information: *const core::ffi::c_void,
            length: u32,
            file_information_class: u32,
        ) -> NTSTATUS;
    }

    #[repr(C)]
    struct UNICODE_STRING {
        length: u16,
        maximum_length: u16,
        buffer: *mut u16,
    }

    #[repr(C)]
    struct OBJECT_ATTRIBUTES {
        length: u32,
        root_directory: HANDLE,
        object_name: *const UNICODE_STRING,
        attributes: u32,
        security_descriptor: *mut core::ffi::c_void,
        security_quality_of_service: *mut core::ffi::c_void,
    }

    #[repr(C)]
    struct IO_STATUS_BLOCK {
        status: usize,
        information: usize,
    }

    /// `FILE_DISPOSITION_INFORMATION` (ntifs.h): mark a file delete-on-close,
    /// so the last handle close removes it wherever it actually is — no path
    /// re-resolution needed to clean up a payload the move redirected.
    #[repr(C)]
    struct FILE_DISPOSITION_INFORMATION {
        delete_file: u8, // BOOLEAN
    }

    const FILE_DIRECTORY_FILE: u32 = 0x0000_0001;
    const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x0000_0020;
    const FILE_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_OPEN: u32 = 0x0000_0001;
    const FILE_CREATE: u32 = 0x0000_0002;
    const FILE_OVERWRITE_IF: u32 = 0x0000_0005;
    const OBJ_CASE_INSENSITIVE: u32 = 0x40;
    const STATUS_SUCCESS: NTSTATUS = 0;
    const STATUS_OBJECT_NAME_NOT_FOUND: NTSTATUS = 0xC000_0034u32 as i32;
    // FileInformationClass values for NtSetInformationFile.
    const FILE_DISPOSITION_INFORMATION: u32 = 13;
    // List/read a directory, add files and subdirectories to it, delete
    // children (what a rename through the root directory needs), and
    // synchronize on the handle — what the resolution, the staging-file
    // creation and the commit rename below need.
    const DIR_ACCESS: u32 =
        0x0000_0001 | 0x0000_0002 | 0x0000_0004 | 0x0000_0040 | 0x0000_0080 | 0x0010_0000;
    const FILE_SHARE_ALL: u32 = 0x0000_0001 | 0x0000_0002 | 0x0000_0004;

    /// Resolves `rel`'s parent directory under `root`, one component at a
    /// time, never following a reparse point; missing components are
    /// created. The returned handle is what the staging file is created
    /// against and what the commit rename resolves against.
    ///
    /// Both `/` and `\` are separators: the protocol path language knows only
    /// `/`, but `NtCreateFile` splits on both, so a component containing `\`
    /// would otherwise be walked by the kernel through components this walk
    /// never pinned — a junction hiding in one of them could redirect the
    /// staging file outside the space.
    pub(super) fn open_parent_no_follow(root: &std::path::Path, rel: &str) -> Result<OwnedHandle> {
        // The space root itself: opened with `OPEN_REPARSE_POINT` so its own
        // entry being a junction is refused rather than followed. Its
        // ancestors lie outside the space, where the threat model grants a
        // local actor nothing.
        let mut dir = open_relative(
            &root_wide(root),
            None,
            DIR_ACCESS,
            FILE_OPEN,
            FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
        )
        .map_err(|status| {
            EngineError::invalid(format!(
                "could not open the space root {}: NTSTATUS {status:#x}",
                root.display()
            ))
        })?;
        let mut components: Vec<&str> = rel.split(['/', '\\']).filter(|c| !c.is_empty()).collect();
        components.pop(); // the file name itself is not resolved, only its parents
        for component in components {
            let next = match create_relative(&dir, component, FILE_OPEN) {
                Ok(next) => next,
                Err(status) if status == STATUS_OBJECT_NAME_NOT_FOUND => {
                    // The path-based open creates missing parents; so does
                    // this one (`FILE_CREATE` opens the new directory).
                    create_relative(&dir, component, FILE_CREATE).map_err(|status| {
                        EngineError::invalid(format!(
                            "could not create the write's parent directory {component}: \
                             NTSTATUS {status:#x}"
                        ))
                    })?
                }
                Err(status) => {
                    return Err(EngineError::invalid(format!(
                        "could not open the write's parent directory {component}: \
                         NTSTATUS {status:#x}"
                    )));
                }
            };
            dir = next;
        }
        Ok(dir)
    }

    /// The path as a null-terminated Win32 string, for the Win32 APIs — the
    /// `\\?\` extended form as stored, which they accept verbatim.
    fn win32_wide(path: &std::path::Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    /// The Win32 path `root` as an absolute NT path (`\??\`-prefixed), for
    /// the one open that has no parent handle to be relative to.
    ///
    /// The space root is stored canonicalized, which on Windows is the
    /// `\\?\`-prefixed extended-length form — and it may equally arrive as a
    /// plain drive or UNC path — so all four shapes are mapped:
    /// `\\?\C:\...` and `C:\...` to `\??\C:\...`, `\\?\UNC\s\sh\...` and
    /// `\\s\sh\...` to `\??\UNC\s\sh\...`.
    fn root_wide(root: &std::path::Path) -> Vec<u16> {
        let text = root.as_os_str().to_string_lossy();
        let mut out: Vec<u16> = Vec::new();
        let (prefix, rest) = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            (r"\??\UNC\", rest)
        } else if let Some(rest) = text.strip_prefix(r"\\?\") {
            (r"\??\", rest)
        } else if let Some(rest) = text.strip_prefix(r"\\") {
            (r"\??\UNC\", rest)
        } else {
            (r"\??\", text.as_ref())
        };
        out.extend_from_slice(prefix.encode_utf16().collect::<Vec<_>>().as_slice());
        out.extend(rest.encode_utf16());
        out
    }

    /// Opens or creates `name` — relative to `root` when it is set, an
    /// absolute NT path otherwise — with `NtCreateFile`.
    ///
    /// Directories are opened with `FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT`
    /// so a junction is never followed.
    fn open_relative(
        name: &[u16],
        root: Option<&OwnedHandle>,
        access: u32,
        disposition: u32,
        create_options: u32,
    ) -> std::result::Result<OwnedHandle, NTSTATUS> {
        let unicode = UNICODE_STRING {
            length: (name.len() * 2) as u16,
            maximum_length: (name.len() * 2) as u16,
            buffer: name.as_ptr() as *mut u16,
        };
        let attributes = OBJECT_ATTRIBUTES {
            length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
            root_directory: root.map_or(std::ptr::null_mut(), |h| h.as_raw_handle() as HANDLE),
            object_name: &unicode,
            attributes: OBJ_CASE_INSENSITIVE,
            security_descriptor: std::ptr::null_mut(),
            security_quality_of_service: std::ptr::null_mut(),
        };
        let mut handle: HANDLE = std::ptr::null_mut();
        let mut status_block = IO_STATUS_BLOCK {
            status: 0,
            information: 0,
        };
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                access,
                &attributes,
                &mut status_block,
                std::ptr::null(),
                FILE_ATTRIBUTE_NORMAL,
                FILE_SHARE_ALL,
                disposition,
                create_options,
                std::ptr::null_mut(),
                0,
            )
        };
        if status != STATUS_SUCCESS {
            return Err(status);
        }
        Ok(unsafe { OwnedHandle::from_raw_handle(handle as RawHandle) })
    }

    /// Opens or creates `name` inside `dir` as a directory handle, never
    /// following a reparse point.
    fn create_relative(
        dir: &OwnedHandle,
        name: &str,
        disposition: u32,
    ) -> std::result::Result<OwnedHandle, NTSTATUS> {
        let wide: Vec<u16> = name.encode_utf16().collect();
        open_relative(
            &wide,
            Some(dir),
            DIR_ACCESS,
            disposition,
            FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
        )
    }

    /// Creates the staging file inside `dir`, never following a reparse
    /// point at the name (a pre-placed junction is opened as itself and
    /// overwritten in place, never followed).
    pub(super) fn create_staging(dir: &OwnedHandle, name: &str) -> Result<std::fs::File> {
        let wide: Vec<u16> = name.encode_utf16().collect();
        // FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE: read *and* write,
        // because the payload is written here and a multipart completion
        // reads it straight back before the rename.
        // `File::from` takes the handle over from the `OwnedHandle` — one
        // owner, one close.
        let handle = open_relative(
            &wide,
            Some(dir),
            0x0012_0089 | 0x0012_0116 | 0x0001_0000,
            FILE_OVERWRITE_IF,
            FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
        )
        .map_err(|status| {
            EngineError::invalid(format!(
                "could not create the staging file beside {name}: NTSTATUS {status:#x}"
            ))
        })?;
        Ok(std::fs::File::from(handle))
    }

    /// Renames the staging file to `target` — `MoveFileExW`, the same API
    /// `std::fs::rename` uses on Windows — followed by a containment check:
    /// the file that now sits at the intended name must *be* the staging
    /// file, identified by volume and file index (`GetFileInformationByHandle`),
    /// which no pathname spelling — case sensitivity, a junction target's
    /// name — can confound. If the move resolved through a junction swapped
    /// in by a local actor, the file's actual location differs from the
    /// intended one; the staging handle still refers to it wherever it is,
    /// so it is marked delete-on-close there and the write fails closed
    /// instead of silently landing outside the space.
    pub(super) fn rename_into(
        dir: &OwnedHandle,
        file: &std::fs::File,
        staging: &std::path::Path,
        target: &std::path::Path,
    ) -> Result<()> {
        let staging_wide = win32_wide(staging);
        let target_wide = win32_wide(target);
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                staging_wide.as_ptr(),
                target_wide.as_ptr(),
                windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING,
            )
        };
        if ok == 0 {
            // The move could not resolve the staging path — a parent moved
            // out from under the write by a local actor being the dangerous
            // case — and the staging handle refers to the payload wherever
            // it is: mark it delete-on-close so the last close removes it
            // there, and fail closed.
            delete_on_close(file);
            return Err(EngineError::invalid(format!(
                "rename into place failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        let name = target
            .file_name()
            .ok_or_else(|| EngineError::invalid("the target path has no file name"))?;
        let name_wide: Vec<u16> = name.to_string_lossy().encode_utf16().collect();
        // The file at the intended name, resolved relative to the pinned
        // directory — no path is re-resolved. `OPEN_REPARSE_POINT`: a
        // junction swapped in at the name after the move is opened as
        // itself, never followed, so it cannot pass the identity check by
        // pointing back at the payload wherever it landed.
        let opened = open_relative(
            &name_wide,
            Some(dir),
            0x0000_0080 | 0x0010_0000, // FILE_READ_ATTRIBUTES | SYNCHRONIZE
            FILE_OPEN,
            FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
        );
        let contained = match opened {
            Ok(check) => same_file(&check, file),
            Err(_) => false,
        };
        if contained {
            return Ok(());
        }
        // The move landed somewhere else (or the intended name now holds a
        // different file): the staging handle refers to the payload wherever
        // it is — mark it delete-on-close so the last handle close removes
        // it there, no path re-resolution involved, and fail closed.
        delete_on_close(file);
        let actual = final_path(file)
            .map(|p| String::from_utf16_lossy(&p))
            .unwrap_or_else(|_| "an unknown location".into());
        Err(EngineError::invalid(format!(
            "the rename resolved through a swapped junction to {actual} and was marked for \
             deletion; the write failed closed"
        )))
    }

    /// Whether two open handles refer to the same file on the same volume —
    /// the file identity, which pathname spelling cannot affect.
    fn same_file(a: &std::os::windows::io::OwnedHandle, b: &std::fs::File) -> bool {
        let mut info_a =
            windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION::default();
        let mut info_b =
            windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION::default();
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetFileInformationByHandle(
                a.as_raw_handle() as HANDLE,
                &mut info_a,
            ) != 0
                && windows_sys::Win32::Storage::FileSystem::GetFileInformationByHandle(
                    b.as_raw_handle() as HANDLE,
                    &mut info_b,
                ) != 0
        };
        ok && info_a.dwVolumeSerialNumber == info_b.dwVolumeSerialNumber
            && info_a.nFileIndexHigh == info_b.nFileIndexHigh
            && info_a.nFileIndexLow == info_b.nFileIndexLow
    }

    /// Marks `file` delete-on-close: the last handle close removes it
    /// wherever it is on disk, without re-resolving any path.
    fn delete_on_close(file: &std::fs::File) {
        let mut info = FILE_DISPOSITION_INFORMATION { delete_file: 1 };
        let mut status_block = IO_STATUS_BLOCK {
            status: 0,
            information: 0,
        };
        let status = unsafe {
            NtSetInformationFile(
                file.as_raw_handle() as HANDLE,
                &mut status_block,
                (&mut info as *mut FILE_DISPOSITION_INFORMATION).cast(),
                std::mem::size_of::<FILE_DISPOSITION_INFORMATION>() as u32,
                FILE_DISPOSITION_INFORMATION,
            )
        };
        debug_assert_eq!(status, STATUS_SUCCESS);
    }

    /// The normalized final path of an open handle, as UTF-16 (the
    /// `\\?\`-prefixed form), or the failure status. Used for diagnostics
    /// only — the containment verdict is the file identity ([`same_file`]),
    /// never a pathname comparison.
    fn final_path(file: &std::fs::File) -> std::result::Result<Vec<u16>, NTSTATUS> {
        let mut buf = vec![0u16; 4096];
        let len = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW(
                file.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE,
                buf.as_mut_ptr(),
                buf.len() as u32,
                windows_sys::Win32::Storage::FileSystem::FILE_NAME_NORMALIZED,
            )
        };
        if len == 0 {
            return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(1) as NTSTATUS);
        }
        buf.truncate(len as usize);
        Ok(buf)
    }
}

/// How `Adoption::clone_from` managed to give the staging file its head start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloneKind {
    /// Extents shared with the source, copy-on-write: no data was moved.
    Reflink,
    /// The source's bytes were copied.
    Copy,
}

/// Shares a file's extents with another file, where the filesystem can.
///
/// `FICLONE` is Linux's reflink ioctl; every other platform (and every
/// filesystem that does not implement it) reports it unsupported, which is a
/// perfectly good answer — the caller copies instead.
fn reflink_file(source: &std::fs::File, dest: &std::fs::File) -> std::io::Result<()> {
    #[cfg(all(
        target_os = "linux",
        not(any(target_arch = "sparc", target_arch = "sparc64"))
    ))]
    {
        rustix::fs::ioctl_ficlone(dest, source).map_err(std::io::Error::from)
    }
    #[cfg(not(all(
        target_os = "linux",
        not(any(target_arch = "sparc", target_arch = "sparc64"))
    )))]
    {
        let _ = (source, dest);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "reflink is not available on this platform",
        ))
    }
}

impl Drop for Adoption {
    fn drop(&mut self) {
        if self.file.is_some() {
            let _ = std::fs::remove_file(&self.staging);
        }
    }
}

/// One file the walk found: its absolute path, its normalized relative path,
/// and whether it is a symlink.
type Found = (PathBuf, String, bool);

/// True if `path` is one of the paths this pass could not judge, or is under
/// one of them.
///
/// A prefix rule, because `walk` stats a `DirEntry` before it can know whether
/// it is a directory: a *directory* it cannot stat is skipped under its own path
/// and never recursed into, so what is actually at risk is every published file
/// beneath it. Whole components only, so `a/b` does not cover `a/bc`.
fn unjudged_covers(unjudged: &[String], path: &str) -> bool {
    unjudged
        .iter()
        .any(|u| path == u || (path.starts_with(u.as_str()) && path[u.len()..].starts_with('/')))
}

fn walk(
    root: &Path,
    dir: &Path,
    ignore: &IgnoreSet,
    report: &mut ScanReport,
    found: &mut Vec<Found>,
) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    // A failing `DirEntry` fails the space, exactly as a failing `read_dir`
    // does two lines up. `read_dir`'s iterator yields per-entry errors, and
    // discarding them left the child indistinguishable from one that was never
    // there — which the deletion sweep reads as "gone" and publishes a tombstone
    // for. The name is what is unknown here, so no single path can be exempted;
    // `scan_all_with_ingest` already records a failed space and keeps every other
    // space's work, which is the containment this wants.
    let mut sorted = Vec::new();
    for entry in entries {
        sorted.push(entry?);
    }
    sorted.sort_by_key(|e| e.file_name());

    for entry in sorted {
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let rel = match normalize_native_path(relative) {
            Ok(rel) => rel,
            Err(e) => {
                report
                    .skipped
                    .push((relative.to_string_lossy().into_owned(), e.to_string()));
                continue;
            }
        };
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                report.skipped.push((rel, e.to_string()));
                continue;
            }
        };
        let is_dir = metadata.is_dir();
        if ignore.is_ignored(&rel, is_dir) {
            report.ignored += 1;
            continue;
        }
        if is_dir {
            found.push((path.clone(), rel, false));
            walk(root, &path, ignore, report, found)?;
        } else {
            found.push((path, rel, metadata.is_symlink()));
        }
    }
    Ok(())
}

/// The change signal a symlink's target reduces to.
///
/// `local_files.content` holds a content root for a file; for a link it holds
/// the hash of the target, which is the same idea applied to the only content a
/// link has.
pub(crate) fn symlink_signal(target: &str) -> Hash {
    Hash::new(target.as_bytes())
}

/// The signal a published entry implies, matching what `local_files` records.
fn published_signal(entry: &FileEntry) -> Option<Hash> {
    match entry.kind {
        EntryKind::Symlink => entry.symlink_target.as_deref().map(symlink_signal),
        _ => entry.content,
    }
}

pub(crate) fn mtime_nanos(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// A platform file identity, when one is cheaply available.
///
/// Together with size and mtime this is what lets the scanner skip hashing,
/// and what lets a checkout trust a verified file without reading it back.
pub(crate) fn file_identity(metadata: &std::fs::Metadata) -> Option<Vec<u8>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mut id = Vec::with_capacity(16);
        id.extend_from_slice(&metadata.dev().to_le_bytes());
        id.extend_from_slice(&metadata.ino().to_le_bytes());
        Some(id)
    }
    #[cfg(not(unix))]
    {
        // Windows exposes a file index only via `MetadataExt::file_index`,
        // which is still unstable (rust-lang/rust#63010), and reading it
        // otherwise costs an open handle per file. Identity is optional by
        // design, so change detection falls back to (size, mtime) there.
        let _ = metadata;
        None
    }
}

fn unix_mode(metadata: &std::fs::Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(metadata.permissions().mode())
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}

/// Reads a `FileEntry` out of a published entry row's origin trie.
pub fn decode_entry(bytes: &[u8]) -> Result<FileEntry> {
    Ok(synch_core::record::decode(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NodeConfig;
    use crate::testkit::{node_with_space, published, reopen};

    #[test]
    fn empty_adoption_writes_do_not_extend_or_advance_staging() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("empty");
        let mut adoption = Adoption::open(target).unwrap();
        adoption.write_at(100, &[]).unwrap();
        adoption.write(&[]).unwrap();
        assert_eq!(adoption.written(), 0);
        let committed = adoption.commit().unwrap();
        assert_eq!(std::fs::metadata(committed).unwrap().len(), 0);
    }

    #[test]
    fn adoption_applies_preserved_metadata_to_the_committed_inode() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("preserved");
        let mut adoption = Adoption::open(target).unwrap();
        adoption.write(b"body").unwrap();
        let stamp = 1_700_000_000_000_000_000i64;
        adoption.set_metadata(Some(0o755), Some(stamp)).unwrap();
        let committed = adoption.commit().unwrap();
        let metadata = std::fs::metadata(committed).unwrap();
        assert_eq!(mtime_nanos(&metadata), stamp);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(metadata.permissions().mode() & 0o7777, 0o755);
        }
    }

    #[tokio::test]
    async fn api_sources_publish_cas_direct_and_never_scan_a_checkout() {
        let (_data, node) = crate::testkit::node().await;
        node.add_api_source("media").unwrap();
        assert!(node.is_api_source("media").unwrap());
        // The scan path's own re-check: a source that becomes API-only between
        // the caller's listing and the per-space scan is refused, not walked.
        assert!(
            node.scan_space_with_ingest("media", &mut |_| unreachable!("nothing to ingest"))
                .is_err()
        );
        assert!(
            crate::watcher::SpaceWatcher::configured_spaces(&node)
                .unwrap()
                .is_empty()
        );

        let source_dir = tempfile::tempdir().unwrap();
        let source = source_dir.path().join("incoming");
        std::fs::write(&source, b"API source payload").unwrap();
        let (root, size) = node
            .commit_api_file_with_metadata("media", "nested/a.txt", &source, 42, Some(0o755))
            .await
            .unwrap();
        node.flush_staged().await.unwrap().unwrap();

        let entry = published(&node, "media", "nested/a.txt");
        assert_eq!(entry.content, Some(root));
        assert_eq!(entry.size, size);
        assert_eq!(entry.mtime_ns, 42);
        assert_eq!(entry.unix_mode, Some(0o755));
        assert_eq!(node.store().read_all(&root).unwrap(), b"API source payload");
        assert!(
            node.store()
                .local_file("media", "nested/a.txt")
                .unwrap()
                .is_none()
        );

        node.adopt_deletion("media", "nested/a.txt").unwrap();
        node.scan_publish_push().await.unwrap().unwrap();
        assert_eq!(
            published(&node, "media", "nested/a.txt").kind,
            EntryKind::Tombstone
        );
        node.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn detected_local_loss_corrects_the_published_complete_ad() {
        let (data, node) = crate::testkit::node().await;
        node.add_api_source("media").unwrap();
        let incoming = data.path().join("incoming");
        std::fs::write(&incoming, vec![19u8; 100_000]).unwrap();
        let (root, _) = node
            .commit_api_file("media", "lost.bin", &incoming, 42)
            .await
            .unwrap();
        node.flush_staged().await.unwrap().unwrap();
        assert!(
            node.store()
                .provider_for_origin(&root, node.origin())
                .unwrap()
                .unwrap()
                .is_complete()
        );

        let hex = root.to_string();
        std::fs::remove_file(data.path().join("store").join(&hex[..2]).join(&hex)).unwrap();
        assert!(node.store().read_all(&root).is_err());
        assert!(!node.store().local_ad(&root).unwrap().unwrap().is_complete());

        let changes = node.ad_reconciliation_changes().unwrap();
        assert_eq!(changes.len(), 1);
        let corrected: synch_core::BlobAd =
            synch_core::record::decode(changes[0].1.as_ref().expect("the partial replacement ad"))
                .unwrap();
        assert!(!corrected.is_complete());
        node.publish(&changes).unwrap();
        assert!(
            !node
                .store()
                .provider_for_origin(&root, node.origin())
                .unwrap()
                .unwrap()
                .is_complete()
        );

        node.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn api_source_cloud_ingest_is_remote_before_its_row_and_publish() {
        let data = tempfile::tempdir().unwrap();
        Node::init(data.path(), None).unwrap();
        let scratch = data.path().join("cloud-scratch");
        let mut config = NodeConfig::loopback(data.path());
        config.cloud = Some(synch_store::cloud::CloudConfig {
            service: synch_store::cloud::CloudService::Memory,
            options: Default::default(),
            scratch_dir: scratch,
            io_timeout: std::time::Duration::from_secs(5),
            upload_policy: synch_store::cloud::CloudUploadPolicy::OwnPinned,
            cache_bytes: Some(512 * 1024 * 1024),
        });
        let node = Node::open(config).await.unwrap();
        node.add_api_source("media").unwrap();

        let source_dir = tempfile::tempdir().unwrap();
        let source = source_dir.path().join("incoming");
        let payload = vec![13u8; 100_000];
        std::fs::write(&source, &payload).unwrap();
        let (root, size) = node
            .commit_api_file("media", "large.bin", &source, 99)
            .await
            .unwrap();
        let row = node.store().blob(&root).unwrap().unwrap();
        assert!(row.durable, "the row must carry the remote durability ack");
        assert_eq!(row.size, size);
        assert_eq!(
            node.cas_backend()
                .read_range(root, 123, 45_678 - 123)
                .await
                .unwrap(),
            payload[123..45_678]
        );
        assert!(
            node.store()
                .entry(node.origin(), "media", "large.bin")
                .unwrap()
                .is_none(),
            "the entry is not visible before the publish transaction"
        );
        node.flush_staged().await.unwrap().unwrap();
        assert_eq!(published(&node, "media", "large.bin").content, Some(root));

        // Shape a self-readopted head whose older SQLite snapshot lacks the
        // blob row. Maintenance must keep the recovered own ad while the
        // provider-serving path reconstructs the cold row from final keys.
        node.store().force_delete_blob_for_test(&root).unwrap();
        assert!(node.store().blob(&root).unwrap().is_none());
        node.reconstruct_recovered_cloud_rows().await.unwrap();
        assert!(node.ad_reconciliation_changes().unwrap().is_empty());
        let (encoded, served) = node
            .cas_backend()
            .encode_slice(root, synch_core::ChunkRanges::single(0, 1))
            .await
            .unwrap();
        assert_eq!(served, synch_core::ChunkRanges::single(0, 1));
        assert!(!encoded.is_empty());
        assert!(node.store().blob(&root).unwrap().unwrap().durable);

        let pinned = node
            .cas_backend()
            .ingest_bytes(b"b-only recovered pin".to_vec(), now_ns())
            .await
            .unwrap();
        let ad = node.store().local_ad(&pinned.root).unwrap().unwrap();
        node.publish(&[(
            blob_key(&pinned.root),
            Some(postcard::to_stdvec(&ad).unwrap()),
        )])
        .unwrap();
        assert!(!node.store().content_is_referenced(&pinned.root).unwrap());
        node.store()
            .force_delete_blob_for_test(&pinned.root)
            .unwrap();
        assert!(node.store().blob(&pinned.root).unwrap().is_none());
        node.reconstruct_recovered_cloud_rows().await.unwrap();
        let recovered_pin = node.store().blob(&pinned.root).unwrap().unwrap();
        assert!(recovered_pin.durable && recovered_pin.pinned);
        assert!(node.ad_reconciliation_changes().unwrap().is_empty());

        // Simulate a replacement container: only the database and remote
        // object survive. The first read refills the cache.
        node.store().reconcile_scratch_generation("fresh").unwrap();
        assert!(!node.store().blob(&root).unwrap().unwrap().complete);
        assert_eq!(
            node.read_range(
                "media",
                "large.bin",
                &synch_store::VersionPolicy::Origin(node.origin().clone()),
                100,
                Some(900),
            )
            .await
            .unwrap(),
            payload[100..1000]
        );
        let warmed = node.store().blob(&root).unwrap().unwrap();
        assert!(
            !warmed.complete,
            "a cold range read hydrates only its groups"
        );
        assert!(warmed.verified_groups().count() < synch_core::group_count(size));
        node.shutdown().await.unwrap();
    }

    /// Ages every scan record past the racy window: fresh records are racily
    /// clean and re-hashed next scan, which stat-trust tests don't exercise.
    fn age_quick_checks(node: &Node) {
        for space in node.store().sources().unwrap() {
            for mut row in node.store().local_file_rows(&space.space).unwrap() {
                row.scanned_at += super::RACY_WINDOW_NS;
                node.store().put_local_file(&row).unwrap();
            }
        }
    }

    /// A node whose tombstone TTL is set for a test rather than for a cluster.
    async fn node_with_ttl(
        ttl: std::time::Duration,
    ) -> (tempfile::TempDir, tempfile::TempDir, Node) {
        let data = tempfile::tempdir().unwrap();
        let space = tempfile::tempdir().unwrap();
        Node::init(data.path(), None).unwrap();
        let mut config = NodeConfig::loopback(data.path());
        config.tombstone_ttl = ttl;
        let node = Node::open(config).await.unwrap();
        node.add_filesystem_source("media", space.path()).unwrap();
        (data, space, node)
    }

    /// Rewrites a tombstone's deletion time, like one 90 days old without the wait.
    fn backdate_tombstone(node: &Node, path: &str, mtime_ns: i64) {
        let row = published(node, "media", path);
        assert_eq!(row.kind, EntryKind::Tombstone);
        node.store()
            .put_entry(
                node.origin(),
                "media",
                path,
                &FileEntry::tombstone(mtime_ns, row.seq, row.prev),
            )
            .unwrap();
    }

    fn in_root(node: &Node, root: Hash, path: &str) -> bool {
        Trie::new(node.store().as_ref())
            .get(root, &file_key("media", path).unwrap())
            .unwrap()
            .is_some()
    }

    #[tokio::test]
    async fn scans_hashes_and_publishes() {
        let (_d, space, node) = node_with_space().await;
        std::fs::create_dir_all(space.path().join("talks")).unwrap();
        std::fs::write(space.path().join("a.txt"), b"hello").unwrap();
        std::fs::write(space.path().join("talks/b.bin"), vec![7u8; 40_000]).unwrap();
        // The walk honors the space's ignore set.
        std::fs::write(space.path().join(".syncignore"), "*.tmp\n").unwrap();
        std::fs::write(space.path().join("scratch.tmp"), b"x").unwrap();

        let (report, head) = node.scan_and_publish().unwrap();
        assert_eq!(report.hashed, 2);
        assert!(report.ignored >= 1);
        assert_eq!(report.unchanged, 0);
        let head = head.unwrap();
        assert_eq!(head.seq, 1);

        let entries = node
            .store()
            .list_entries(Some(node.origin()), "media", "", None, None)
            .unwrap();
        assert_eq!(entries.len(), 2);
        let a = entries.iter().find(|e| e.path == "a.txt").unwrap();
        assert_eq!(a.size, 5);
        assert_eq!(a.content, Some(Hash::new(b"hello")));
        // Content is in the CAS, reads back verified, and is advertised as a provider.
        assert_eq!(
            node.store().read_all(&a.content.unwrap()).unwrap(),
            b"hello"
        );
        assert_eq!(
            node.store().providers(&a.content.unwrap()).unwrap().len(),
            1
        );
        node.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_racy_same_size_rewrite_is_still_detected() {
        // A same-size rewrite sharing the hashed mtime: inside the racy window
        // the stat proves nothing, so the bytes are hashed again.
        let (_d, space, node) = node_with_space().await;
        let path = space.path().join("rolling.txt");
        std::fs::write(&path, b"revision 1").unwrap();
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        let (_, head) = node.scan_and_publish().unwrap();
        assert_eq!(head.unwrap().seq, 1);

        std::fs::write(&path, b"revision 2").unwrap();
        let times = std::fs::FileTimes::new().set_modified(mtime);
        std::fs::File::options()
            .append(true)
            .open(&path)
            .unwrap()
            .set_times(times)
            .unwrap();

        let (report, head) = node.scan_and_publish().unwrap();
        assert_eq!(report.hashed, 1, "a racily clean stat proves nothing");
        assert_eq!(head.unwrap().seq, 2);
        assert_eq!(
            published(&node, "media", "rolling.txt").content,
            Some(Hash::new(b"revision 2"))
        );

        // The refreshed record ages into a trustworthy stat.
        age_quick_checks(&node);
        let (report, head) = node.scan_and_publish().unwrap();
        assert_eq!(report.hashed, 0);
        assert_eq!(report.unchanged, 1);
        assert!(head.is_none());
        node.shutdown().await.unwrap();
    }

    /// Edits are detected with prev lineage at the replaced content; deletions
    /// become tombstones distinguishing "deleted at seq N" from "never existed".
    #[tokio::test]
    async fn edits_and_deletions_flow_through_the_lifecycle() {
        let (_d, space, node) = node_with_space().await;
        std::fs::write(space.path().join("a.txt"), b"before").unwrap();
        node.scan_and_publish().unwrap();

        // A distinguishable mtime, so the stat triple differs even on coarse clocks.
        std::fs::write(space.path().join("a.txt"), b"after!!").unwrap();
        let (report, head) = node.scan_and_publish().unwrap();
        assert_eq!(report.hashed, 1);
        assert_eq!(head.unwrap().seq, 2);
        let entry = published(&node, "media", "a.txt");
        assert_eq!(entry.content, Some(Hash::new(b"after!!")));
        assert_eq!(entry.prev, Some(Hash::new(b"before")));

        // The delete phase of the same lifecycle.
        std::fs::remove_file(space.path().join("a.txt")).unwrap();
        let (report, head) = node.scan_and_publish().unwrap();
        assert_eq!(report.deleted, 1);
        assert_eq!(head.unwrap().seq, 3);
        let entry = published(&node, "media", "a.txt");
        assert_eq!(entry.kind, EntryKind::Tombstone);
        assert_eq!(entry.seq, 3);
        assert_eq!(entry.prev, Some(Hash::new(b"after!!")));
        node.shutdown().await.unwrap();
    }

    #[test]
    fn the_unjudged_exemption_covers_whole_subtrees_only() {
        let unjudged = vec!["d/sub".to_string(), "solo.txt".to_string()];
        // The directory itself, and anything under it at any depth.
        assert!(unjudged_covers(&unjudged, "d/sub"));
        assert!(unjudged_covers(&unjudged, "d/sub/deep.txt"));
        assert!(unjudged_covers(&unjudged, "solo.txt"));
        // Whole components only: a shared-prefix sibling is not covered, or
        // one unreadable directory would freeze its neighbours' deletions.
        assert!(!unjudged_covers(&unjudged, "d/subterfuge.txt"));
        assert!(!unjudged_covers(&unjudged, "solo.txt.bak"));
        assert!(!unjudged_covers(&[], "anything"));
    }

    /// A path the walk could not judge is not tombstoned: a published file
    /// whose stat fails (an unreadable parent) is skipped, not swept as gone.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_path_the_walk_could_not_stat_is_not_deleted() {
        use std::os::unix::fs::PermissionsExt;
        // Root bypasses the EACCES this relies on; detect it by trying, not uid.
        let probe = tempfile::tempdir().unwrap();
        std::fs::write(probe.path().join("p"), b"x").unwrap();
        std::fs::set_permissions(probe.path(), std::fs::Permissions::from_mode(0o444)).unwrap();
        let bypassed = std::fs::symlink_metadata(probe.path().join("p")).is_ok();
        std::fs::set_permissions(probe.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        if bypassed {
            eprintln!("skipped: this user bypasses the EACCES this test needs");
            return;
        }
        let (_d, space, node) = node_with_space().await;
        // Both shapes: directly under and under a subdir — `walk` stats a
        // DirEntry before it recurses.
        let dir = space.path().join("d");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("keep.txt"), b"hello").unwrap();
        std::fs::write(dir.join("sub").join("deep.txt"), b"deeper").unwrap();
        node.scan_and_publish().unwrap();
        for path in ["d/keep.txt", "d/sub/deep.txt"] {
            assert_eq!(
                published(&node, "media", path).kind,
                EntryKind::File,
                "{path} must be published before the directory is locked"
            );
        }

        // Listable but not traversable: `read_dir` yields names, `symlink_metadata` fails.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o444)).unwrap();
        let report = node.scan_all().unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(
            !report.skipped.is_empty(),
            "the scan must report the path it could not judge"
        );
        assert_eq!(report.deleted, 0, "{:?}", report.skipped);
        for path in ["keep.txt", "deep.txt"] {
            assert!(
                !report
                    .staged
                    .iter()
                    .any(|(key, _)| String::from_utf8_lossy(key).contains(path)),
                "no tombstone may be staged for {path}, which is still there"
            );
        }
        node.shutdown().await.unwrap();
    }

    /// The gates travel with the write, so they are re-taken at the rename
    /// rather than only at the open.
    ///
    /// That is the whole point of `Adoption` carrying them: a write can be open
    /// for as long as its client takes — an S3 body, a multipart assembly — and
    /// the node it commits into is not the node it opened against. Removing the
    /// check in `commit_inner` fails this.
    #[tokio::test]
    async fn a_space_write_re_takes_its_gates_at_the_commit() {
        let (_d, space, node) = crate::testkit::node_with_space().await;
        let victim = space.path().join("f.txt");
        std::fs::write(&victim, b"mine, unpublished").unwrap();

        // Opened against a healthy node, and written.
        let mut adoption = node.open_adoption("media", "f.txt").unwrap();
        adoption.write(b"theirs").unwrap();

        // A peer advertises a head for our own origin that we have no history
        // for: key-loss recovery, arriving while the body was on the wire.
        node.store()
            .record_observed_head(
                node.origin(),
                100,
                &synch_core::Hash([7u8; 32]),
                true,
                None,
                now_ns(),
            )
            .unwrap();

        let refused = adoption.commit().unwrap_err().to_string();
        assert!(refused.contains("recover"), "{refused}");
        assert_eq!(
            std::fs::read(&victim).unwrap(),
            b"mine, unpublished",
            "the rename must not have landed on a file nothing could then \
             publish a replacement for"
        );
        node.shutdown().await.unwrap();
    }

    /// A key the platform reads as more than one relative component is
    /// refused (Windows backslash and drive forms; POSIX sees ordinary
    /// components, so its own path rules still refuse what matters).
    #[tokio::test]
    async fn an_adoption_target_stays_inside_its_space() {
        let (_d, _space, node) = node_with_space().await;
        // Against the root `add_space` recorded (`canonical_dir` resolves
        // `/var` to `/private/var` on macOS).
        let root = PathBuf::from(
            node.store()
                .source("media")
                .unwrap()
                .unwrap()
                .local_path
                .unwrap(),
        );
        let inside = node.adoption_target("media", "sub/ok.txt").unwrap();
        assert_eq!(inside, root.join("sub").join("ok.txt"));

        #[cfg(windows)]
        for escape in [
            "..\\..\\evil.txt",
            "C:/Windows/Temp/evil.txt",
            "\\\\srv\\s\\x",
        ] {
            assert!(
                node.adoption_target("media", escape).is_err(),
                "{escape} must not resolve outside the space"
            );
        }
        assert!(node.adoption_target("media", "../evil.txt").is_err());
        assert!(node.adoption_target("media", "/etc/passwd").is_err());
        node.shutdown().await.unwrap();
    }

    /// A key with a platform separator inside it (`a\b`) must pin `a` like
    /// `a/b` does — the kernel's `NtCreateFile` splits on both separators,
    /// so a component containing `\` handed to it whole would be walked
    /// through components the pinning walk never resolved. Writing
    /// `sub\evil.txt` must land at `root/sub/evil.txt` and refuse a junction
    /// at `sub`, exactly like the `/` form.
    #[cfg(windows)]
    #[tokio::test]
    async fn a_backslash_key_is_pinned_like_a_slash_key() {
        let (_d, _space, node) = node_with_space().await;
        let root = PathBuf::from(
            node.store()
                .source("media")
                .unwrap()
                .unwrap()
                .local_path
                .unwrap(),
        );
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();

        // A junction at `sub` must be refused rather than followed: `sub` is
        // a real directory, so the write lands at the intended path. (The
        // junction case itself is covered by `a_swapped_parent_cannot_
        // redirect_the_commit` on Unix; the Windows CI runs the escape
        // matrix above.)
        let mut adoption = node.open_adoption("media", r"sub\evil.txt").unwrap();
        adoption.write(b"the payload").unwrap();
        adoption.commit().unwrap();
        assert_eq!(
            std::fs::read(root.join("sub").join("evil.txt")).unwrap(),
            b"the payload"
        );
        node.shutdown().await.unwrap();
    }

    /// The escape guard is not a one-shot check at open: a parent directory
    /// swapped for a symlink while a body is in flight must not redirect the
    /// commit outside the space (§9.4). On Linux the rename resolves against
    /// the directory pinned at open; elsewhere the guard is re-run in the
    /// same blocking step as the rename.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_swapped_parent_cannot_redirect_the_commit() {
        let (_d, _space, node) = node_with_space().await;
        let root = PathBuf::from(
            node.store()
                .source("media")
                .unwrap()
                .unwrap()
                .local_path
                .unwrap(),
        );
        let sub = root.join("sub");
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(&sub).unwrap();

        // Open the write while `sub` is a real directory, then replace it
        // with a symlink pointing outside the space — the swap a local actor
        // with write access to the space could perform between the open-time
        // check and the commit rename. The staging file sits inside `sub`, so
        // the actor clears it first (unlinking an in-flight staging file is
        // within the same grant).
        let adoption = node.open_adoption("media", "sub/escape.txt").unwrap();
        for entry in std::fs::read_dir(&sub).unwrap() {
            std::fs::remove_file(entry.unwrap().path()).unwrap();
        }
        std::fs::remove_dir(&sub).unwrap();
        std::os::unix::fs::symlink(outside.path(), &sub).unwrap();
        let mut adoption = adoption;
        adoption.write(b"the payload").unwrap();

        let outcome = adoption.commit();
        // The write must not land outside the space under any platform: Unix
        // resolves the parent with `O_NOFOLLOW` at open and renames against
        // the pinned directory (the staging name was unlinked, so the commit
        // fails `ENOENT`); Windows re-runs the guard in the same blocking
        // step as the rename and refuses.
        assert!(
            !outside.path().join("escape.txt").exists(),
            "the commit must not land the object outside the space"
        );
        assert!(
            outcome.is_err(),
            "the redirected write must not commit as if nothing happened"
        );
        node.shutdown().await.unwrap();
    }

    /// The pinned directory pins an inode, not a place in the tree: a parent
    /// directory *renamed* — staging file still inside — to another location
    /// while the body streams takes the commit's `renameat` along with it.
    /// The post-commit verification must notice the object is not at the
    /// intended path, remove it where the moved directory actually is, and
    /// fail the write (§9.4).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_parent_renamed_mid_stream_cannot_redirect_the_commit() {
        let (_d, _space, node) = node_with_space().await;
        let root = PathBuf::from(
            node.store()
                .source("media")
                .unwrap()
                .unwrap()
                .local_path
                .unwrap(),
        );
        let sub = root.join("sub");
        let moved = root.join("moved");
        std::fs::create_dir_all(&sub).unwrap();

        // Open the write while `sub` is a real directory, then rename the
        // whole directory — nonempty: the staging file is inside it — to
        // another location while the body streams, as a local actor with
        // write access to the space could.
        let mut adoption = node.open_adoption("media", "sub/escape.txt").unwrap();
        std::fs::rename(&sub, &moved).unwrap();
        adoption.write(b"the payload").unwrap();

        let outcome = adoption.commit();
        assert!(
            outcome.is_err(),
            "a commit whose parent moved mid-stream must not succeed"
        );
        assert!(
            !moved.join("escape.txt").exists(),
            "the object must be removed from where the moved directory took it"
        );
        assert!(
            !sub.join("escape.txt").exists(),
            "and nothing may appear at the intended path either"
        );
        node.shutdown().await.unwrap();
    }

    /// A path re-created while its own tombstone is expiring survives the
    /// batch: the scan's live entry restating the key beats the raw removal.
    #[tokio::test]
    async fn a_path_re_created_as_its_tombstone_expires_is_still_published() {
        let (_d, space, node) = node_with_ttl(std::time::Duration::from_secs(3600)).await;
        std::fs::write(space.path().join("a.txt"), b"first").unwrap();
        node.scan_and_publish().unwrap();
        std::fs::remove_file(space.path().join("a.txt")).unwrap();
        node.scan_and_publish().unwrap();
        assert_eq!(
            published(&node, "media", "a.txt").kind,
            EntryKind::Tombstone
        );

        // The tombstone ages out and the file returns before the next scan
        // sees either — the whole of the trigger.
        backdate_tombstone(&node, "a.txt", now_ns() - 2 * 3600 * 1_000_000_000);
        std::fs::write(space.path().join("a.txt"), b"second").unwrap();

        let report = node.scan_all().unwrap();
        assert_eq!(
            report.expired, 0,
            "the removal gives way to the entry restating the key"
        );
        node.publish(&report.staged).unwrap();

        let entry = published(&node, "media", "a.txt");
        assert_eq!(entry.kind, EntryKind::File);
        assert_eq!(entry.content, Some(Hash::new(b"second")));
    }

    /// A deletion whose tombstone never reached a root is re-derived from the
    /// published tree by the next scan, and a tombstoned path is not swept
    /// again (which would republish a deletion forever).
    #[tokio::test]
    async fn a_deletion_whose_batch_was_lost_is_re_derived_by_the_next_scan() {
        let (_d, space, node) = node_with_space().await;
        std::fs::write(space.path().join("a.txt"), b"hello").unwrap();
        node.scan_and_publish().unwrap();

        // The deletion is scanned but its batch never publishes.
        std::fs::remove_file(space.path().join("a.txt")).unwrap();
        let lost = node.scan_all().unwrap();
        assert_eq!(lost.deleted, 1, "the scan saw the deletion");
        assert!(node.store().local_files("media").unwrap().is_empty());
        assert_eq!(
            published(&node, "media", "a.txt").kind,
            EntryKind::File,
            "and it is still published as live"
        );

        // The next scan re-derives it from the published tree.
        let (report, head) = node.scan_and_publish().unwrap();
        assert_eq!(report.deleted, 1);
        assert_eq!(
            published(&node, "media", "a.txt").kind,
            EntryKind::Tombstone
        );
        assert_eq!(head.unwrap().seq, 2);

        // And a tombstoned path is not swept again.
        let (again, head) = node.scan_and_publish().unwrap();
        assert_eq!(again.deleted, 0);
        assert!(head.is_none());
        node.shutdown().await.unwrap();
    }

    /// The streamed form of adoption (§9.4): bytes go from the CAS into the
    /// space a piece at a time, through an invisible staging file that is gone
    /// by the time the adoption returns.
    #[tokio::test]
    async fn adopting_a_peer_version_streams_it_into_the_space() {
        let (_d, space, node) = node_with_space().await;
        std::fs::write(space.path().join("a.txt"), b"mine").unwrap();
        node.scan_and_publish().unwrap();

        let peer = synch_core::OriginId::named("nas", "x.example").unwrap();
        let payload: Vec<u8> = (0..1_200_000u32).map(|i| (i * 11 % 251) as u8).collect();
        let root = node.store().ingest_bytes(&payload, now_ns()).unwrap();
        node.store()
            .put_entry(
                &peer,
                "media",
                "a.txt",
                &FileEntry::file(payload.len() as u64, 9_000, root, 4),
            )
            .unwrap();

        let target = node.adopt_from(&peer, "media", "a.txt").await.unwrap();
        // The path under the *stored* root, canonicalized at `source add`
        // (macOS `/var` -> `/private/var`).
        let canonical_space = space.path().canonicalize().unwrap();
        assert_eq!(target, canonical_space.join("a.txt"));
        assert_eq!(std::fs::read(&target).unwrap(), payload);
        let left: Vec<String> = std::fs::read_dir(space.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, vec!["a.txt".to_string()]);

        node.scan_and_publish().unwrap();
        let entry = published(&node, "media", "a.txt");
        assert_eq!(entry.content, Some(root));
        assert_eq!(entry.prev, Some(Hash::new(b"mine")));

        // A path outside every filesystem source is refused before fetching.
        assert!(node.adopt_from(&peer, "absent", "a.txt").await.is_err());
        node.shutdown().await.unwrap();
    }

    /// §4.2: tombstones are retained for `tombstone_ttl`, then dropped in a
    /// later root — only the aged ones — and expiring one is not forbidding
    /// the path: creating it again republishes it, lineage starting over.
    #[tokio::test]
    async fn tombstone_ttl_expires_aged_keys_and_allows_recreation() {
        let (_d, space, node) = node_with_ttl(std::time::Duration::from_secs(3600)).await;
        std::fs::write(space.path().join("old.txt"), b"old").unwrap();
        std::fs::write(space.path().join("recent.txt"), b"recent").unwrap();
        node.scan_and_publish().unwrap();

        std::fs::remove_file(space.path().join("old.txt")).unwrap();
        std::fs::remove_file(space.path().join("recent.txt")).unwrap();
        let (report, head) = node.scan_and_publish().unwrap();
        assert_eq!(report.deleted, 2);
        assert_eq!(report.expired, 0, "both tombstones are brand new");
        assert!(in_root(&node, head.unwrap().root, "old.txt"));

        // One of them is now older than the TTL; the other is not.
        backdate_tombstone(&node, "old.txt", now_ns() - 2 * 3600 * 1_000_000_000);
        let (report, head) = node.scan_and_publish().unwrap();
        assert_eq!(report.expired, 1);
        let root = head.unwrap().root;
        assert!(
            !in_root(&node, root, "old.txt"),
            "the aged key must be gone"
        );
        assert!(in_root(&node, root, "recent.txt"), "the fresh one stays");
        assert!(
            node.store()
                .entry(node.origin(), "media", "old.txt")
                .unwrap()
                .is_none()
        );

        // Nothing is left to expire, so a further scan mints nothing.
        let (report, head) = node.scan_and_publish().unwrap();
        assert_eq!(report.expired, 0);
        assert!(head.is_none());

        // The expired path can be created again as an ordinary entry.
        std::fs::write(space.path().join("old.txt"), b"again").unwrap();
        let (report, head) = node.scan_and_publish().unwrap();
        assert_eq!(report.hashed, 1);
        let root = head.unwrap().root;
        assert!(in_root(&node, root, "old.txt"));
        let entry = published(&node, "media", "old.txt");
        assert_eq!(entry.kind, EntryKind::File);
        assert_eq!(entry.content, Some(Hash::new(b"again")));
        assert_eq!(entry.prev, None, "what it replaced is no longer published");
        node.shutdown().await.unwrap();
    }

    /// The maintenance path: expiry stages into the publisher, so it costs one
    /// head like any other batch and needs no scan of its own.
    #[tokio::test]
    async fn maintenance_stages_expiry_through_the_publisher() {
        // A TTL of zero makes every tombstone expired the moment it exists.
        let (_d, space, node) = node_with_ttl(std::time::Duration::ZERO).await;
        std::fs::write(space.path().join("a.txt"), b"x").unwrap();
        node.scan_and_publish().unwrap();
        std::fs::remove_file(space.path().join("a.txt")).unwrap();
        node.scan_and_publish().unwrap();
        assert_eq!(
            published(&node, "media", "a.txt").kind,
            EntryKind::Tombstone
        );

        assert_eq!(node.publisher().pending(), 0);
        node.maintenance_pass().unwrap();
        assert_eq!(node.publisher().pending(), 1, "staged, not published");

        let head = node.flush_staged().await.unwrap().unwrap();
        assert!(!in_root(&node, head.root, "a.txt"));
        assert!(
            node.store()
                .entry(node.origin(), "media", "a.txt")
                .unwrap()
                .is_none()
        );
        node.shutdown().await.unwrap();
    }

    /// A replicated trie belongs to its origin: expiry never touches it.
    #[tokio::test]
    async fn expiry_never_touches_another_origin() {
        let (_d, _space, node) = node_with_ttl(std::time::Duration::ZERO).await;
        let peer = synch_core::OriginId::named("laptop", "x.example").unwrap();
        node.store()
            .put_entry(
                &peer,
                "media",
                "theirs.txt",
                &FileEntry::tombstone(0, 1, None),
            )
            .unwrap();

        assert!(node.expired_tombstone_changes().unwrap().is_empty());
        assert_eq!(node.expire_tombstones().unwrap(), 0);
        assert!(
            node.store()
                .entry(&peer, "media", "theirs.txt")
                .unwrap()
                .is_some()
        );
        node.shutdown().await.unwrap();
    }

    /// §7.1: on open, `local_files` rows the published root does not
    /// corroborate are dropped so the next scan re-indexes them; corroborated
    /// rows — a published symlink's included — survive an ordinary restart.
    #[tokio::test]
    async fn open_time_reconciliation_drops_unpublished_rows_only() {
        let data = tempfile::tempdir().unwrap();
        let space = tempfile::tempdir().unwrap();
        Node::init(data.path(), None).unwrap();
        let rows = if cfg!(unix) { 2 } else { 1 };
        {
            let node = Node::open(NodeConfig::loopback(data.path())).await.unwrap();
            node.add_filesystem_source("media", space.path()).unwrap();
            std::fs::write(space.path().join("a.txt"), b"hello").unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink("a.txt", space.path().join("link")).unwrap();
            // The batch is lost: hashed and recorded, never published.
            let report = node.scan_and_stage_async().await.unwrap();
            assert_eq!(report.hashed, rows);
            assert!(node.publisher().pending() > 0);
            assert!(node.own_head().unwrap().is_none());
            assert_eq!(node.store().local_files("media").unwrap().len(), rows);
            node.shutdown().await.unwrap();
        }

        // Opening drops the rows the trie does not publish, so the next scan
        // re-hashes instead of skipping forever.
        let node = reopen(data.path()).await;
        assert!(node.store().local_files("media").unwrap().is_empty());
        let (report, head) = node.scan_and_publish().unwrap();
        assert_eq!(
            report.hashed, rows,
            "the file must be re-hashed, not skipped"
        );
        assert_eq!(head.unwrap().seq, 1);
        node.shutdown().await.unwrap();

        // A published tree keeps every row: ordinary restarts do not re-hash.
        let node = reopen(data.path()).await;
        assert_eq!(node.reconcile_local_files().unwrap(), 0);
        assert_eq!(node.store().local_files("media").unwrap().len(), rows);
        age_quick_checks(&node);
        let (report, head) = node.scan_and_publish().unwrap();
        assert_eq!(report.hashed, 0);
        assert_eq!(report.unchanged, rows);
        assert!(head.is_none());
        node.shutdown().await.unwrap();
    }

    /// §10: the scan runs off the runtime. A current-thread runtime makes this
    /// decisive: a ticker task is polled orders of magnitude more often when
    /// the hashing thread is free than when a worker does the hashing itself.
    #[tokio::test]
    async fn a_scan_does_not_block_the_runtime() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let (_d, space, node) = node_with_space().await;
        // Enough bytes that the hashing is unmistakably longer than a poll.
        for i in 0..32 {
            std::fs::write(
                space.path().join(format!("f{i}.bin")),
                vec![i as u8; 512 * 1024],
            )
            .unwrap();
        }

        let ticks = std::sync::Arc::new(AtomicUsize::new(0));
        let ticker = {
            let ticks = ticks.clone();
            tokio::spawn(async move {
                loop {
                    ticks.fetch_add(1, Ordering::Relaxed);
                    tokio::task::yield_now().await;
                }
            })
        };
        // Let the ticker reach its loop before the measurement starts.
        tokio::task::yield_now().await;

        let before = ticks.load(Ordering::Relaxed);
        let report = node.scan_and_stage_async().await.unwrap();
        let after = ticks.load(Ordering::Relaxed);

        // Calibrated to failure modes, not machine speed: an inline hash ticks
        // under a hundred times for 32 files; a free thread ~9,000 on CI runners.
        const FREE_RUNTIME_TICKS: usize = 1_000;

        assert_eq!(report.hashed, 32);
        assert!(
            after - before > FREE_RUNTIME_TICKS,
            "the runtime was barely polling while the scan ran ({before} -> {after}): the hashing is back on a runtime worker"
        );
        ticker.abort();
        node.shutdown().await.unwrap();
    }

    /// The symlink lifecycle (§7.1): tracked by lstat mtime and target signal,
    /// so an unchanged link stages nothing, a retarget stages an update, and
    /// a removed link is tombstoned — the `local_files` row is what makes the
    /// deletion sweep see the path at all.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_symlink_lifecycle() {
        let (_d, space, node) = node_with_space().await;
        std::fs::write(space.path().join("a.txt"), b"a").unwrap();
        std::fs::write(space.path().join("b.txt"), b"b").unwrap();
        std::os::unix::fs::symlink("a.txt", space.path().join("link")).unwrap();

        let (report, head) = node.scan_and_publish().unwrap();
        assert_eq!(report.hashed, 3, "the two files and the link");
        assert_eq!(head.unwrap().seq, 1);
        let entry = published(&node, "media", "link");
        assert_eq!(entry.kind, EntryKind::Symlink);
        assert_eq!(entry.symlink_target.as_deref(), Some("a.txt"));
        // The link's own lstat mtime, never `now_ns()`.
        let lstat = mtime_nanos(&std::fs::symlink_metadata(space.path().join("link")).unwrap());
        assert_eq!(entry.mtime_ns, lstat);

        // An unchanged link stages nothing.
        age_quick_checks(&node);
        let (again, head) = node.scan_and_publish().unwrap();
        assert_eq!(again.hashed, 0);
        assert_eq!(again.unchanged, 3);
        assert!(again.staged.is_empty(), "{:?}", again.staged);
        assert!(head.is_none(), "an unchanged tree publishes no head");

        // Retargeting moves the signal and stages an update.
        std::fs::remove_file(space.path().join("link")).unwrap();
        std::os::unix::fs::symlink("b.txt", space.path().join("link")).unwrap();
        let (report, head) = node.scan_and_publish().unwrap();
        assert_eq!(report.hashed, 1, "only the link moved");
        assert_eq!(head.unwrap().seq, 2);
        assert_eq!(
            published(&node, "media", "link").symlink_target.as_deref(),
            Some("b.txt")
        );

        // Removing the link is tombstoned, and its row goes with it.
        std::fs::remove_file(space.path().join("link")).unwrap();
        let (report, head) = node.scan_and_publish().unwrap();
        assert_eq!(report.deleted, 1);
        assert!(head.is_some());
        assert_eq!(published(&node, "media", "link").kind, EntryKind::Tombstone);
        assert!(node.store().local_file("media", "link").unwrap().is_none());
        node.shutdown().await.unwrap();
    }
}

/// The symlink-escape guard, exercised where a symlink can be made (creating
/// one to test against is not cross-platform).
#[cfg(all(test, unix))]
mod escape_tests {
    use crate::testkit::node_with_space;

    /// Without the ancestor check, a client naming a key could write and
    /// delete anywhere the daemon's uid reaches.
    #[tokio::test]
    async fn a_symlinked_directory_cannot_be_written_or_deleted_through() {
        let (_d, space, node) = node_with_space().await;
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), b"not yours").unwrap();
        std::os::unix::fs::symlink(outside.path(), space.path().join("escape")).unwrap();

        assert!(node.open_adoption("media", "escape/secret").is_err());
        assert!(node.adopt_deletion("media", "escape/secret").is_err());
        assert!(
            node.adopt_deletion("media", "escape/nested/deep.txt")
                .is_err()
        );
        // The file outside the space is untouched by all of that.
        assert_eq!(
            std::fs::read(outside.path().join("secret")).unwrap(),
            b"not yours"
        );
        // Ordinary paths, and a symlink that *is* the final component, still work.
        assert!(node.open_adoption("media", "ordinary.txt").is_ok());
        node.shutdown().await.unwrap();
    }
}
