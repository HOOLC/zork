//! One-shot materialization into a space's *own* directory (§7.2).
//!
//! A checkout is the continuous, read-only half of materialization: it owns the
//! directory it writes into, removes whatever the tree stops carrying, and is
//! never indexed back into the local origin trie. A tree adoption is the other half. It
//! writes into the writable directory `synch source add` named — the one this
//! node publishes from — and so it may only ever *add*:
//!
//! - nothing is removed, ever. A tombstoned version, a path the policy selects
//!   nothing for, a local file no origin publishes: all left exactly as they
//!   are. Deleting is what `synch adopt path` of a tombstone is for, one deliberate
//!   path at a time.
//! - a local file whose bytes already differ is reported and left alone.
//!   `--replace` replaces it, which is the bulk form of `synch adopt path`.
//!
//! Nothing here publishes, and a node that *cannot* publish does not tree adoption at
//! all: a scan would refuse there too, so the tree would sit unannounced — and
//! `--replace`'s own-origin guard, which needs this node to publish something,
//! would be inert while it wrote. `synch recover` first (§3.4).
//!
//! Nothing here publishes. The files land in an indexed directory, so the next
//! scan — the watcher's, or an explicit `synch source scan` — stages and publishes
//! them as this node's own view (§7.1), exactly as it would files copied in by
//! hand.
//!
//! Which is why a adopted file carries the selected version's metadata as well
//! as its bytes. The mtime that scan publishes is then the one the origin
//! published, so adopting a path restates the version that was adopted instead
//! of minting a newer one — and minting one would be no small thing: `newest`
//! orders on `(mtime, content root, origin)`, so a tree adoption stamped with the wall
//! clock would make this node win the selection for every path it touched,
//! cluster-wide.
//!
//! Filesystems coarsen the stamp — NTFS to 100 ns, HFS+ to whole seconds — and
//! only ever downward, so what the next scan publishes can sit a tick below the
//! version that was adopted. That direction is harmless: an mtime is not part of
//! a version's identity (`Version`, synch-store), so the two origins still
//! collapse to one version with two attestors, and being fractionally *older*
//! is the safe way to be wrong about a `newest` order.
//!
//! A symbolic link is the exception, and cannot help being one: stamping a
//! link's own times needs a facility the standard library does not expose
//! (§7.2), so a adopted link does publish as newer than the version it came
//! from. The consequence is bounded — a link's target *is* its version, and the
//! target is identical, so every node still converges on the same link — but
//! the "restates rather than mints" rule above is a rule about files.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use synch_core::{EntryKind, Hash};
use synch_store::{Donor, EntryRow, VersionPolicy, VersionSet};

use crate::{
    checkout::{apply_metadata, escapes_via_symlink, materialize_symlink, Metadata},
    error::{EngineError, Result},
    ignore::IgnoreSet,
    node::Node,
    scanner::target_within,
};

#[cfg(windows)]
use crate::checkout::unsafe_name;

/// How a tree adoption treats what is already on disk.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AdoptTreeOptions {
    /// Replace local files whose bytes differ from the selected version,
    /// rather than reporting them and leaving them alone.
    pub replace: bool,
    /// Decide everything and write nothing: the report says what a real run
    /// would do, down to which files it would replace.
    pub dry_run: bool,
}

/// What one tree adoption did, or — under [`AdoptTreeOptions::dry_run`] — would do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdoptTreeReport {
    /// Files and symlinks written: the ones that were missing, plus whatever
    /// `--replace` replaced.
    pub adopted: usize,
    /// Paths already holding the selected version's bytes.
    pub current: usize,
    /// The paths `--replace` overwrote — the subset of `adopted` that had
    /// different content here first. Named rather than counted: replacing a
    /// local file is the one thing a tree adoption does that loses something.
    pub replaced: Vec<String>,
    /// Paths whose local copy differs from the selected version and was left
    /// alone. `--replace`, or `synch adopt path` per path, is what ends the standoff.
    pub differing: Vec<String>,
    /// Paths that changed after this tree adoption was planned: a path the plan
    /// found empty that something now stands at, or a file rewritten since the
    /// plan stat'd it. Refused whatever the flags say — `--replace` answers for
    /// the file it was pointed at, not for whatever the path became while an
    /// object was being fetched — so they are kept apart from `differing`,
    /// which `--replace` does resolve.
    pub appeared: Vec<String>,
    /// Paths a tree adoption could not write, with the reason — including
    /// every path a `strict` tree adoption refused to guess at.
    pub skipped: Vec<(String, String)>,
    /// Paths that *were* written but whose write was not everything it should
    /// have been: the metadata the filesystem refused, the scanner record that
    /// could not be dropped. Kept out of `skipped`, which means "not written",
    /// so that `adopted + skipped` never counts one path twice.
    pub warnings: Vec<(String, String)>,
    /// Bytes that crossed the network for what this tree adoption wrote.
    pub fetched_bytes: u64,
    /// Bytes that did not, because a local donor already held them
    /// (`docs/DELTA-SYNC.md` §3.3).
    pub reused_bytes: u64,
    /// Files whose bytes were not copied at all, because the space and the CAS
    /// share a filesystem. A subset of `adopted`.
    pub reflinked: usize,
    /// Whether this report describes a run that wrote nothing on purpose.
    pub dry_run: bool,
    /// Paths the space's ignore rules exclude, which a tree adoption does not write
    /// because a scan would never publish them. Counted, not named: one rule
    /// like `node_modules/` covers arbitrarily many.
    pub ignored: usize,
    /// How many paths the unified tree carried under the prefix that was
    /// adopted, whatever became of them. Zero means the prefix names nothing —
    /// which a caller cannot infer from the counters, since a path the policy
    /// selects nothing for and a tombstoned one are both passed over in
    /// silence.
    pub considered: usize,
}

impl Node {
    /// Adoptions a space's configured directory with the unified tree's content
    /// (§7.2, `synch adopt tree`).
    ///
    /// `prefix` narrows the tree adoption to a directory within the space, empty for
    /// all of it. The space must be one this node indexes: a tree adoption writes where
    /// the scanner will find it, and outside a space nothing would.
    pub async fn adopt_tree(
        &self,
        space_id: &str,
        prefix: &str,
        policy: &VersionPolicy,
        options: AdoptTreeOptions,
    ) -> Result<AdoptTreeReport> {
        // Serialized against every other materialization pass on this node, so
        // two adopts of one space cannot plan against each other's half-written
        // state. Checkouts take the same lock; their roots cannot overlap a
        // space's, so sharing it costs nothing but the wait.
        let _pass = self.lock_materialization().await;
        // Refused in recovery, before anything is written, for the reason every
        // other command that does irreversible work before publishing takes
        // this gate — and for one more that is specific to a tree adoption.
        //
        // A recovering node holds no complete head of its own (§3.4), which is
        // exactly the state an operator reaches for a tree adoption in: the checkout is
        // gone and the cluster has the content. But a scan refuses in recovery
        // too, so everything adopted would sit unpublished and the closing line
        // of the command — "the next scan publishes what was adopted" — would be
        // false. Worse, `--replace`'s own-origin guard is *inert* here: it fires
        // when the selected version is this node's own, and a recovering node
        // publishes nothing under its own origin, so every path selects a
        // peer's version and every local file that differs is overwritten with
        // no version, no `prev` and no trace. The guard against silent loss is
        // missing precisely where the danger is greatest.
        //
        // `synch recover` first, then tree adoption, then scan. The error names it.
        {
            let node = self.clone();
            crate::blocking::offload(move || node.ensure_publishable()).await?;
        }
        let plan = self
            .plan_adoption(space_id, prefix, policy, options)
            .await?;
        self.write_adoption(plan).await
    }

    /// Everything a tree adoption can settle before the network: which paths need
    /// writing, and every decision that needs no bytes.
    ///
    /// Split from [`Node::write_adoption`] because the two halves are what a tree adoption
    /// *is* — and because the gap between them is where the interesting
    /// failures live, so a test has to be able to stand in it.
    async fn plan_adoption(
        &self,
        space_id: &str,
        prefix: &str,
        policy: &VersionPolicy,
        options: AdoptTreeOptions,
    ) -> Result<AdoptTreePlan> {
        // The space row is read once and its root carried through the pass:
        // the write guard needs the root per path, and re-reading the row for
        // each would be one store acquisition per file in the space.
        let root_dir = {
            let (node, space_id) = (self.clone(), space_id.to_string());
            crate::blocking::offload(move || {
                let space = node.store().source(&space_id)?.ok_or_else(|| {
                    EngineError::not_found(format!(
                        "no filesystem source {space_id}: `synch source add {space_id} <dir>` \
                         names the directory tree adoption writes into"
                    ))
                })?;
                // An API source has no directory to adopt into. A replica
                // checkout is a read-only projection, not a publishing source.
                let local_path = space.local_path.ok_or_else(|| {
                    EngineError::invalid(format!(
                        "source {space_id} is API-only and has no local directory for tree adoption; \
                         `synch replica add {space_id} --checkout <dir>` creates a read-only materialization"
                    ))
                })?;
                // The guard `scan_space_with_ingest` takes, for the same reason and the
                // opposite failure. A vanished root — an unmounted drive, a
                // renamed mount — must not read to the scanner as "every file
                // deleted", and must not read to a tree adoption as "every file
                // missing": nothing would stop it recreating the mount point on
                // the underlying filesystem and materializing the whole tree
                // into it, adopting the system disk with a copy that the next
                // scan refuses to publish and the returning drive hides.
                match std::fs::metadata(&local_path) {
                    Ok(meta) if meta.is_dir() => {}
                    Ok(_) => {
                        return Err(EngineError::invalid(format!(
                            "space {space_id} root {local_path} is not a directory"
                        )))
                    }
                    Err(e) => {
                        return Err(EngineError::invalid(format!(
                            "space {space_id} root {local_path} is unavailable: {e}"
                        )))
                    }
                }
                Ok(PathBuf::from(local_path))
            })
            .await?
        };

        // Blocking end to end for the reasons checkout.rs lays out: the listing
        // is a range scan over every path in the space plus a version set each,
        // and deciding what a path needs is a stat and — where the scanner's
        // own record cannot answer — a whole-file hash.
        let node = self.clone();
        let (space, prefix) = (space_id.to_string(), prefix.to_string());
        let policy = policy.clone();
        crate::blocking::offload(move || {
            // A tree adoption writes into an *indexed* directory, so it is bound by the
            // rules that indexing applies. Writing a path this space ignores
            // would put a file where the scanner will never look: never
            // published, never swept — it is in neither `local_files` nor the
            // published tree — and reported `current` by every tree adoption after it.
            // A checkout needs none of this because nothing indexes what it
            // writes.
            let ignore = IgnoreSet::for_space(&root_dir)?;
            let listing = node.unified_listing(&space, &prefix, None, None)?;
            let (report, wanted, links) = decide(
                &node, &space, &root_dir, &listing, &policy, &ignore, options,
            )?;
            Ok(AdoptTreePlan {
                space,
                root_dir,
                options,
                report,
                wanted,
                links,
            })
        })
        .await
    }

    /// Fetches and writes what the plan decided it must, and reports.
    async fn write_adoption(&self, plan: AdoptTreePlan) -> Result<AdoptTreeReport> {
        let AdoptTreePlan {
            space: space_id,
            root_dir,
            options,
            mut report,
            wanted,
            links,
        } = plan;
        if options.dry_run {
            // The plan *is* the answer: everything it decided is already in the
            // report, and these two are what a real run would go and write.
            report.adopted += wanted.len() + links.len();
            report.replaced.extend(
                links
                    .iter()
                    .filter(|l| l.replacing)
                    .map(|l| l.path.clone())
                    .chain(
                        wanted
                            .iter()
                            .filter(|w| w.replacing)
                            .map(|w| w.path.clone()),
                    ),
            );
            return Ok(report);
        }

        // Links first, and cheaply: they need no fetch, so nothing is gained by
        // interleaving them with objects that do.
        for link in links {
            let PendingLink {
                path,
                target,
                link_target,
                replacing,
                was,
            } = link;
            // The same three guards the object loop takes below, and for the
            // same reasons: `materialize_symlink` unlinks whatever stands at the
            // target, so a link is exactly as capable of destroying a file that
            // arrived since the plan as a rename is.
            let (root, guarded) = (root_dir.clone(), path.clone());
            let outcome = crate::blocking::offload(move || {
                if let Some(reason) = root_is_gone(&root) {
                    return Ok(Err(reason));
                }
                if escapes_via_symlink(&root, &guarded) {
                    return Ok(Err(ESCAPED.to_string()));
                }
                // Identity, not just existence: `--replace` answers for the
                // file the operator was shown, and a path that has become
                // something else since is not it.
                let over = match target.symlink_metadata() {
                    Ok(stat) if replacing && was.as_ref() == Some(&signature(&stat)) => true,
                    Ok(_) => return Ok(Err(APPEARED.to_string())),
                    Err(_) => false,
                };
                Ok(materialize_symlink(&target, Some(&link_target)).map(|_| over))
            })
            .await?;
            match outcome {
                Ok(over) => {
                    report.adopted += 1;
                    if over {
                        report.replaced.push(path);
                    }
                }
                Err(reason) if reason == APPEARED => report.appeared.push(path),
                Err(reason) => report.skipped.push((path, reason)),
            }
        }

        // Phase 2: fetch what phase 1 could not satisfy locally — building each
        // object out of a donor where the descent can — and write it as it
        // lands.
        for want in wanted {
            let Wanted {
                path,
                target,
                content,
                size,
                meta,
                donors,
                replacing,
                was,
            } = want;
            // A failure here is this path's, not the run's. A one-shot tree adoption has
            // no second pass to repair what an early `?` would abandon, and by
            // now some of these files are already on disk — so the run finishes
            // and the report names what went wrong, rather than the operator
            // getting an error and no account of what was written.
            let fetched = match self.fetch_all_from(&content, size, &donors).await {
                Ok(fetched) => fetched,
                Err(e) => {
                    report
                        .skipped
                        .push((path, format!("content could not be fetched: {e}")));
                    continue;
                }
            };
            if !fetched.complete {
                report
                    .skipped
                    .push((path, "no provider could serve the content".into()));
                continue;
            }

            // Both guards are re-taken here because a fetch stands between the
            // plan and this point and each of them describes something that can
            // have moved in the meantime. Not in the same step as the write:
            // `materialize_blob` is its own await below, and a clone-or-copy
            // cannot be held under a stat. The window this closes is the fetch;
            // what is left is the materialization, which the link loop — where
            // stat and write really are one step — does not have.
            //
            // The escape guard is the checkout's, and describes the *directory*
            // the write lands in — `escapes_via_symlink` pops the last
            // component, so it says nothing about the target itself.
            //
            // The second guard is the one a tree adoption needs and a checkout does not.
            // A checkout owns its root; a tree adoption writes into the directory the user
            // works in, with the watcher running, so "nothing was here when we
            // planned" is a statement with a shelf life. Materialization ends
            // in a rename, which destroys whatever it lands on — so a file that
            // appeared during the fetch would be silently overwritten by a run
            // that was never given `--replace`. Re-stat, and refuse.
            let (root, guarded) = (root_dir.clone(), path.clone());
            let stat_target = target.clone();
            let ready = crate::blocking::offload(move || {
                if let Some(reason) = root_is_gone(&root) {
                    return Ok(Ready::RootGone(reason));
                }
                if escapes_via_symlink(&root, &guarded) {
                    return Ok(Ready::Escaped);
                }
                Ok(match std::fs::symlink_metadata(&stat_target) {
                    // Planned as a replacement, and still the very file the
                    // plan looked at: `--replace` was given for exactly this.
                    Ok(stat) if replacing && was.as_ref() == Some(&signature(&stat)) => {
                        Ready::Write { over: true }
                    }
                    // Something is here that the operator was not shown — a
                    // path that was empty and is not, or one whose file has
                    // been rewritten since. `--replace` answers for the file it
                    // was pointed at, not for whatever the path became while
                    // an object was being fetched, which on a large space is
                    // minutes of somebody else's work.
                    Ok(_) => Ready::Appeared,
                    // Nothing here now — including the case where this path was
                    // planned as a replacement and the file has since been
                    // deleted by someone else. Writing is right either way; what
                    // changes is that `replaced` must not claim a file this tree adoption
                    // did not overwrite.
                    Err(_) => Ready::Write { over: false },
                })
            })
            .await?;
            let over = matches!(ready, Ready::Write { over: true });
            // `stale` rides along: dropping the scanner's row is bookkeeping
            // done in the same blocking step as the stamp, and its failure is a
            // report line rather than the end of the run.
            let (outcome, stale) = if let Ready::RootGone(reason) = ready {
                (Written::Failed(reason), None)
            } else if let Ready::Escaped = ready {
                (Written::Escaped, None)
            } else if let Ready::Appeared = ready {
                (Written::Appeared, None)
            } else {
                // A materialization that fails takes its path down with it and
                // nothing else: the target is untouched.
                match self.materialize_blob(&content, size, target.clone()).await {
                    Err(e) => (Written::Failed(e.to_string()), None),
                    Ok(kind) => {
                        // The bytes are the file; the metadata is stamped right
                        // after, and a filesystem that refuses the stamp is
                        // reported rather than allowed to fail the pass. Not
                        // cosmetic here: the stamped mtime is the one the next
                        // scan publishes.
                        let (node, space, relpath) = (self.clone(), space_id.clone(), path.clone());
                        crate::blocking::offload(move || {
                            let written = match apply_metadata(&target, meta) {
                                Ok(()) => Written::Fully(kind),
                                Err(e) => Written::WithoutMetadata(kind, e.to_string()),
                            };
                            // The scanner skips a file whose `(size, mtime_ns,
                            // file_id)` still matches its `local_files` row, and
                            // this path's row describes bytes that are gone.
                            // Dropping it makes the next scan re-hash.
                            //
                            // It matters most where `file_identity` is `None` —
                            // every non-unix platform — because there the
                            // comparison is only `(size, mtime)`, and a tree
                            // adoption stamps the selected version's mtime. A version
                            // tying this node's own published one on both (the
                            // tie `newest` breaks on content root) would leave
                            // the scan calling an adopted path unchanged and never
                            // publishing it: the disk holding the peer's bytes
                            // while this node went on publishing its own. §7.2
                            // rests on that publish happening.
                            //
                            // Per path rather than batched at the end: adopting
                            // a large tree runs for minutes, and a client that
                            // hangs up drops this future at an await. A batch
                            // deferred to the end is a batch a Ctrl-C loses,
                            // which is exactly how the drift above gets in.
                            //
                            // Reported, never propagated: this is bookkeeping
                            // that runs *after* the bytes and the stamp landed,
                            // and a `?` here would throw away the account of
                            // every file the tree adoption had already written — the
                            // failure round two removed from the fetch and the
                            // donor lookup for the same reason.
                            let stale = node
                                .store()
                                .remove_local_file(&space, &relpath)
                                .err()
                                .map(|e| e.to_string());
                            Ok((written, stale))
                        })
                        .await?
                    }
                }
            };
            match outcome {
                Written::Fully(kind) | Written::WithoutMetadata(kind, _) => {
                    report.adopted += 1;
                    report.reflinked += usize::from(kind == crate::CloneKind::Reflink);
                    // Counted here rather than at the fetch, so the pair
                    // describes the bytes behind the files this tree adoption wrote
                    // rather than including work the guards then turned away.
                    report.fetched_bytes += crate::checkout::bytes_of(&fetched.fetched, size);
                    report.reused_bytes += crate::checkout::bytes_of(&fetched.promoted, size);
                    if over {
                        report.replaced.push(path.clone());
                    }
                    if let Some(why) = stale {
                        report.warnings.push((
                            path.clone(),
                            format!(
                                "written, but the scanner's record of the old file could not be \
                                 dropped, so the next scan may not re-index it: {why}"
                            ),
                        ));
                    }
                    if let Written::WithoutMetadata(_, why) = outcome {
                        report.warnings.push((
                            path,
                            format!(
                                "content written, but its metadata could not be reproduced, so \
                                 the next scan will publish it under this node's own clock: {why}"
                            ),
                        ));
                    }
                }
                Written::Escaped => report.skipped.push((path, ESCAPED.into())),
                Written::Appeared => report.appeared.push(path),
                Written::Failed(why) => report
                    .skipped
                    .push((path, format!("content could not be written: {why}"))),
            }
        }

        Ok(report)
    }
}

#[cfg(test)]
impl Node {
    /// The plan half alone, so a test can let the world move between the plan
    /// and the write the way a slow fetch does.
    pub(crate) async fn adoption_plan_for_test(
        &self,
        space_id: &str,
        policy: &VersionPolicy,
    ) -> AdoptTreePlan {
        self.plan_adoption(space_id, "", policy, AdoptTreeOptions::default())
            .await
            .expect("the plan half should succeed")
    }

    /// The same, under options other than the default.
    pub(crate) async fn adoption_plan_with_options_for_test(
        &self,
        space_id: &str,
        policy: &VersionPolicy,
        options: AdoptTreeOptions,
    ) -> AdoptTreePlan {
        self.plan_adoption(space_id, "", policy, options)
            .await
            .expect("the plan half should succeed")
    }

    /// The write half alone, against a plan taken earlier.
    pub(crate) async fn finish_adoption_for_test(
        &self,
        plan: AdoptTreePlan,
    ) -> Result<AdoptTreeReport> {
        self.write_adoption(plan).await
    }
}

/// One path a tree adoption has decided it must fetch and write.
#[derive(Debug)]
struct Wanted {
    /// The path within the space, for the report.
    path: String,
    /// Where it goes on disk.
    target: PathBuf,
    /// The object to materialize.
    content: Hash,
    /// Its size, which bounds the fetch and the copy.
    size: u64,
    /// The metadata to stamp on once the bytes are there.
    meta: Metadata,
    /// Where the bytes might already be, in §3.2 priority order.
    donors: Vec<Donor>,
    /// Whether a local file with different bytes is being overwritten, which
    /// only `--replace` reaches.
    replacing: bool,
    /// What the plan saw at the target: `(len, mtime_ns, file_id)`, or `None`
    /// where there was nothing. The write compares against it, so `--replace`
    /// overwrites the file the operator was shown and not whatever the path
    /// became while the object was being fetched.
    was: Option<(u64, i64, Option<Vec<u8>>)>,
}

/// The stat signature the plan records for a target, and the write re-checks.
fn signature(stat: &std::fs::Metadata) -> (u64, i64, Option<Vec<u8>>) {
    (
        stat.len(),
        crate::scanner::mtime_nanos(stat),
        crate::scanner::file_identity(stat),
    )
}

/// What the guards taken immediately before a write decided.
#[derive(Debug)]
enum Ready {
    /// Write it. `over` is whether something is actually there to be replaced,
    /// which the plan can only guess at and this stat knows.
    Write { over: bool },
    /// The space's own directory has gone since the tree adoption started.
    RootGone(String),
    /// An ancestor is a symlink: the write would land outside the space.
    Escaped,
    /// What is at the path now is not what the plan looked at — nothing was
    /// here and something is, or the file has been rewritten since. Either way
    /// it belongs to whoever wrote it.
    Appeared,
}

/// How one write ended.
#[derive(Debug)]
enum Written {
    /// Bytes and metadata both.
    Fully(crate::CloneKind),
    /// The bytes landed; the filesystem refused the metadata.
    WithoutMetadata(crate::CloneKind, String),
    /// Refused by the symlink-escape guard: nothing was written.
    Escaped,
    /// Refused because the path is no longer the thing the plan looked at.
    Appeared,
    /// The object could not be materialized. The target is as it was.
    Failed(String),
}

/// What one tree adoption settled before any content was fetched.
#[derive(Debug)]
pub(crate) struct AdoptTreePlan {
    /// The space being adopted, for the `local_files` rows the write half drops.
    space: String,
    /// Its configured directory.
    root_dir: PathBuf,
    /// The options both halves branch on. Carried rather than passed again, so
    /// a plan cannot be written under options it was not decided under.
    options: AdoptTreeOptions,
    /// Everything already decided.
    report: AdoptTreeReport,
    /// What is left for the network half to fetch.
    wanted: Vec<Wanted>,
    /// Symbolic links, which need no fetch — but are written by the same half
    /// as everything else, so that deciding stays free of side effects and a
    /// dry run is a dry run all the way down.
    links: Vec<PendingLink>,
}

/// A symbolic link the plan decided to write.
#[derive(Debug)]
struct PendingLink {
    /// The path within the space, for the report.
    path: String,
    /// Where it goes on disk.
    target: PathBuf,
    /// What the link should point at.
    link_target: String,
    /// Whether something is being replaced, which only `--replace` reaches.
    replacing: bool,
    /// What the plan saw at the target, as [`signature`]. The write compares
    /// against it for the same reason the object path does: `materialize_symlink`
    /// unlinks whatever stands there, so a link destroys a file rewritten since
    /// the plan exactly as a rename does.
    was: Option<(u64, i64, Option<Vec<u8>>)>,
}

/// Decides, and performs, everything a tree adoption can settle without the network.
///
/// Blocking from end to end: [`Node::plan_adoption`] runs it on the blocking pool.
#[allow(clippy::too_many_arguments)]
fn decide(
    node: &Node,
    space_id: &str,
    root_dir: &Path,
    listing: &[VersionSet],
    policy: &VersionPolicy,
    ignore: &IgnoreSet,
    options: AdoptTreeOptions,
) -> Result<(AdoptTreeReport, Vec<Wanted>, Vec<PendingLink>)> {
    // One clock reading for the pass, and the store's rather than the bare
    // clock, so every path selects against the same instant (`plan_pass`,
    // checkout.rs).
    let now = node.store().read_instant()?;
    let mut report = AdoptTreeReport {
        dry_run: options.dry_run,
        ..AdoptTreeReport::default()
    };
    let mut wanted: Vec<Wanted> = Vec::new();
    let mut links: Vec<PendingLink> = Vec::new();
    // Paths this pass will make into symbolic links, so that what they shadow
    // is judged against the tree the tree adoption is about to create rather than the
    // one it started with.
    let mut planned_links: Vec<String> = Vec::new();
    // What the listing carried, so the caller can tell "this prefix names
    // nothing" from "everything under it was already here or was passed over".
    report.considered = listing.len();
    // Detected before anything is written, the way a checkout pass does it: the
    // first claimant of a folded name wins and the rest are reported — but only
    // where this space's filesystem actually folds them.
    let mut claimed: HashMap<String, String> = HashMap::new();

    for set in listing {
        // A checkout refuses these names on every platform, so that one projection of
        // one tree is the same directory everywhere. A tree adoption has the opposite
        // obligation: it writes into *this* machine's directory, where this
        // machine's own scanner is the thing that published half these paths.
        // `2026-08-21T10:00:00.log` and `aux.txt` are ordinary names here, and
        // refusing them would report a file as skipped on every tree adoption while it
        // sat on disk already current — and would make a peer's copy of such a
        // path unadoptable on the very platform that can hold it.
        //
        // So the check is the platform's own. Safety does not rest on it in
        // either case: `target_within` below is what refuses a path that would
        // leave the space, and it applies the platform's rules on every one.
        #[cfg(windows)]
        if let Some(reason) = unsafe_name(&set.path) {
            report.skipped.push((set.path.clone(), reason));
            continue;
        }
        // A link this same pass is about to write shadows everything beneath
        // it. `target_within` cannot see that — the link is not on disk yet, so
        // its escape check passes — and the write phase, which takes the guard
        // again once the link *is* there, then refuses the path. Without this
        // the plan promises what the write will not do, which under `--dry-run`
        // is the one thing a plan must never do.
        //
        // The checkout gets this for free by materializing in listing order, so a
        // link written for `sub` is on disk before `sub/passwd` is judged
        // (checkout.rs). A tree adoption decides everything before it writes anything, so
        // it has to carry the knowledge instead of reading it off the disk. The
        // listing is sorted, so a link at `sub` is always seen before `sub/…`.
        if planned_links
            .iter()
            .any(|link| set.path.starts_with(&format!("{link}/")))
        {
            report.skipped.push((set.path.clone(), ESCAPED.to_string()));
            continue;
        }
        // The same guard `synch adopt path` and the S3 gateway write through: a
        // published path only ever lands inside the space it belongs to.
        let target = match target_within(root_dir, space_id, &set.path) {
            Ok(target) => target,
            Err(e) => {
                report.skipped.push((set.path.clone(), e.to_string()));
                continue;
            }
        };

        let selected = match set.select(policy, now) {
            synch_store::Selection::Selected(entry) => *entry,
            // The policy selects nothing here — an `origin=` pin on an origin
            // that publishes no version of this path — so the path is not in
            // this tree adoption's view. Whatever is on disk is ours and stays.
            synch_store::Selection::Absent => continue,
            // Strict tree adoption never guesses, and it has nothing to remove:
            // the local copy is this node's own
            // assertion, not a materialized guess, so the path is reported and
            // left exactly as it is.
            synch_store::Selection::Divergent => {
                report.skipped.push((
                    set.path.clone(),
                    format!(
                        "{} versions and the policy is strict: {}",
                        set.version_count(),
                        set.describe().join("; ")
                    ),
                ));
                continue;
            }
        };

        // A tree adoption adds. A tombstone asks for a removal, which is the one thing
        // it will not do — `synch adopt path` of that version is how a deletion is
        // adopted, deliberately and one path at a time (§8).
        if selected.kind == EntryKind::Tombstone || selected.kind == EntryKind::Dir {
            continue;
        }

        // Whatever this space excludes, a tree adoption does not write: such a file
        // would sit where the scanner never looks — never published, never
        // swept, and reported `current` by every tree adoption after it.
        //
        // Counted rather than named, the way a scan counts them
        // (`ScanReport::ignored`): a peer that published `node_modules/` before
        // this space had a `.syncignore` would otherwise turn every tree adoption into a
        // hundred thousand lines of stdout. And asked here rather than at the
        // top of the loop, so a path the policy passes over in silence — a
        // tombstone, a version an `origin=` pin does not carry — stays silent
        // rather than being reported as excluded.
        if ignore.excludes_path(&set.path) {
            report.ignored += 1;
            continue;
        }

        // What is here now, if anything. `symlink_metadata`, so a symlink is
        // seen as the link it is rather than followed.
        let on_disk = std::fs::symlink_metadata(&target).ok();
        if on_disk.as_ref().is_some_and(|m| m.is_dir()) {
            report.skipped.push((
                set.path.clone(),
                "a directory stands here, and a tree adoption does not remove things".into(),
            ));
            continue;
        }

        // The first claimant to a folded name wins and the rest are reported
        // (`claim_folded_name`): without the claim, `--replace` would write one
        // over the other and call both of them adopted.
        //
        // Applied on every platform, as the checkout applies it. A tree adoption could ask
        // the filesystem instead — and did, for three revisions — but the cost
        // of being wrong runs the other way: refusing a name is a report about
        // two files that are both there, while a wrong "does not fold" is a
        // silent clobber. §7.2 takes the conservative side.
        if let Err(reason) = crate::checkout::claim_folded_name(&mut claimed, &set.path) {
            report.skipped.push((set.path.clone(), reason));
            continue;
        }

        if selected.kind == EntryKind::Symlink {
            let wanted_target = selected.symlink_target.as_deref();
            let current = std::fs::read_link(&target)
                .ok()
                .is_some_and(|link| Some(link.to_string_lossy().as_ref()) == wanted_target);
            if current {
                report.current += 1;
            } else if let Some(reason) = symlink_refusal(wanted_target) {
                // Asked in the plan rather than at the write, so a dry run
                // cannot promise a link this platform will refuse — on every
                // non-unix one, that is all of them. And before the `differing`
                // arm, so a path there is never told "--replace replaces it" by a
                // platform that will answer `--replace` with this same refusal.
                report.skipped.push((set.path.clone(), reason));
            } else if on_disk.is_some() && !options.replace {
                report.differing.push(set.path.clone());
            } else if on_disk.is_some() && selected.origin == *node.origin() {
                // The same refusal the regular-file branch makes, for the same
                // reason: `--replace` adopts a peer's version, and this version is
                // ours. A link retargeted since the last scan is an unpublished
                // edit, and recreating the published target would restore what
                // we already publish — so the next scan would stage nothing and
                // the retarget would vanish without a trace.
                report
                    .skipped
                    .push((set.path.clone(), OWN_VERSION_DIFFERS.into()));
            } else {
                planned_links.push(set.path.clone());
                links.push(PendingLink {
                    path: set.path.clone(),
                    target,
                    link_target: wanted_target.unwrap_or_default().to_string(),
                    replacing: on_disk.is_some(),
                    was: on_disk.as_ref().map(signature),
                });
            }
            continue;
        }

        let Some(content) = selected.content else {
            report
                .skipped
                .push((set.path.clone(), "entry has no content".into()));
            continue;
        };

        if let Some(stat) = &on_disk {
            match content_here(node, space_id, &set.path, &target, stat, &selected) {
                LocalContent::Is(here) if here == content => {
                    // Right bytes. The metadata is left alone deliberately:
                    // what a file here carries is this node's own assertion,
                    // and a tree adoption that restamped it would republish every path
                    // it looked at.
                    report.current += 1;
                    continue;
                }
                // The bytes are readable and are not the selected version's.
                LocalContent::Is(_) | LocalContent::WrongSize => {
                    if !options.replace {
                        report.differing.push(set.path.clone());
                        continue;
                    }
                    // Our *own* published version, and the file here does not
                    // match it: the disk is a local edit this node has not
                    // scanned yet, and the entry is what it published last
                    // time. Adopting would overwrite the newer statement of our
                    // view with the older one, and — because the old content
                    // root is the one we already publish — the next scan would
                    // stage nothing, so the edit would vanish leaving no
                    // version, no `prev`, and no trace anywhere in the cluster.
                    // `--replace` means "take theirs"; there is no theirs here.
                    if selected.origin == *node.origin() {
                        report
                            .skipped
                            .push((set.path.clone(), OWN_VERSION_DIFFERS.into()));
                        continue;
                    }
                }
                // Something is here, it is the right length to be the version,
                // and it could not be read. "Differs" would be a claim nothing
                // established, and `--replace` acts on that claim by renaming
                // over the file — which needs no read permission at all. So a
                // file this node cannot read is never replaced and never
                // called differing.
                LocalContent::Unreadable(why) => {
                    report.skipped.push((
                        set.path.clone(),
                        format!("a file is here and could not be read to compare it: {why}"),
                    ));
                    continue;
                }
            }
        }

        // Per-path, like every other failure in this loop: a donor lookup that
        // fails is a path that cannot be planned, not a run that cannot finish
        // — and by this point the loop has already written symlinks.
        let donors = match node.donors_for(&selected, set) {
            Ok(donors) => donors,
            Err(e) => {
                report.skipped.push((
                    set.path.clone(),
                    format!("could not look up what could serve the content: {e}"),
                ));
                continue;
            }
        };
        wanted.push(Wanted {
            path: set.path.clone(),
            target,
            content,
            size: selected.size,
            meta: Metadata::of(&selected),
            donors,
            replacing: on_disk.is_some(),
            was: on_disk.as_ref().map(signature),
        });
    }

    Ok((report, wanted, links))
}

/// The content root the scanner recorded for a path, when the stat on disk
/// still proves that record describes the file that is there.
///
/// The rule is the scanner's own (`index_file`, scanner.rs): size, mtime and
/// platform identity all match, and the hash was taken comfortably after the
/// mtime it vouches for, so no same-size in-place rewrite can have shared the
/// stamp. Anything less answers `None`, and the caller hashes.
fn known_source_content(
    node: &Node,
    space_id: &str,
    relpath: &str,
    stat: &std::fs::Metadata,
) -> Option<Hash> {
    if !stat.is_file() {
        return None;
    }
    let known = node.store().local_file(space_id, relpath).ok().flatten()?;
    let fresh = known.size == stat.len()
        && known.mtime_ns == crate::scanner::mtime_nanos(stat)
        && known.file_id == crate::scanner::file_identity(stat)
        && known.scanned_at.saturating_sub(known.mtime_ns) >= crate::scanner::RACY_WINDOW_NS;
    fresh.then_some(known.content).flatten()
}

/// Why the space root cannot be written into right now, or `None`.
///
/// Taken again before every write, not just once in the plan. The plan's check
/// is what stops a tree adoption onto a root that was already gone; this is what stops
/// one that is *unplugged halfway through* — a tree adoption of a large tree runs for
/// hours, and materialization does `create_dir_all` on the target's parent,
/// which for a top-level path is the root itself. Without this, minute five of
/// a tree adoption onto an unmounted drive recreates the mount point and materializes
/// the rest of the tree onto the disk underneath it.
///
/// One `stat` of a directory that is by now certainly in the page cache.
fn root_is_gone(root: &Path) -> Option<String> {
    match std::fs::metadata(root) {
        Ok(meta) if meta.is_dir() => None,
        Ok(_) => Some(format!("{} is no longer a directory", root.display())),
        Err(e) => Some(format!("{} became unavailable: {e}", root.display())),
    }
}

/// The refusal reported for a path whose ancestors include a symlink.
const ESCAPED: &str = "path resolves through a symlink; refusing to write outside the space";

/// The marker the write guards use for a path that is no longer the thing the
/// plan looked at. Never shown to anyone: the caller turns it into a
/// [`AdoptTreeReport::appeared`] entry, which the CLI describes in its own words.
const APPEARED: &str = "a file appeared here while the tree adoption ran";

/// Why `--replace` declines a path whose selected version is this node's own.
///
/// There is nothing to adopt: the file on disk is an edit no scan has
/// published, and writing the published version over it would restore content
/// this node already publishes — so the next scan would stage nothing, and the
/// edit would be gone with no version, no `prev`, and no trace in the cluster.
const OWN_VERSION_DIFFERS: &str =
    "the selected version is this node's own, and what is here differs from it: that is an edit \
     no scan has published yet, not a version to adopt. Run `synch source scan` to publish it";

/// What is on disk where the selected version belongs.
#[derive(Debug)]
enum LocalContent {
    /// The bytes here, named by their content root.
    Is(Hash),
    /// Not the right length to be the selected version, so nothing was read:
    /// no hash of it could have matched.
    WrongSize,
    /// Something is here that could not be read — a mode this daemon does not
    /// satisfy, an I/O error, a file another process holds. Deliberately not
    /// folded into "differs": see the caller.
    Unreadable(String),
}

/// Reads what is at `target` well enough to compare it with the version being
/// adopted, without ever reading more than it must.
///
/// The scanner's own `local_files` record answers first wherever its stat still
/// vouches for the file, which is what keeps a second tree adoption of a large space
/// from re-hashing it. Failing that, length settles almost every case for the
/// price of a `stat`, and only a file that *could* be the version is hashed.
///
/// A regular file or nothing: a symlink standing where the tree publishes a
/// file is a different kind of thing whatever it points at, and following it
/// would let a link to an identical file read as current.
fn content_here(
    node: &Node,
    space_id: &str,
    relpath: &str,
    target: &Path,
    stat: &std::fs::Metadata,
    selected: &EntryRow,
) -> LocalContent {
    if !stat.is_file() {
        return LocalContent::WrongSize;
    }
    if let Some(known) = known_source_content(node, space_id, relpath, stat) {
        return LocalContent::Is(known);
    }
    if stat.len() != selected.size {
        return LocalContent::WrongSize;
    }
    match std::fs::File::open(target)
        .and_then(|file| synch_core::hash_reader(std::io::BufReader::new(file)))
    {
        Ok(root) => LocalContent::Is(root),
        Err(e) => LocalContent::Unreadable(e.to_string()),
    }
}

/// Why this platform cannot write the symbolic link an entry describes, or
/// `None` if it can.
///
/// The refusals [`materialize_symlink`] makes before it touches the disk, asked
/// separately so a dry run can report exactly what a real run would do rather
/// than promising a link that could never be created.
fn symlink_refusal(link_target: Option<&str>) -> Option<String> {
    if link_target.is_none() {
        return Some("symlink entry carries no target".into());
    }
    #[cfg(unix)]
    {
        None
    }
    #[cfg(not(unix))]
    {
        Some(format!(
            "symlink to {}: creating symbolic links is not available to the daemon on this \
             platform, so the path is skipped rather than written as a plain file",
            link_target.unwrap_or_default()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{node_with_space, published};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use synch_core::{now_ns, FileEntry, OriginId};

    fn peer() -> OriginId {
        OriginId::named("nas", "x.example").unwrap()
    }

    fn other() -> OriginId {
        OriginId::named("laptop", "x.example").unwrap()
    }

    /// A real wall-clock stamp, not a pre-epoch one every filesystem stores
    /// exactly.
    const STAMP: i64 = 1_700_000_000_123_456_789;

    /// Publishes `origin`'s version of a path, with its bytes in this node's
    /// CAS so the fetch has a local provider.
    fn publish(node: &Node, origin: &OriginId, path: &str, content: &[u8], mtime: i64) {
        publish_with_mode(node, origin, path, content, mtime, None);
    }

    /// The same, in a named space rather than `media`.
    fn publish_in(
        node: &Node,
        origin: &OriginId,
        space: &str,
        path: &str,
        content: &[u8],
        mtime: i64,
    ) {
        let root = node.store().ingest_bytes(content, now_ns()).unwrap();
        let entry = FileEntry::file(content.len() as u64, mtime, root, 1);
        node.store().put_entry(origin, space, path, &entry).unwrap();
    }

    /// Publishes `origin`'s version of a path as a symbolic link.
    fn publish_link(node: &Node, origin: &OriginId, path: &str, link_target: &str) {
        let mut entry = FileEntry::tombstone(STAMP, 1, None);
        entry.kind = EntryKind::Symlink;
        entry.symlink_target = Some(link_target.into());
        node.store()
            .put_entry(origin, "media", path, &entry)
            .unwrap();
    }

    fn publish_with_mode(
        node: &Node,
        origin: &OriginId,
        path: &str,
        content: &[u8],
        mtime: i64,
        mode: Option<u32>,
    ) {
        let root = node.store().ingest_bytes(content, now_ns()).unwrap();
        let mut entry = FileEntry::file(content.len() as u64, mtime, root, 1);
        entry.unix_mode = mode;
        node.store()
            .put_entry(origin, "media", path, &entry)
            .unwrap();
    }

    /// The whole contract in one pass: what is missing arrives, what already
    /// matches is left alone, and what differs is reported rather than
    /// overwritten — until `--replace` says otherwise.
    #[tokio::test]
    async fn tree_adoption_adds_and_reports_rather_than_overwrites() {
        let (_data, space, node) = node_with_space().await;
        publish(&node, &peer(), "missing.txt", b"theirs", STAMP);
        publish(&node, &peer(), "same.txt", b"agreed", STAMP);
        publish(&node, &peer(), "ours.txt", b"theirs", STAMP);
        std::fs::write(space.path().join("same.txt"), b"agreed").unwrap();
        std::fs::write(space.path().join("ours.txt"), b"mine").unwrap();

        let report = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(report.adopted, 1, "{report:?}");
        assert_eq!(report.current, 1, "{report:?}");
        assert_eq!(report.differing, vec!["ours.txt".to_string()], "{report:?}");
        assert!(report.replaced.is_empty(), "{report:?}");
        assert!(report.skipped.is_empty(), "{report:?}");
        assert_eq!(
            std::fs::read(space.path().join("missing.txt")).unwrap(),
            b"theirs"
        );
        assert_eq!(
            std::fs::read(space.path().join("ours.txt")).unwrap(),
            b"mine",
            "a local file that differs is left exactly as it is"
        );

        // The second pass has nothing to do: the file it wrote is current now.
        let report = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(report.adopted, 0, "{report:?}");
        assert_eq!(report.current, 2, "{report:?}");

        // `--replace` is the bulk `synch adopt path`: it names what it overwrote.
        let report = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions {
                    replace: true,
                    dry_run: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(report.adopted, 1, "{report:?}");
        assert_eq!(report.replaced, vec!["ours.txt".to_string()], "{report:?}");
        assert!(report.differing.is_empty(), "{report:?}");
        assert_eq!(
            std::fs::read(space.path().join("ours.txt")).unwrap(),
            b"theirs"
        );
        node.shutdown().await.unwrap();
    }

    /// A tree adoption only ever adds. A tombstoned version, and a local file nobody
    /// publishes, both survive it.
    #[tokio::test]
    async fn tree_adoption_removes_nothing() {
        let (_data, space, node) = node_with_space().await;
        std::fs::write(space.path().join("deleted.txt"), b"still here").unwrap();
        std::fs::write(space.path().join("private.txt"), b"only mine").unwrap();
        node.store()
            .put_entry(
                &peer(),
                "media",
                "deleted.txt",
                &FileEntry::tombstone(STAMP, 1, None),
            )
            .unwrap();

        let report = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(report.adopted, 0, "{report:?}");
        assert!(report.differing.is_empty(), "{report:?}");
        assert!(space.path().join("deleted.txt").exists());
        assert!(space.path().join("private.txt").exists());

        // Not even under --replace: replacing bytes is one thing, removing a
        // path is `synch adopt path` of the tombstone.
        let report = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions {
                    replace: true,
                    dry_run: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(report.adopted, 0, "{report:?}");
        assert!(space.path().join("deleted.txt").exists());
        node.shutdown().await.unwrap();
    }

    /// The point of stamping: a adopted path publishes as the version that was
    /// adopted, not as a newer one that would win every `newest` selection in
    /// the cluster.
    #[tokio::test]
    async fn adopting_then_scanning_republishes_the_adopted_version() {
        let (_data, _space, node) = node_with_space().await;
        publish_with_mode(&node, &peer(), "f.txt", b"theirs", STAMP, Some(0o100640));
        let theirs = node
            .store()
            .entry(&peer(), "media", "f.txt")
            .unwrap()
            .unwrap();

        let report = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(report.adopted, 1, "{report:?}");
        node.scan_publish_push().await.unwrap();

        let ours = published(&node, "media", "f.txt");
        assert_eq!(ours.content, theirs.content, "the same object");
        // Not equality: filesystems coarsen. NTFS keeps 100 ns, HFS+ whole
        // seconds, and a stamp is only ever coarsened *downward* — so what the
        // scan republishes can sit just below the version that was adopted. What
        // must never happen is the other direction, which is the half that
        // matters: `newest` orders on the mtime, so a adopted path publishing a
        // *newer* one would flip the selection to this node, cluster-wide.
        let drift = theirs.mtime_ns - ours.mtime_ns;
        assert!(
            (0..crate::checkout::MTIME_GRANULARITY_NS).contains(&drift),
            "republished {} against the origin's {}: a adopted path must never publish an mtime \
             newer than the version it came from, nor more than one filesystem tick below it",
            ours.mtime_ns,
            theirs.mtime_ns
        );
        #[cfg(unix)]
        assert_eq!(ours.unix_mode.map(|m| m & 0o777), Some(0o640));
        // And the path is no longer divergent — one version, two attestors.
        let versions = node.versions("media", "f.txt").unwrap();
        assert_eq!(versions.version_count(), 1, "{versions:?}");
        node.shutdown().await.unwrap();
    }

    /// `strict` reports a divergent path and touches nothing. Unlike a strict
    /// checkout it removes no stale copy: what is here is this node's own
    /// assertion, not a materialized guess.
    #[tokio::test]
    async fn strict_tree_adoption_reports_divergence_and_leaves_the_local_copy() {
        let (_data, space, node) = node_with_space().await;
        publish(&node, &peer(), "split.txt", b"theirs", 100);
        publish(&node, &other(), "split.txt", b"others", 200);
        publish(&node, &peer(), "agreed.txt", b"agreed", STAMP);
        std::fs::write(space.path().join("split.txt"), b"mine").unwrap();

        let report = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Strict,
                AdoptTreeOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(report.adopted, 1, "the undisputed path is written");
        assert_eq!(report.skipped.len(), 1, "{report:?}");
        assert_eq!(report.skipped[0].0, "split.txt");
        assert!(report.skipped[0].1.contains("strict"), "{report:?}");
        assert_eq!(
            std::fs::read(space.path().join("split.txt")).unwrap(),
            b"mine"
        );
        node.shutdown().await.unwrap();
    }

    /// An `origin=` pin adopts that origin's versions, and says nothing about
    /// paths it does not publish.
    #[tokio::test]
    async fn origin_selected_tree_adoption_uses_that_origins_versions() {
        let (_data, space, node) = node_with_space().await;
        publish(&node, &peer(), "f.txt", b"theirs", 100);
        publish(&node, &other(), "f.txt", b"others", 200);
        publish(&node, &other(), "only-theirs.txt", b"x", 300);

        let report = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Origin(peer()),
                AdoptTreeOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(report.adopted, 1, "{report:?}");
        assert_eq!(
            std::fs::read(space.path().join("f.txt")).unwrap(),
            b"theirs"
        );
        assert!(
            !space.path().join("only-theirs.txt").exists(),
            "a path the pinned origin does not publish is not in this tree adoption's view"
        );
        node.shutdown().await.unwrap();
    }

    /// A dry run decides everything and writes nothing — including the list of
    /// what `--replace` would overwrite, which is what it is for.
    #[tokio::test]
    async fn a_dry_run_writes_nothing() {
        let (_data, space, node) = node_with_space().await;
        publish(&node, &peer(), "missing.txt", b"theirs", STAMP);
        publish(&node, &peer(), "ours.txt", b"theirs", STAMP);
        std::fs::write(space.path().join("ours.txt"), b"mine").unwrap();

        let report = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions {
                    replace: true,
                    dry_run: true,
                },
            )
            .await
            .unwrap();
        assert!(report.dry_run);
        assert_eq!(report.adopted, 2, "{report:?}");
        assert_eq!(report.replaced, vec!["ours.txt".to_string()], "{report:?}");
        assert!(!space.path().join("missing.txt").exists());
        assert_eq!(
            std::fs::read(space.path().join("ours.txt")).unwrap(),
            b"mine"
        );
        node.shutdown().await.unwrap();
    }

    /// A prefix narrows the tree adoption to one directory of the space.
    #[tokio::test]
    async fn tree_adoption_of_a_prefix_writes_one_directory() {
        let (_data, space, node) = node_with_space().await;
        publish(&node, &peer(), "talks/a.txt", b"a", STAMP);
        publish(&node, &peer(), "notes/b.txt", b"b", STAMP);

        let report = node
            .adopt_tree(
                "media",
                "talks/",
                &VersionPolicy::Newest,
                AdoptTreeOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(report.adopted, 1, "{report:?}");
        assert!(space.path().join("talks/a.txt").exists());
        assert!(!space.path().join("notes/b.txt").exists());
        node.shutdown().await.unwrap();
    }

    /// A tree adoption writes where the scanner will find it, so a space this node does
    /// not index is refused rather than materialized somewhere nothing
    /// publishes.
    #[tokio::test]
    async fn a_space_this_node_does_not_index_is_refused() {
        let (_data, _space, node) = node_with_space().await;
        publish(&node, &peer(), "f.txt", b"theirs", STAMP);
        let refused = node
            .adopt_tree(
                "elsewhere",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions::default(),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("no filesystem source elsewhere"),
            "{refused}"
        );
        node.shutdown().await.unwrap();
    }

    /// A symlink is a version like any other: written where nothing stands,
    /// reported where something else does.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_is_written_where_nothing_stands_and_reported_where_something_does() {
        let (_data, space, node) = node_with_space().await;
        publish_link(&node, &peer(), "link", "target.txt");
        publish_link(&node, &peer(), "taken", "elsewhere.txt");
        std::fs::write(space.path().join("taken"), b"a real file").unwrap();

        let report = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_link(space.path().join("link"))
                .unwrap()
                .to_string_lossy(),
            "target.txt"
        );
        assert_eq!(
            report.differing,
            vec!["taken".to_string()],
            "a file standing where a link is published is reported, not replaced: {report:?}"
        );
        assert_eq!(
            std::fs::read(space.path().join("taken")).unwrap(),
            b"a real file"
        );
        assert!(report.skipped.is_empty(), "{report:?}");
        node.shutdown().await.unwrap();
    }

    /// The guard that keeps a tree adoption inside its space, at both the moments it is
    /// taken: when the path is planned, and again immediately before the write
    /// the plan led to.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlinked_ancestor_is_refused() {
        let (_data, space, node) = node_with_space().await;
        let outside = tempfile::tempdir().unwrap();
        // The shape a hostile peer plants: a link the tree adoption would resolve
        // through, and a path beneath it.
        std::os::unix::fs::symlink(outside.path(), space.path().join("sub")).unwrap();
        publish(&node, &peer(), "sub/escaped.txt", b"not yours", STAMP);

        let report = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(report.adopted, 0, "{report:?}");
        assert_eq!(report.skipped.len(), 1, "{report:?}");
        assert!(
            report.skipped[0].1.contains("symlink"),
            "{:?}",
            report.skipped
        );
        assert!(
            !outside.path().join("escaped.txt").exists(),
            "a tree adoption must never write outside the space root"
        );
        node.shutdown().await.unwrap();
    }

    /// A file that appears while the tree adoption is fetching belongs to whoever wrote
    /// it. Without `--replace` the plan's "nothing was here" has a shelf life,
    /// and materialization ends in a rename that would destroy it silently.
    #[tokio::test]
    async fn a_file_that_appears_mid_adoption_is_not_overwritten() {
        let (_data, space, node) = node_with_space().await;
        publish(&node, &peer(), "late.txt", b"theirs", STAMP);

        // Planned as absent, then created behind the plan's back: the same
        // interleaving a slow fetch gives any editor working in the space.
        let plan = node
            .adoption_plan_for_test("media", &VersionPolicy::Newest)
            .await;
        std::fs::write(space.path().join("late.txt"), b"mine, just now").unwrap();
        let report = node.finish_adoption_for_test(plan).await.unwrap();

        assert_eq!(
            std::fs::read(space.path().join("late.txt")).unwrap(),
            b"mine, just now",
            "the file written during the tree adoption must survive it"
        );
        assert_eq!(report.adopted, 0, "{report:?}");
        assert_eq!(
            report.appeared,
            vec!["late.txt".to_string()],
            "reported as appeared, not differing: this is no longer the file the tree adoption was \
             shown, so `--replace` neither caused it nor resolves it"
        );
        assert!(report.differing.is_empty(), "{report:?}");
        node.shutdown().await.unwrap();
    }

    /// A current file is left completely alone — its mtime included. Restamping
    /// it would republish every path a tree adoption looked at, and `newest` orders on
    /// the mtime, so it would win the selection cluster-wide.
    #[tokio::test]
    async fn a_current_file_is_not_restamped_and_publishes_nothing() {
        let (_data, space, node) = node_with_space().await;
        let target = space.path().join("f.txt");
        std::fs::write(&target, b"agreed").unwrap();
        node.scan_publish_push().await.unwrap();
        let before = std::fs::metadata(&target).unwrap().modified().unwrap();
        // The peer publishes the same bytes under a different stamp.
        publish(&node, &peer(), "f.txt", b"agreed", STAMP);

        let report = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(report.current, 1, "{report:?}");
        assert_eq!(report.adopted, 0, "{report:?}");
        assert_eq!(
            std::fs::metadata(&target).unwrap().modified().unwrap(),
            before,
            "a tree adoption must not restamp a file whose bytes are already right"
        );
        // And so the scan after it has nothing to say.
        assert!(
            node.scan_publish_push().await.unwrap().is_none(),
            "restamping would have republished a path nothing changed at"
        );
        node.shutdown().await.unwrap();
    }

    /// A file the daemon cannot read is never called differing and never
    /// replaced: "differs" would be a claim the failed read did not establish,
    /// and a rename needs no read permission to destroy it.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_unreadable_file_is_reported_rather_than_replaced() {
        let (_data, space, node) = node_with_space().await;
        let target = space.path().join("secret.txt");
        // The same length as the version being adopted, so length cannot settle
        // it and the compare has to read.
        std::fs::write(&target, b"mine!!").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o000)).unwrap();
        publish(&node, &peer(), "secret.txt", b"theirs", STAMP);

        // A mode is not a wall for every uid: run as root — in a container, in
        // a sandbox — and `0o000` reads fine, so there is no unreadable file to
        // test with. Say so and stop rather than assert something the platform
        // is not doing.
        if std::fs::File::open(&target).is_ok() {
            eprintln!(
                "skipped: this uid reads through a 0o000 mode, so nothing here is unreadable"
            );
            node.shutdown().await.unwrap();
            return;
        }

        for replace in [false, true] {
            let report = node
                .adopt_tree(
                    "media",
                    "",
                    &VersionPolicy::Newest,
                    AdoptTreeOptions {
                        replace,
                        dry_run: false,
                    },
                )
                .await
                .unwrap();
            assert!(report.differing.is_empty(), "replace={replace}: {report:?}");
            assert!(report.replaced.is_empty(), "replace={replace}: {report:?}");
            assert_eq!(report.skipped.len(), 1, "replace={replace}: {report:?}");
            assert!(
                report.skipped[0].1.contains("could not be read"),
                "replace={replace}: {report:?}"
            );
        }
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"mine!!");
        node.shutdown().await.unwrap();
    }

    /// `--replace` adopts a peer's version. Where the selected version is this
    /// node's *own*, there is nothing to adopt: the file on disk is an edit no
    /// scan has published yet, and overwriting it would leave no version, no
    /// `prev`, and no trace anywhere.
    #[tokio::test]
    async fn replace_will_not_revert_an_unscanned_local_edit() {
        let (_data, space, node) = node_with_space().await;
        let target = space.path().join("mine.txt");
        std::fs::write(&target, b"published").unwrap();
        node.scan_publish_push().await.unwrap();
        std::fs::write(&target, b"edited since the scan").unwrap();

        let report = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions {
                    replace: true,
                    dry_run: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"edited since the scan",
            "the edit must survive: nothing in the cluster would record its loss"
        );
        assert!(report.replaced.is_empty(), "{report:?}");
        assert_eq!(report.skipped.len(), 1, "{report:?}");
        assert!(
            report.skipped[0].1.contains("synch source scan"),
            "{report:?}"
        );
        node.shutdown().await.unwrap();
    }

    /// Content nobody can serve is one skipped path, not a failed run — and the
    /// report of what did land survives it.
    #[tokio::test]
    async fn an_unservable_object_costs_its_own_path_only() {
        let (_data, space, node) = node_with_space().await;
        publish(&node, &peer(), "here.txt", b"fetchable", STAMP);
        // An entry naming content this node does not hold, with no peer to ask.
        let mut absent = FileEntry::file(9, STAMP, synch_core::Hash([9u8; 32]), 1);
        absent.unix_mode = None;
        node.store()
            .put_entry(&peer(), "media", "absent.txt", &absent)
            .unwrap();

        let report = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            report.adopted, 1,
            "the servable path still landed: {report:?}"
        );
        assert!(space.path().join("here.txt").exists());
        assert_eq!(report.skipped.len(), 1, "{report:?}");
        assert_eq!(report.skipped[0].0, "absent.txt");
        assert!(!space.path().join("absent.txt").exists());
        node.shutdown().await.unwrap();
    }

    /// The fold-collision rule, named both ways: which path wins, and that the
    /// winner's bytes are what is on disk.
    #[tokio::test]
    async fn the_first_claimant_of_a_folded_name_wins() {
        let (_data, space, node) = node_with_space().await;
        publish(&node, &peer(), "Fold.txt", b"upper", STAMP);
        publish(&node, &peer(), "fold.txt", b"lower", STAMP);

        let report = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(report.adopted, 1, "{report:?}");
        assert_eq!(report.skipped.len(), 1, "{report:?}");
        // The listing is ordered, so the first claimant is the lexicographically
        // first path — the same winner a checkout picks (§7.2).
        assert_eq!(report.skipped[0].0, "fold.txt", "{report:?}");
        assert!(report.skipped[0].1.contains("collides with Fold.txt"));
        assert_eq!(
            std::fs::read(space.path().join("Fold.txt")).unwrap(),
            b"upper"
        );
        node.shutdown().await.unwrap();
    }

    /// A API source has no checkout to tree adoption, and says which command does
    /// materialize one.
    #[tokio::test]
    async fn an_api_source_is_refused_with_the_command_that_fits() {
        let (_data, _space, node) = node_with_space().await;
        node.store()
            .put_source("cloud", synch_store::SourceKind::Api, None)
            .unwrap();
        publish_in(&node, &peer(), "cloud", "f.txt", b"theirs", STAMP);
        let refused = node
            .adopt_tree(
                "cloud",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions::default(),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(refused.contains("API-only"), "{refused}");
        assert!(
            refused.contains("synch replica add cloud --checkout"),
            "{refused}"
        );
        node.shutdown().await.unwrap();
    }

    /// A space root that has gone — an unmounted drive, a renamed mount — is
    /// refused rather than recreated. `scan_space_with_ingest` takes the same guard against
    /// the opposite misreading, that every file was deleted.
    #[tokio::test]
    async fn a_vanished_space_root_is_refused_rather_than_recreated() {
        let (_data, space, node) = node_with_space().await;
        publish(&node, &peer(), "f.txt", b"theirs", STAMP);
        let root = space.path().to_path_buf();
        std::fs::remove_dir_all(&root).unwrap();

        let refused = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions::default(),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(refused.contains("unavailable"), "{refused}");
        assert!(
            !root.exists(),
            "a tree adoption must not recreate a mount point and write the tree into it"
        );
        node.shutdown().await.unwrap();
    }

    /// A tree adoption is bound by the rules that bind indexing: writing a path this
    /// space ignores would put a file where the scanner never looks — never
    /// published, never swept, and reported current by every tree adoption after it.
    #[tokio::test]
    async fn tree_adoption_does_not_write_what_the_space_ignores() {
        let (_data, space, node) = node_with_space().await;
        std::fs::write(
            space.path().join(".syncignore"),
            "raw/
",
        )
        .unwrap();
        publish(&node, &peer(), "raw/photo.raw", b"theirs", STAMP);
        publish(&node, &peer(), "keep.txt", b"theirs", STAMP);

        let report = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(report.adopted, 1, "{report:?}");
        assert!(space.path().join("keep.txt").exists());
        assert!(
            !space.path().join("raw/photo.raw").exists(),
            "an ignored path would never be published, so a tree adoption does not write it"
        );
        assert_eq!(
            report.ignored, 1,
            "counted, not named: one rule can cover a hundred thousand paths"
        );
        assert!(report.skipped.is_empty(), "{report:?}");
        node.shutdown().await.unwrap();
    }

    /// The own-origin refusal covers links as well as files: a link retargeted
    /// since the last scan is an unpublished edit, not a version to adopt.
    #[cfg(unix)]
    #[tokio::test]
    async fn replace_will_not_revert_an_unscanned_symlink_retarget() {
        let (_data, space, node) = node_with_space().await;
        let link = space.path().join("latest");
        std::fs::write(space.path().join("v1"), b"one").unwrap();
        std::fs::write(space.path().join("v2"), b"two").unwrap();
        std::os::unix::fs::symlink("v1", &link).unwrap();
        node.scan_publish_push().await.unwrap();
        // Retargeted, and no scan since.
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink("v2", &link).unwrap();

        let report = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions {
                    replace: true,
                    dry_run: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_link(&link).unwrap().to_string_lossy(),
            "v2",
            "the retarget must survive: nothing would record its loss"
        );
        assert!(report.replaced.is_empty(), "{report:?}");
        assert!(
            report
                .skipped
                .iter()
                .any(|(p, why)| p == "latest" && why.contains("synch source scan")),
            "{report:?}"
        );
        node.shutdown().await.unwrap();
    }

    /// `--replace` resolves `differing`, which is about files somebody looked at.
    /// A file that arrived mid-tree adoption was looked at by nobody, so it is refused
    /// under `--replace` too, and reported apart from the paths `--replace` fixes.
    #[tokio::test]
    async fn a_file_that_appears_mid_adoption_survives_replace_too() {
        let (_data, space, node) = node_with_space().await;
        publish(&node, &peer(), "late.txt", b"theirs", STAMP);

        let plan = node
            .adoption_plan_with_options_for_test(
                "media",
                &VersionPolicy::Newest,
                AdoptTreeOptions {
                    replace: true,
                    dry_run: false,
                },
            )
            .await;
        std::fs::write(space.path().join("late.txt"), b"mine, just now").unwrap();
        let report = node.finish_adoption_for_test(plan).await.unwrap();

        assert_eq!(
            std::fs::read(space.path().join("late.txt")).unwrap(),
            b"mine, just now"
        );
        assert_eq!(report.appeared, vec!["late.txt".to_string()], "{report:?}");
        assert!(report.replaced.is_empty(), "{report:?}");
        node.shutdown().await.unwrap();
    }

    /// A link is as capable of destroying a file that arrived since the plan as
    /// a rename is: `materialize_symlink` unlinks whatever stands at the target.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_file_that_appears_where_a_link_goes_is_not_unlinked() {
        let (_data, space, node) = node_with_space().await;
        publish_link(&node, &peer(), "latest", "v1");

        let plan = node
            .adoption_plan_for_test("media", &VersionPolicy::Newest)
            .await;
        std::fs::write(space.path().join("latest"), b"mine, just now").unwrap();
        let report = node.finish_adoption_for_test(plan).await.unwrap();

        assert_eq!(
            std::fs::read(space.path().join("latest")).unwrap(),
            b"mine, just now",
            "a link must not unlink a file that appeared while the tree adoption ran"
        );
        assert_eq!(report.appeared, vec!["latest".to_string()], "{report:?}");
        assert_eq!(report.adopted, 0, "{report:?}");
        assert!(report.replaced.is_empty(), "{report:?}");
        node.shutdown().await.unwrap();
    }

    /// What a dry run counts is what a real run does. The case that separates
    /// them is a path under a link the same pass is about to write: the link is
    /// not on disk when the path is judged, and it is by the time it is written.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_dry_run_counts_what_the_real_run_writes() {
        let (_data, space, node) = node_with_space().await;
        let outside = tempfile::tempdir().unwrap();
        publish_link(&node, &peer(), "sub", &outside.path().to_string_lossy());
        publish(&node, &peer(), "sub/escaped.txt", b"not yours", STAMP);

        let dry = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions {
                    replace: false,
                    dry_run: true,
                },
            )
            .await
            .unwrap();
        let real = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions::default(),
            )
            .await
            .unwrap();

        assert_eq!(
            dry.adopted, real.adopted,
            "a dry run promised {} and the real run wrote {}: dry {dry:?} real {real:?}",
            dry.adopted, real.adopted
        );
        assert_eq!(dry.skipped.len(), real.skipped.len(), "{dry:?} {real:?}");
        assert!(
            !outside.path().join("escaped.txt").exists(),
            "and the path under the link is still refused"
        );
        assert!(space.path().join("sub").is_symlink());
        node.shutdown().await.unwrap();
    }

    /// The root guard is taken per write, not once: a tree adoption of a large tree runs
    /// for hours, and an unplugged drive mid-run must not have its mount point
    /// recreated and the rest of the tree written onto the disk underneath.
    #[tokio::test]
    async fn a_root_that_vanishes_mid_adoption_stops_the_writes() {
        let (_data, space, node) = node_with_space().await;
        publish(&node, &peer(), "a.txt", b"one", STAMP);
        publish(&node, &peer(), "b.txt", b"two", STAMP);

        let plan = node
            .adoption_plan_for_test("media", &VersionPolicy::Newest)
            .await;
        let root = space.path().to_path_buf();
        std::fs::remove_dir_all(&root).unwrap();
        let report = node.finish_adoption_for_test(plan).await.unwrap();

        assert_eq!(report.adopted, 0, "{report:?}");
        assert_eq!(report.skipped.len(), 2, "{report:?}");
        assert!(
            report
                .skipped
                .iter()
                .all(|(_, why)| why.contains("unavailable")),
            "{report:?}"
        );
        assert!(
            !root.exists(),
            "the root must not be recreated mid-tree adoption"
        );
        node.shutdown().await.unwrap();
    }

    /// Every writer into a filesystem-source directory is bound by its ignore rules, not
    /// just the multipart one: a plain `PutObject` takes `open_adoption`, and
    /// `synch adopt path` takes `adopt`/`adopt_from`. A write the scanner will never
    /// look at is worse than a refused write — the bytes land in the operator's
    /// own directory, unpublished and unswept, and the client that sent them
    /// gets an error anyway when the publish finds nothing.
    #[tokio::test]
    async fn every_space_writer_refuses_an_ignored_path() {
        let (_data, space, node) = node_with_space().await;
        std::fs::write(space.path().join(".syncignore"), "raw/\n").unwrap();

        for refused in [
            node.open_adoption("media", "raw/photo.raw").err(),
            node.adopt("media", "raw/photo.raw", b"bytes").err(),
            node.refuse_if_ignored("media", "raw/nested/deep.raw").err(),
        ] {
            let refused = refused.expect("an ignored path must be refused");
            assert!(refused.to_string().contains("ignore rule"), "{refused}");
        }
        assert!(!space.path().join("raw/photo.raw").exists());
        // And a path the rules do not cover is unaffected.
        assert!(node.open_adoption("media", "keep.txt").is_ok());
        node.shutdown().await.unwrap();
    }

    /// `--replace` answers for the file the operator was shown. A file *edited*
    /// during the fetch window is as much somebody else's work as one created
    /// there, and gets the same refusal.
    #[tokio::test]
    async fn replace_does_not_overwrite_a_file_edited_during_the_fetch() {
        let (_data, space, node) = node_with_space().await;
        let target = space.path().join("f.txt");
        std::fs::write(&target, b"what the plan saw").unwrap();
        publish(&node, &peer(), "f.txt", b"theirs", STAMP);

        let plan = node
            .adoption_plan_with_options_for_test(
                "media",
                &VersionPolicy::Newest,
                AdoptTreeOptions {
                    replace: true,
                    dry_run: false,
                },
            )
            .await;
        // Rewritten behind the plan's back, the way an editor saving into a
        // space does while a large object is still being fetched.
        std::fs::write(&target, b"edited since the plan looked").unwrap();
        let report = node.finish_adoption_for_test(plan).await.unwrap();

        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"edited since the plan looked",
            "--replace answers for the file it was pointed at, not for what the path became"
        );
        assert_eq!(report.appeared, vec!["f.txt".to_string()], "{report:?}");
        assert!(report.replaced.is_empty(), "{report:?}");
        node.shutdown().await.unwrap();
    }

    /// The identity check covers links as well as objects: `--replace` answers
    /// for the file the operator was shown, and a link that unlinks whatever
    /// stands there loses it as completely as a rename does.
    #[cfg(unix)]
    #[tokio::test]
    async fn replace_does_not_unlink_a_file_edited_during_the_plan() {
        let (_data, space, node) = node_with_space().await;
        let target = space.path().join("latest");
        std::fs::write(&target, b"what the plan saw").unwrap();
        publish_link(&node, &peer(), "latest", "v1");

        let plan = node
            .adoption_plan_with_options_for_test(
                "media",
                &VersionPolicy::Newest,
                AdoptTreeOptions {
                    replace: true,
                    dry_run: false,
                },
            )
            .await;
        std::fs::write(&target, b"edited since the plan looked").unwrap();
        let report = node.finish_adoption_for_test(plan).await.unwrap();

        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"edited since the plan looked",
            "a link must not unlink a file the operator was never shown"
        );
        assert_eq!(report.appeared, vec!["latest".to_string()], "{report:?}");
        assert!(report.replaced.is_empty(), "{report:?}");
        node.shutdown().await.unwrap();
    }

    /// A dry run creates nothing at all, not even beside the paths it decided
    /// about.
    #[tokio::test]
    async fn a_dry_run_leaves_no_trace() {
        let (_data, space, node) = node_with_space().await;
        publish(&node, &peer(), "a.txt", b"one", STAMP);
        publish(&node, &peer(), "sub/b.txt", b"two", STAMP);

        let report = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions {
                    replace: false,
                    dry_run: true,
                },
            )
            .await
            .unwrap();
        assert_eq!(report.adopted, 2, "{report:?}");
        assert_eq!(
            std::fs::read_dir(space.path()).unwrap().count(),
            0,
            "a dry run must leave the directory exactly as it found it"
        );
        node.shutdown().await.unwrap();
    }

    /// The last adoption entry point, and the one every gate had missed: a
    /// API-source commit promotes an object to the durable tier and stages a
    /// reference, neither of which a node in recovery can publish.
    #[tokio::test]
    async fn a_recovering_node_commits_no_api_source_file() {
        let (_data, _space, node) = node_with_space().await;
        node.store()
            .put_source("cloud", synch_store::SourceKind::Api, None)
            .unwrap();
        let source = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(source.path(), b"payload").unwrap();
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

        let refused = node
            .commit_api_file("cloud", "f.txt", source.path(), now_ns())
            .await
            .unwrap_err()
            .to_string();
        assert!(refused.contains("recover"), "{refused}");
        node.shutdown().await.unwrap();
    }

    /// A recovering node cannot publish, so a tree adoption would write a tree nothing
    /// would ever announce — and `--replace`'s own-origin guard, which needs this
    /// node to publish something, would be inert while it did.
    #[tokio::test]
    async fn a_recovering_node_refuses_tree_adoption() {
        let (_data, space, node) = node_with_space().await;
        publish(&node, &peer(), "f.txt", b"theirs", STAMP);
        // The observation that puts a node into key-loss recovery (§3.4): a
        // peer advertising a head for our own origin that we have no history
        // for.
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

        let refused = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Newest,
                AdoptTreeOptions::default(),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(refused.contains("recover"), "{refused}");
        assert!(
            !space.path().join("f.txt").exists(),
            "nothing is written before the gate"
        );
        node.shutdown().await.unwrap();
    }

    /// `synch adopt path` adopts by writing into the space and publishing after. On a
    /// node that cannot publish, doing the write first destroys the local copy
    /// and can tell nobody — so the gate is taken before anything is touched,
    /// for content and for a deletion alike.
    #[tokio::test]
    async fn a_recovering_node_adopts_nothing() {
        let (_data, space, node) = node_with_space().await;
        let target = space.path().join("f.txt");
        std::fs::write(&target, b"mine, unscanned").unwrap();
        publish(&node, &peer(), "f.txt", b"theirs", STAMP);
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

        let refused = node
            .adopt_from(&peer(), "media", "f.txt")
            .await
            .unwrap_err()
            .to_string();
        assert!(refused.contains("recover"), "{refused}");
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"mine, unscanned",
            "the local copy must survive an adoption that could never be published"
        );

        let refused = node
            .adopt_deletion("media", "f.txt")
            .unwrap_err()
            .to_string();
        assert!(refused.contains("recover"), "{refused}");
        assert!(target.exists(), "and a deletion must not unlink it either");
        node.shutdown().await.unwrap();
    }

    /// The scanner's own record answers the currency check without a read.
    ///
    /// Proven by making the record and the bytes *disagree*, which is the case
    /// the record's own rule is about: the file is scanned, then rewritten in
    /// place at the same length with its mtime restored, so `(size, mtime,
    /// file_id)` still match and the record still vouches for content the disk
    /// no longer holds. A tree adoption that consults the record calls the path current;
    /// one that hashes the file finds different bytes and wants to write.
    ///
    /// A `chmod 000` was the earlier proof and was no proof at all: every
    /// container running as root reads straight through it, so the test passed
    /// whether or not the record was ever consulted.
    #[tokio::test]
    async fn a_source_tracked_file_is_believed_without_a_read() {
        let (_data, space, node) = node_with_space().await;
        let target = space.path().join("f.txt");
        std::fs::write(&target, b"agreed").unwrap();
        // Backdated past the racy window, so the record the scan leaves is one
        // a later stat is allowed to be trusted against.
        let stamp = std::time::SystemTime::now() - std::time::Duration::from_secs(10);
        let backdate = || {
            std::fs::File::options()
                .write(true)
                .open(&target)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(stamp))
                .unwrap();
        };
        backdate();
        node.scan_publish_push().await.unwrap();

        // Rewritten in place, same length, mtime put back: the stat is
        // unchanged and the record now vouches for bytes that are gone.
        std::fs::write(&target, b"REVISE").unwrap();
        backdate();
        // The peer publishes what the record still claims is here.
        publish(&node, &peer(), "f.txt", b"agreed", STAMP);

        let report = node
            .adopt_tree(
                "media",
                "",
                &VersionPolicy::Origin(peer()),
                AdoptTreeOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            report.current, 1,
            "the record answered. A tree adoption that hashed the file would have found \
             `REVISE` against a published `agreed` and wanted to write: {report:?}"
        );
        assert!(report.differing.is_empty(), "{report:?}");
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"REVISE",
            "and nothing was written over it"
        );
        node.shutdown().await.unwrap();
    }
}
