//! Multipart uploads, from the daemon's side (§9.4).
//!
//! The S3 gateway holds no state and any number of gateway processes may point
//! at one daemon, so a multipart upload — which outlives every request in it —
//! lives here. The store keeps the bookkeeping ([`synch_store::uploads`]) and
//! this module owns the bytes: a directory per upload under the data dir, one
//! staged payload per part, and an assembly step that hands the finished object
//! to the ordinary ingest pipeline.
//!
//! **Assembly always goes through an [`Adoption`].** The tempting shortcut for
//! the single-part case — rename the part straight onto the target — is wrong
//! twice over. `rename(2)` fails with `EXDEV` whenever the data dir and the
//! space are on different filesystems, which is the normal shape for a NAS or
//! an external drive; and a renamed file keeps the mtime it was *uploaded*
//! with, which is what §8's `newest` policy orders versions by, so a completion
//! would publish a version that loses to content it supersedes. One path, taken
//! by every completion, avoids both.

use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;

use synch_core::Hash;
use synch_store::{UploadPart, UploadState, MAX_PART_NUMBER, MAX_PART_SIZE, MIN_PART_SIZE};

use crate::{
    error::{EngineError, Result},
    node::Node,
    scanner::Adoption,
};

/// How long an upload nobody finished stays before the sweeper collects it.
///
/// S3 leaks incomplete multipart uploads by design — that is what its lifecycle
/// rules are for — so something here has to decide. A week is long enough that
/// a genuinely slow client is never cut off and short enough that a wedged one
/// does not hold its bytes forever.
pub const DEFAULT_UPLOAD_TTL: std::time::Duration = std::time::Duration::from_secs(7 * 86_400);

/// How long a completion may hold the latch before another may steal it.
///
/// A completion whose caller went away — a client socket timing out mid-
/// assembly is routine, and the assembly then runs on to the end on a blocking
/// thread nobody is waiting on — leaves the latch set with no error path to
/// clear it. Without a steal the upload can never be completed, aborted, or
/// swept: every one of those refuses a latched row. An hour is far longer than
/// any assembly and far shorter than the TTL.
pub(crate) const LATCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3600);

/// How many uploads may be open at once, in total.
///
/// The data dir carries the database, the CAS and this node's signing key, so
/// an unbounded number of open uploads is not "S3 writes get slow" — it is the
/// node losing the disk it needs to publish, or recover, at all.
pub(crate) const MAX_OPEN_UPLOADS: u64 = 10_000;

/// How many uploads one access key may hold open at once.
pub(crate) const MAX_OPEN_UPLOADS_PER_PRINCIPAL: u64 = 1_000;

/// How many bytes all staged parts may hold before new ones are refused.
pub(crate) const MAX_STAGED_BYTES: u64 = 256 * 1024 * 1024 * 1024;

/// Where a part's payload is being staged, and what it will be recorded as.
#[derive(Debug, Clone)]
pub struct PartStaging {
    /// The upload the part belongs to.
    pub upload: String,
    /// The part number.
    pub number: u32,
    /// The file the payload is written to, inside the upload's directory.
    pub path: PathBuf,
    /// The name that file is recorded under.
    pub file: String,
}

/// What a delete did, and what it left behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deleted {
    /// Whether some origin still publishes a live entry for the path.
    ///
    /// True means the delete did what it could and the key is still readable:
    /// another origin asserts it, and only that origin can retract it (§8).
    pub still_published: bool,
}

/// What a completion produced.
#[derive(Debug, Clone)]
pub struct CompletedUpload {
    /// The assembled object's root.
    pub root: Hash,
    /// Its size in bytes.
    pub size: u64,
}

impl Node {
    /// Where an upload will land, refusing anything outside a configured space.
    ///
    /// Resolved at creation rather than at completion so a path the space
    /// cannot hold is refused before the client streams a single part.
    pub fn upload_target(&self, space: &str, path: &str) -> Result<PathBuf> {
        if self.is_api_source(space)? {
            let normalized = synch_core::normalize_path(path)
                .map_err(|error| EngineError::invalid(error.to_string()))?;
            return Ok(PathBuf::from(format!("{space}/{normalized}")));
        }
        self.adoption_target(space, path)
    }

    /// Opens a multipart upload and returns its id.
    ///
    /// The directory and its parent are flushed before the row commits, which
    /// is the ordering the "a part row implies bytes" invariant rests on: the
    /// row is the thing that may be lost, and a lost row leaves a directory the
    /// sweeper collects rather than a pointer into nothing.
    pub fn create_upload(
        &self,
        space: &str,
        path: &str,
        principal: Option<&str>,
        _target: &Path,
    ) -> Result<String> {
        // A space that does not exist cannot hold an upload; `refuse_if_ignored`
        // passes over one rather than inventing an error, so it is named here.
        if self.store().source(space)?.is_none() {
            return Err(EngineError::not_found(format!("space {space}")));
        }
        // A key the scanner would skip can never become an object, and finding
        // that out at completion — after the client has streamed gigabytes and
        // the parts have been consumed — is the worst possible moment for it.
        // It is checked again there all the same: an upload outlives the rules
        // it opened under.
        self.refuse_if_ignored(space, path)?;
        self.check_upload_capacity(principal)?;
        let id = new_upload_id()?;
        let dir = self.store().upload_dir(&id);
        std::fs::create_dir_all(&dir)?;
        synch_core::fs::fsync_dir(&dir);
        synch_core::fs::fsync_dir(&self.store().uploads_dir());
        self.store()
            .create_upload(&id, space, path, principal, synch_core::now_ns())?;
        Ok(id)
    }

    /// Refuses a new upload when the node is already holding as much as it will.
    ///
    /// The only bound there is on what a client may make this node carry. The
    /// data dir holds the database, the CAS and this node's signing key, so an
    /// unbounded staging area is not "S3 writes get slow" — it is the node
    /// losing the disk it needs to publish, or recover, at all.
    fn check_upload_capacity(&self, principal: Option<&str>) -> Result<()> {
        let (total, mine) = self.store().open_upload_counts(principal)?;
        if total >= MAX_OPEN_UPLOADS {
            return Err(EngineError::invalid(format!(
                "this node is already holding {MAX_OPEN_UPLOADS} open multipart uploads"
            )));
        }
        if mine >= MAX_OPEN_UPLOADS_PER_PRINCIPAL {
            return Err(EngineError::invalid(format!(
                "you already hold {MAX_OPEN_UPLOADS_PER_PRINCIPAL} open multipart uploads"
            )));
        }
        let staged = self.store().staged_bytes()?;
        if staged >= MAX_STAGED_BYTES {
            return Err(EngineError::invalid(format!(
                "multipart staging is already holding {staged} byte(s)"
            )));
        }
        Ok(())
    }

    /// Checks an upload will still take this part, and says where to stage it.
    ///
    /// The part number is checked against S3's range here rather than at
    /// completion: a client that numbers parts wrongly should find out before
    /// it has pushed gigabytes, and without a bound a client could open an
    /// unbounded number of staged payloads against one upload.
    pub fn open_part(
        &self,
        upload: &str,
        space: &str,
        path: &str,
        principal: Option<&str>,
        number: u32,
    ) -> Result<PartStaging> {
        if number == 0 || number > MAX_PART_NUMBER {
            return Err(EngineError::invalid(format!(
                "part number {number} is outside 1..={MAX_PART_NUMBER}"
            )));
        }
        let staged = self.store().staged_bytes()?;
        if staged >= MAX_STAGED_BYTES {
            return Err(EngineError::invalid(format!(
                "multipart staging is already holding {staged} byte(s)"
            )));
        }
        let record = self.upload_for(upload, space, path, principal)?;
        if record.state != UploadState::Open {
            return Err(EngineError::invalid(format!(
                "upload {upload} is no longer accepting parts"
            )));
        }
        // A name per attempt, not per part number. Two clients racing the same
        // part number would otherwise share one file and interleave their
        // bytes, and the winner of the row write would describe the loser's
        // payload.
        let file = format!("{number:05}.{}{}", nonce(), crate::scanner::PART_SUFFIX);
        Ok(PartStaging {
            upload: upload.to_string(),
            number,
            path: self.store().upload_dir(upload).join(&file),
            file,
        })
    }

    /// Commits a staged part and records it.
    ///
    /// The payload is fsynced and renamed into its final name *before* the row
    /// is written, so a row never names a file that is not there. A crash
    /// between the two leaves an unreferenced payload, which the sweeper
    /// collects — the safe half of the asymmetry.
    pub(crate) fn commit_part(
        &self,
        staging: PartStaging,
        adoption: Adoption,
    ) -> Result<UploadPart> {
        let size = adoption.written();
        if size > MAX_PART_SIZE {
            return Err(EngineError::invalid(format!(
                "a part of {size} byte(s) is larger than the {MAX_PART_SIZE}-byte maximum"
            )));
        }
        let path = adoption.commit()?;
        let root = synch_core::hash_reader(std::io::BufReader::new(std::fs::File::open(&path)?))?;
        let part = UploadPart {
            number: staging.number,
            file: staging.file,
            size,
            root,
            created_ns: synch_core::now_ns(),
        };
        // A superseded attempt is left on disk deliberately: a completion may
        // already hold it open, and the sweeper collects payloads no row names.
        self.store().record_part(&staging.upload, &part)?;
        Ok(part)
    }

    /// Commits one gateway part to the backend's durable part store.
    ///
    /// Local nodes use the fsync-before-row path above. Cloud nodes upload the
    /// part object before recording its row, so an `UploadPart` acknowledgement
    /// never rests on ephemeral scratch.
    pub async fn commit_part_durable(
        &self,
        staging: PartStaging,
        adoption: Adoption,
    ) -> Result<UploadPart> {
        if !self.cas_backend().remote_upload_parts() {
            let node = self.clone();
            return crate::blocking::offload(move || node.commit_part(staging, adoption)).await;
        }
        let upload = staging.upload.clone();
        let file = staging.file.clone();
        let number = staging.number;
        let (path, size, root) = crate::blocking::offload(move || {
            let size = adoption.written();
            if size > MAX_PART_SIZE {
                return Err(EngineError::invalid(format!(
                    "a part of {size} byte(s) is larger than the {MAX_PART_SIZE}-byte maximum"
                )));
            }
            let path = adoption.commit()?;
            let root =
                synch_core::hash_reader(std::io::BufReader::new(std::fs::File::open(&path)?))?;
            Ok((path, size, root))
        })
        .await?;
        let key = cloud_part_key(&upload, &file);
        let uploaded = self.cas_backend().put_upload_part(key, path.clone()).await;
        let _ = tokio::fs::remove_file(&path).await;
        uploaded?;
        let part = UploadPart {
            number,
            file,
            size,
            root,
            created_ns: synch_core::now_ns(),
        };
        let (store, upload) = (self.store().clone(), upload);
        let recorded = part.clone();
        crate::blocking::offload(move || {
            store.record_part(&upload, &recorded)?;
            Ok(())
        })
        .await?;
        Ok(part)
    }

    /// Assembles the named parts, publishes the object, and reports its root.
    pub async fn complete_upload(
        &self,
        upload: &str,
        space: &str,
        path: &str,
        principal: Option<&str>,
        parts: &[(u32, Option<Hash>)],
    ) -> Result<CompletedUpload> {
        // Both reads take a store connection, so they go to the blocking pool
        // together rather than one at a time (§10): the authorization and the
        // latch belong to the same instant anyway.
        let start = {
            let (node, upload_id) = (self.clone(), upload.to_string());
            let (space_owned, path_owned) = (space.to_string(), path.to_string());
            let principal_owned = principal.map(str::to_string);
            crate::blocking::offload(move || {
                node.upload_for(
                    &upload_id,
                    &space_owned,
                    &path_owned,
                    principal_owned.as_deref(),
                )?;
                Ok(node.store().begin_complete(
                    &upload_id,
                    synch_core::now_ns(),
                    LATCH_TIMEOUT.as_nanos().try_into().unwrap_or(i64::MAX),
                )?)
            })
            .await?
        };
        let (recorded_space, recorded_path, available) = match start {
            // A retried completion. The client never saw the first answer, so
            // it gets that answer rather than being told its upload is gone.
            synch_store::CompleteStart::AlreadyCompleted { etag, size } => {
                return Ok(CompletedUpload { root: etag, size })
            }
            synch_store::CompleteStart::Ready { space, path, parts } => (space, path, parts),
        };
        // From here the upload holds the latch, so every failure has to put it
        // back: the client is entitled to fix its part list and retry, and an
        // upload stuck in `completing` could never be retried at all.
        match self
            .assemble(upload, &recorded_space, &recorded_path, parts, &available)
            .await
        {
            Ok(completed) => Ok(completed),
            Err(e) => {
                let (node, upload_id) = (self.clone(), upload.to_string());
                let unlatched: Result<()> =
                    crate::blocking::offload(move || Ok(node.store().reopen_upload(&upload_id)?))
                        .await;
                if let Err(unlatch) = unlatched {
                    tracing::warn!(upload, error = %unlatch, "could not reopen a failed upload");
                }
                Err(e)
            }
        }
    }

    /// The assembly itself, once the upload is latched.
    async fn assemble(
        &self,
        upload: &str,
        space: &str,
        path: &str,
        wanted: &[(u32, Option<Hash>)],
        available: &[UploadPart],
    ) -> Result<CompletedUpload> {
        let chosen = choose_parts(wanted, available)?;
        let dir = self.store().upload_dir(upload);
        let api_source = {
            let (node, space) = (self.clone(), space.to_string());
            crate::blocking::offload(move || node.is_api_source(&space)).await?
        };
        let remote_parts = self.cas_backend().remote_upload_parts();
        if remote_parts && !api_source {
            return Err(EngineError::invalid(
                "a cloud-CAS node cannot complete into a filesystem source",
            ));
        }
        // An API-source completion assembles into a staging file the daemon owns.
        // A filesystem-source completion assembles into the source itself and opens that
        // write through `open_adoption` below — which resolves the target, takes
        // the gates, and carries them to the commit, so an upload open for days
        // is judged by the rules in force when it lands rather than the ones it
        // started under.
        let target = if api_source {
            dir.join(format!(
                "assembled.{}{}",
                nonce(),
                crate::scanner::PART_SUFFIX
            ))
        } else {
            PathBuf::new()
        };

        let (root, size) = if remote_parts {
            tokio::fs::create_dir_all(&dir).await?;
            let mut output = tokio::fs::File::create(&target).await?;
            for part in &chosen {
                let key = cloud_part_key(upload, &part.file);
                let mut offset = 0u64;
                while offset < part.size {
                    let end = (offset + 8 * 1024 * 1024).min(part.size);
                    let bytes = self
                        .cas_backend()
                        .read_upload_part(key.clone(), offset..end)
                        .await?;
                    output.write_all(&bytes).await?;
                    offset = end;
                }
            }
            output.sync_all().await?;
            drop(output);
            let committed = self
                .commit_api_file(space, path, &target, synch_core::now_ns())
                .await;
            let _ = tokio::fs::remove_file(&target).await;
            committed?
        } else {
            let sources: Vec<PathBuf> = chosen.iter().map(|part| dir.join(&part.file)).collect();
            let assembled_target = target.clone();
            let (node, space_owned, path_owned) =
                (self.clone(), space.to_string(), path.to_string());
            let assembled = crate::blocking::offload(move || {
                // The space-validated open: the escape check runs at open and
                // the commit rename resolves against the directory pinned
                // then (or re-checks the guard, where there are no directory
                // handles), so the assembly streaming in between cannot be
                // redirected outside the space.
                let mut adoption = if api_source {
                    Adoption::at(&assembled_target)?
                } else {
                    node.open_adoption(&space_owned, &path_owned)?
                };
                for source in &sources {
                    adoption.append_file(source)?;
                }
                let written = adoption.written();
                let root = adoption.hash_staged()?;
                adoption.commit()?;
                Ok((root, written))
            })
            .await?;
            if api_source {
                let committed = self
                    .commit_api_file(space, path, &target, synch_core::now_ns())
                    .await;
                let _ = tokio::fs::remove_file(&target).await;
                let committed = committed?;
                if committed != assembled {
                    return Err(EngineError::invalid(
                        "multipart assembly changed during API-source ingest",
                    ));
                }
                committed
            } else {
                assembled
            }
        };

        // Publication is part of the completion promise, especially for a
        // API source with no watcher to repair it later.
        self.scan_publish_push().await?;

        let (node, upload_id) = (self.clone(), upload.to_string());
        crate::blocking::offload(move || {
            node.store()
                .finish_complete(&upload_id, &root, size, synch_core::now_ns())?;
            Ok(())
        })
        .await?;
        if remote_parts {
            if let Err(error) = self
                .cas_backend()
                .delete_upload_prefix(format!("uploads/{upload}/"))
                .await
            {
                tracing::warn!(upload, %error, "cloud upload prefix left for lifecycle sweep");
            }
        } else {
            for part in &chosen {
                let _ = std::fs::remove_file(dir.join(&part.file));
            }
        }
        let _ = tokio::fs::remove_dir_all(&dir).await;
        Ok(CompletedUpload { root, size })
    }

    /// Removes this node's copy of a path and publishes its tombstone (§8,
    /// §9.4).
    ///
    /// The delete half of `PutObject`, and it obeys the same rule: a write
    /// publishes *this node's own view*, because the version model has no way
    /// to publish anyone else's. So this removes our copy and asserts our
    /// tombstone, and that is the whole of what a delete can mean here. If
    /// another origin still publishes the path, the path is still in the
    /// unified tree afterwards — with one fewer version — and the caller is
    /// told so rather than left to discover it on the next read.
    ///
    /// Removing nothing is not a failure. S3 makes `DeleteObject` idempotent
    /// and tooling leans on it hard (`rm -f`, retried deletes, `rm` of a key a
    /// concurrent writer already removed), so a path that is already absent
    /// here is a delete that has already happened.
    pub async fn delete_object(&self, space: &str, path: &str) -> Result<Deleted> {
        // Before touching the file, not after: a node that cannot publish would
        // otherwise unlink the local copy and be unable to tell anyone (§3.4),
        // which loses data — the tombstone that would have justified the
        // removal never gets signed.
        // One trip to the blocking pool for both: the recovery check reads the
        // store, and §10 keeps store reads off the runtime workers.
        let node = self.clone();
        let (space_owned, path_owned) = (space.to_string(), path.to_string());
        crate::blocking::offload(move || {
            node.ensure_publishable()?;
            node.adopt_deletion(&space_owned, &path_owned)
        })
        .await?;

        // Unconditionally, and not only when a file was removed. The tombstone
        // comes from the scanner's deletion sweep, which walks `local_files`
        // rows, so a path whose file was already gone — removed out of band
        // while the daemon was down — would otherwise never be published as
        // deleted at all. An unchanged tree stages nothing, so being wrong here
        // costs one stat pass.
        self.scan_publish_push().await?;

        // The sweep tombstones what it can see, and what it can see is
        // `local_files`. If our own entry is *still live* after that, the sweep
        // never saw the path — so the tombstone is staged here, from the trie
        // rather than from the row that is missing. A row goes missing more
        // easily than it looks: `reconcile_local_files` drops any the current
        // head does not corroborate, and the control socket answers before the
        // startup scan has finished. Without this the delete returns `204` and
        // leaves this node asserting the key, signed, to every peer, with no
        // later scan or restart able to notice.
        let mine = synch_store::VersionPolicy::Origin(self.origin().clone());
        // `now` comes back from the same trip as the listing: it touches the
        // connection, and selecting every version against one instant is what
        // keeps a tombstone that expires mid-read from reading two ways.
        let (set, now) = {
            let (node, space_owned, path_owned) =
                (self.clone(), space.to_string(), path.to_string());
            crate::blocking::offload(move || {
                Ok((
                    node.versions(&space_owned, &path_owned)?,
                    node.store().read_instant()?,
                ))
            })
            .await?
        };
        if self
            .resolve_set(&set, &mine, now)
            .is_ok_and(|row| row.kind != synch_core::EntryKind::Tombstone)
        {
            tracing::warn!(
                space,
                path,
                "no local record backed this path; tombstoning it from the trie"
            );
            // The same tombstone the sweep would have staged: a *record*, not a
            // removal. Staging `None` retires the key outright, which is what
            // expiring a tombstone means — the path would read as "never
            // existed" rather than "deleted at seq N", and peers would lose the
            // assertion that tells them to stop serving their own copies.
            let tombstone = {
                let (node, space_owned, path_owned) =
                    (self.clone(), space.to_string(), path.to_string());
                crate::blocking::offload(move || {
                    let previous = node
                        .store()
                        .entry(node.origin(), &space_owned, &path_owned)?
                        .and_then(|entry| entry.content);
                    Ok(synch_core::FileEntry::tombstone(
                        synch_core::now_ns(),
                        node.next_seq()?,
                        previous,
                    ))
                })
                .await?
            };
            let encoded = synch_core::record::encode(&tombstone)?;
            self.stage([(synch_core::file_key(space, path)?, Some(encoded))]);
            self.flush_staged().await?;
        }

        // Whether the key survives is a question about *other* origins. Our own
        // entry is a tombstone by now, or was never there; counting it would
        // report every ordinary delete as one somebody else is still
        // publishing, which is the opposite of the warning it exists to give.
        let ours = self.origin();
        let set = {
            let (node, space_owned, path_owned) =
                (self.clone(), space.to_string(), path.to_string());
            crate::blocking::offload(move || node.versions(&space_owned, &path_owned)).await?
        };
        let still_published = set
            .entries
            .iter()
            .any(|entry| entry.origin != *ours && entry.kind != synch_core::EntryKind::Tombstone);
        Ok(Deleted { still_published })
    }

    /// Drops an upload and everything staged for it.
    pub fn abort_upload(
        &self,
        upload: &str,
        space: &str,
        path: &str,
        principal: Option<&str>,
    ) -> Result<bool> {
        match self.upload_for(upload, space, path, principal) {
            Ok(_) => {}
            // An abort of something that is not there is a success: it is not
            // there, which is what the caller asked for.
            Err(EngineError::NotFound(_)) => return Ok(false),
            Err(e) => return Err(e),
        }
        let existed = self.store().abort_upload(upload)?;
        if existed {
            let _ = std::fs::remove_dir_all(self.store().upload_dir(upload));
        }
        Ok(existed)
    }

    /// Drops an upload and its durable cloud part objects, if configured.
    pub async fn abort_upload_durable(
        &self,
        upload: &str,
        space: &str,
        path: &str,
        principal: Option<&str>,
    ) -> Result<bool> {
        let (node, upload_id, space, path, principal) = (
            self.clone(),
            upload.to_string(),
            space.to_string(),
            path.to_string(),
            principal.map(str::to_string),
        );
        let existed = crate::blocking::offload(move || {
            node.abort_upload(&upload_id, &space, &path, principal.as_deref())
        })
        .await?;
        if existed && self.cas_backend().remote_upload_parts() {
            self.cas_backend()
                .delete_upload_prefix(format!("uploads/{upload}/"))
                .await?;
        }
        Ok(existed)
    }

    /// Every upload still accepting parts under a prefix.
    pub fn open_uploads(
        &self,
        space: &str,
        prefix: &str,
        principal: Option<&str>,
    ) -> Result<Vec<synch_store::Upload>> {
        Ok(self.store().open_uploads(space, prefix, principal)?)
    }

    /// Every part recorded for one upload.
    pub fn upload_parts(
        &self,
        upload: &str,
        space: &str,
        path: &str,
        principal: Option<&str>,
    ) -> Result<Vec<UploadPart>> {
        self.upload_for(upload, space, path, principal)?;
        Ok(self.store().upload_parts(upload)?)
    }

    /// Reads an upload, insisting it belongs to the key the caller named.
    ///
    /// An upload id is a bearer token for one key. Answering a request that
    /// quotes it against a *different* key would let a client complete an
    /// upload into a path it never named — and since two buckets may map to one
    /// space, the comparison has to be on the space and path rather than on the
    /// bucket the request arrived at.
    fn upload_for(
        &self,
        upload: &str,
        space: &str,
        path: &str,
        principal: Option<&str>,
    ) -> Result<synch_store::Upload> {
        let record = self
            .store()
            .upload(upload)?
            .ok_or_else(|| EngineError::not_found(format!("upload {upload}")))?;
        // Same answer for the wrong key and the wrong principal, and
        // deliberately: "no such upload" is all an unauthorized caller learns,
        // where "wrong owner" would confirm the id it guessed is real.
        if record.space != space || record.path != path || record.principal.as_deref() != principal
        {
            return Err(EngineError::not_found(format!(
                "upload {upload} is not against {space}/{path}"
            )));
        }
        Ok(record)
    }

    /// Collects uploads nobody finished, and payloads no row names.
    ///
    /// Two sweeps, because there are two ways to leak. An *upload* leaks when a
    /// client walks away, and is collected on its age. A *payload* leaks when a
    /// crash lands between the rename and the row, or when a re-uploaded part
    /// supersedes an attempt a completion might still have had open — those sit
    /// inside a directory that is very much still live, so no directory-level
    /// sweep would ever see them.
    pub(crate) fn sweep_uploads(&self, ttl: std::time::Duration) -> Result<usize> {
        let ttl_ns = i64::try_from(ttl.as_nanos()).unwrap_or(i64::MAX);
        let cutoff = synch_core::now_ns().saturating_sub(ttl_ns);
        let mut collected = 0;
        for id in self.store().uploads_before(cutoff)? {
            // `abort_upload` refuses while a completion holds the latch, which
            // is what should happen: an assembly in flight is not abandoned. A
            // latch nothing is behind any more ages out on its own and is
            // collected on a later pass.
            match self.store().expire_upload(&id) {
                Ok(true) => {
                    let _ = std::fs::remove_dir_all(self.store().upload_dir(&id));
                    collected += 1;
                }
                Ok(false) => {}
                Err(e) => tracing::debug!(upload = %id, error = %e, "upload not swept"),
            }
        }

        let live: std::collections::HashSet<String> =
            self.store().upload_ids()?.into_iter().collect();
        let root = self.store().uploads_dir();
        let Ok(entries) = std::fs::read_dir(&root) else {
            return Ok(collected);
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if !live.contains(&name) {
                // Either a crash between the mkdir and the row, or a row already
                // swept. The grace window keeps this off a directory
                // `create_upload` made a moment ago. A stray *file* here is
                // removed as one: `remove_dir_all` refuses it, and nothing else
                // would ever come back for it.
                if older_than(&path, ORPHAN_GRACE) {
                    let removed = if path.is_dir() {
                        std::fs::remove_dir_all(&path).is_ok()
                    } else {
                        std::fs::remove_file(&path).is_ok()
                    };
                    if removed {
                        collected += 1;
                    }
                }
                continue;
            }
            // One bad row must not end the pass: everything else in this loop is
            // best-effort, and a sweep that gives up on the first oddity leaves
            // every later upload uncollected forever.
            let named: std::collections::HashSet<String> = match self.store().upload_parts(&name) {
                Ok(parts) => parts.into_iter().map(|part| part.file).collect(),
                Err(e) => {
                    tracing::debug!(upload = %name, error = %e, "parts not readable; not swept");
                    continue;
                }
            };
            let Ok(payloads) = std::fs::read_dir(&path) else {
                continue;
            };
            for payload in payloads.flatten() {
                let file = payload.file_name().to_string_lossy().into_owned();
                // An `Adoption`'s own staging file lives in this directory while
                // a part is still streaming, under a dot-prefixed name no row
                // will ever mention. It is not an orphan — it is a write in
                // progress, and a client that paused for longer than the grace
                // window would find its part unlinked from under an open handle
                // and fail for a reason it could do nothing about.
                if file.starts_with('.') {
                    continue;
                }
                if !named.contains(&file) && older_than(&payload.path(), ORPHAN_GRACE) {
                    let _ = std::fs::remove_file(payload.path());
                    collected += 1;
                }
            }
        }
        Ok(collected)
    }

    /// Runs the upload TTL sweep and removes the corresponding cloud prefixes.
    pub async fn sweep_uploads_durable(&self, ttl: std::time::Duration) -> Result<usize> {
        let cutoff =
            synch_core::now_ns().saturating_sub(i64::try_from(ttl.as_nanos()).unwrap_or(i64::MAX));
        let old = {
            let store = self.store().clone();
            crate::blocking::offload(move || Ok(store.uploads_before(cutoff)?)).await?
        };
        let node = self.clone();
        let collected = crate::blocking::offload(move || node.sweep_uploads(ttl)).await?;
        if self.cas_backend().remote_upload_parts() {
            let store = self.store().clone();
            let cleanup = crate::blocking::offload(move || {
                let mut cleanup = Vec::new();
                for id in old {
                    match store.upload(&id)? {
                        None => cleanup.push(id),
                        Some(upload) if upload.state == UploadState::Completed => cleanup.push(id),
                        Some(_) => {}
                    }
                }
                Ok(cleanup)
            })
            .await?;
            for upload in cleanup {
                if let Err(error) = self
                    .cas_backend()
                    .delete_upload_prefix(format!("uploads/{upload}/"))
                    .await
                {
                    tracing::warn!(%upload, %error, "cloud upload prefix left for lifecycle sweep");
                }
            }
        }
        Ok(collected)
    }

    /// Returns every interrupted completion to `open`, at startup.
    ///
    /// A completion severed by a daemon stop or a crash leaves the latch set,
    /// and nothing else ever clears it: without this the upload would refuse
    /// every retry until its bytes aged out, even though every part is still
    /// there.
    pub fn reopen_interrupted_uploads(&self) -> Result<usize> {
        Ok(self.store().reopen_interrupted_uploads()?)
    }
}

/// How long an unreferenced payload or directory must sit before it is swept.
///
/// The sweep races the writers by construction — a directory exists before its
/// row, and a payload exists before the row that names it — so it only ever
/// collects what has been unreferenced for longer than any of those windows.
const ORPHAN_GRACE: std::time::Duration = std::time::Duration::from_secs(3600);

/// Whether a path was last modified longer ago than `grace`.
///
/// A path whose age cannot be read is treated as young: refusing to guess is
/// what keeps a sweep from deleting something it could not measure.
fn older_than(path: &Path, grace: std::time::Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .and_then(|at| at.elapsed().map_err(std::io::Error::other))
        .map(|age| age > grace)
        .unwrap_or(false)
}

/// Picks the parts a completion named, in the order S3 requires.
///
/// Every check S3 makes at completion time, and for its reasons: a client that
/// names parts out of order has lost track of its own upload, one that names a
/// part that is not there has lost an upload it thinks succeeded, and one whose
/// interior part is under the minimum has produced an object no other S3
/// implementation would have accepted.
fn choose_parts<'a>(
    wanted: &[(u32, Option<Hash>)],
    available: &'a [UploadPart],
) -> Result<Vec<&'a UploadPart>> {
    if wanted.is_empty() {
        return Err(EngineError::invalid("a completion names no parts"));
    }
    // Order first, then existence, then sizes — and all of each before any of
    // the next. Reporting a part as too small when a *later* part was never
    // uploaded sends the client to shrink a part that was fine, and it retries
    // into the same failure.
    let mut previous = 0;
    for (number, _) in wanted {
        if *number <= previous {
            return Err(EngineError::invalid(format!(
                "part {number} is out of order"
            )));
        }
        previous = *number;
    }
    let mut chosen = Vec::with_capacity(wanted.len());
    for (number, expected) in wanted {
        let part = available
            .iter()
            .find(|part| part.number == *number)
            .ok_or_else(|| EngineError::invalid(format!("part {number} was never uploaded")))?;
        // The client echoes back the root it was given, and checking it is the
        // point of having handed one over: a mismatch means the part it thinks
        // it uploaded is not the part that is here.
        if let Some(expected) = expected {
            if part.root != *expected {
                return Err(EngineError::invalid(format!(
                    "part {number} does not have the root the completion named"
                )));
            }
        }
        chosen.push(part);
    }
    // Every part but the last has to reach the minimum. The last one is exempt
    // because an object is rarely a whole number of parts long.
    for part in &chosen[..chosen.len() - 1] {
        if part.size < MIN_PART_SIZE {
            return Err(EngineError::invalid(format!(
                "part {} is {} byte(s), under the {MIN_PART_SIZE}-byte minimum for a part that is not the last",
                part.number, part.size
            )));
        }
    }
    Ok(chosen)
}

/// A fresh upload id: 32 hex characters, from the system CSPRNG.
///
/// It has to be *unguessable*, not merely unique. An id is what authorizes a
/// caller to add parts to an upload and to complete it, so an id derived from a
/// clock, a counter and a pid — all of them observable or bounded — is a
/// password an attacker can search. 128 bits from the OS is the whole of the
/// defence.
///
/// Hex, never base64: the id travels as a query parameter, and `+` has no
/// business needing an escape in one.
fn new_upload_id() -> Result<String> {
    use aws_lc_rs::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    aws_lc_rs::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| EngineError::invalid("the system random source is unavailable"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Sixteen hex characters of process-local uniqueness.
///
/// A tiebreaker for a filename inside an already-authorized directory, and
/// nothing else — it is not, and must not become, a secret.
fn nonce() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut seed = Vec::with_capacity(20);
    seed.extend_from_slice(&synch_core::now_ns().to_le_bytes());
    seed.extend_from_slice(&count.to_le_bytes());
    seed.extend_from_slice(&std::process::id().to_le_bytes());
    Hash::new(&seed).to_hex()[..16].to_string()
}

fn cloud_part_key(upload: &str, file: &str) -> String {
    format!("uploads/{upload}/{file}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cloud_parts_are_durable_before_rows_and_complete_an_api_source() {
        let data = tempfile::tempdir().unwrap();
        Node::init(data.path(), None).unwrap();
        let mut config = crate::config::NodeConfig::loopback(data.path());
        config.cloud = Some(synch_store::cloud::CloudConfig {
            service: synch_store::cloud::CloudService::Memory,
            options: Default::default(),
            scratch_dir: data.path().join("cloud-scratch"),
            io_timeout: std::time::Duration::from_secs(5),
            upload_policy: synch_store::cloud::CloudUploadPolicy::OwnPinned,
            cache_bytes: Some(512 * 1024 * 1024),
        });
        let node = Node::open(config).await.unwrap();
        node.add_api_source("media").unwrap();
        let target = node.upload_target("media", "joined.bin").unwrap();
        let upload = node
            .create_upload("media", "joined.bin", None, &target)
            .unwrap();
        let head = vec![7u8; MIN_PART_SIZE as usize];
        let tail = b"cloud tail".to_vec();
        let mut keys = Vec::new();
        for (number, bytes) in [(1u32, &head), (2u32, &tail)] {
            let staging = node
                .open_part(&upload, "media", "joined.bin", None, number)
                .unwrap();
            let key = cloud_part_key(&upload, &staging.file);
            let mut adoption = Adoption::at(&staging.path).unwrap();
            adoption.write(bytes).unwrap();
            let part = node.commit_part_durable(staging, adoption).await.unwrap();
            assert_eq!(part.size, bytes.len() as u64);
            assert_eq!(
                node.cas_backend()
                    .read_upload_part(key.clone(), 0..bytes.len() as u64)
                    .await
                    .unwrap(),
                **bytes
            );
            keys.push(key);
        }

        let completed = node
            .complete_upload(
                &upload,
                "media",
                "joined.bin",
                None,
                &[(1, None), (2, None)],
            )
            .await
            .unwrap();
        let mut expected = head;
        expected.extend_from_slice(&tail);
        assert_eq!(completed.root, Hash::new(&expected));
        assert_eq!(completed.size, expected.len() as u64);
        assert!(node.store().blob(&completed.root).unwrap().unwrap().durable);
        assert_eq!(
            node.store()
                .entry(node.origin(), "media", "joined.bin")
                .unwrap()
                .unwrap()
                .content,
            Some(completed.root)
        );
        for key in keys {
            assert!(matches!(
                node.cas_backend().read_upload_part(key, 0..1).await,
                Err(synch_store::StoreError::CloudNotFound { .. })
            ));
        }
        node.shutdown().await.unwrap();
    }

    fn part(number: u32, size: u64) -> UploadPart {
        UploadPart {
            number,
            file: format!("{number}"),
            size,
            root: Hash::new(&number.to_le_bytes()),
            created_ns: 0,
        }
    }

    fn named(numbers: &[u32]) -> Vec<(u32, Option<Hash>)> {
        numbers.iter().map(|n| (*n, None)).collect()
    }

    #[test]
    fn choose_parts_enforces_the_selection_rules() {
        let available = vec![part(1, MIN_PART_SIZE), part(2, MIN_PART_SIZE), part(3, 10)];
        // In order, whole or as a subset; the parts left out are discarded.
        assert_eq!(
            choose_parts(&named(&[1, 2, 3]), &available).unwrap().len(),
            3
        );
        assert_eq!(choose_parts(&named(&[1, 3]), &available).unwrap().len(), 2);
        // Descending, duplicated, or empty orders are refused.
        assert!(choose_parts(&named(&[2, 1]), &available).is_err());
        assert!(choose_parts(&named(&[1, 1]), &available).is_err());
        assert!(choose_parts(&[], &available).is_err());
        // Only the last part may be small: ten bytes alone is fine, as a
        // first part it is not.
        let small = vec![part(1, 10), part(2, 10)];
        assert!(choose_parts(&named(&[1]), &small).is_ok());
        assert!(choose_parts(&named(&[1, 2]), &small).is_err());

        // The named part must be the available one: a wrong hash is refused,
        // and a missing part is reported by name.
        let wrong = vec![(1u32, Some(Hash::new(b"something else")))];
        assert!(choose_parts(&wrong, &available).is_err());
        let right = vec![(1u32, Some(available[0].root))];
        assert!(choose_parts(&right, &available).is_ok());
        let err = choose_parts(&named(&[1, 9]), &available)
            .unwrap_err()
            .to_string();
        assert!(err.contains("part 9 was never uploaded"), "{err}");
    }
}

#[cfg(test)]
mod sweeper_tests {
    use super::*;
    use crate::testkit::node_with_space;

    /// The sweeper collects what nobody is coming back for, and nothing else:
    /// a part still streaming is not an orphan — its staging file lives under
    /// a dot-prefixed name no row mentions, so sweeping it would unlink a
    /// part from under an open handle.
    #[tokio::test]
    async fn the_sweeper_collects_only_what_is_abandoned() {
        let (_d, _s, node) = node_with_space().await;
        let target = node.upload_target("media", "a.bin").unwrap();
        let id = node.create_upload("media", "a.bin", None, &target).unwrap();

        assert_eq!(node.sweep_uploads(std::time::Duration::ZERO).unwrap(), 1);
        assert!(node.store().upload(&id).unwrap().is_none());
        assert!(!node.store().upload_dir(&id).exists());

        let id = node.create_upload("media", "b.bin", None, &target).unwrap();
        assert_eq!(node.sweep_uploads(DEFAULT_UPLOAD_TTL).unwrap(), 0);
        assert!(node.store().upload(&id).unwrap().is_some());

        // A part still streaming survives the sweep and commits afterwards.
        let id = node.create_upload("media", "c.bin", None, &target).unwrap();
        let staging = node.open_part(&id, "media", "c.bin", None, 1).unwrap();
        let mut adoption = crate::scanner::Adoption::at(&staging.path).unwrap();
        adoption.write(b"still arriving").unwrap();
        node.sweep_uploads(DEFAULT_UPLOAD_TTL).unwrap();
        let part = node.commit_part(staging, adoption).unwrap();
        assert_eq!(part.size, 14);
        assert_eq!(node.store().upload_parts(&id).unwrap().len(), 1);
        node.shutdown().await.unwrap();
    }

    /// A completion into a node that cannot publish lands nothing (issue #71).
    ///
    /// End to end rather than at the moment of the rename: the assembly opens
    /// its write through `open_adoption`, so this is refused at the open. What
    /// pins the *commit* — a node floored between the open and the rename, on
    /// an upload that may have been assembling for minutes — is
    /// `a_space_write_re_takes_its_gates_at_the_commit` in scanner.rs, where
    /// the gate now lives.
    #[tokio::test]
    async fn a_completion_floored_after_it_opened_does_not_land() {
        let (_d, space, node) = node_with_space().await;
        let victim = space.path().join("joined.bin");
        std::fs::write(&victim, b"mine, unpublished").unwrap();

        let target = node.upload_target("media", "joined.bin").unwrap();
        let id = node
            .create_upload("media", "joined.bin", None, &target)
            .unwrap();
        let bytes = vec![7u8; MIN_PART_SIZE as usize];
        let staging = node.open_part(&id, "media", "joined.bin", None, 1).unwrap();
        let mut adoption = crate::scanner::Adoption::at(&staging.path).unwrap();
        adoption.write(&bytes).unwrap();
        node.commit_part(staging, adoption).unwrap();

        // A peer advertises a head for our own origin that we have no history
        // for — key-loss recovery, arriving while the upload is open.
        node.store()
            .record_observed_head(
                node.origin(),
                100,
                &synch_core::Hash([7u8; 32]),
                true,
                None,
                synch_core::now_ns(),
            )
            .unwrap();

        let refused = node
            .complete_upload(&id, "media", "joined.bin", None, &[(1, None)])
            .await
            .unwrap_err()
            .to_string();
        assert!(refused.contains("recover"), "{refused}");
        assert_eq!(
            std::fs::read(&victim).unwrap(),
            b"mine, unpublished",
            "the assembly must not have renamed over a file nothing could then \
             publish a replacement for"
        );
        node.shutdown().await.unwrap();
    }

    /// A completion answers with the root of the bytes it assembled.
    #[tokio::test]
    async fn a_completion_hashes_what_it_assembled() {
        let (_d, space, node) = node_with_space().await;
        let target = node.upload_target("media", "joined.bin").unwrap();
        let id = node
            .create_upload("media", "joined.bin", None, &target)
            .unwrap();

        let head = vec![7u8; MIN_PART_SIZE as usize];
        let tail = b"and the tail".to_vec();
        for (number, bytes) in [(1u32, &head), (2u32, &tail)] {
            let staging = node
                .open_part(&id, "media", "joined.bin", None, number)
                .unwrap();
            let mut adoption = crate::scanner::Adoption::at(&staging.path).unwrap();
            adoption.write(bytes).unwrap();
            node.commit_part(staging, adoption).unwrap();
        }
        let done = node
            .complete_upload(&id, "media", "joined.bin", None, &[(1, None), (2, None)])
            .await
            .unwrap();

        let mut expected = head.clone();
        expected.extend_from_slice(&tail);
        assert_eq!(done.size, expected.len() as u64);
        assert_eq!(done.root, synch_core::Hash::new(&expected));
        assert_eq!(
            std::fs::read(space.path().join("joined.bin")).unwrap(),
            expected
        );
        // The parts and their directory go once the answer is recorded.
        assert!(!node.store().upload_dir(&id).exists());
        assert!(node.store().upload_parts(&id).unwrap().is_empty());
        assert_eq!(node.sweep_uploads(std::time::Duration::ZERO).unwrap(), 1);
        assert!(node.store().upload(&id).unwrap().is_none());
        node.shutdown().await.unwrap();
    }
}

#[cfg(test)]
mod ownership_tests {
    use crate::testkit::node_with_space;

    /// An upload id is a bearer token for one key *and* one principal.
    #[tokio::test]
    async fn another_principal_cannot_touch_an_upload() {
        let (_d, _s, node) = node_with_space().await;
        let target = node.upload_target("media", "a.bin").unwrap();
        let id = node
            .create_upload("media", "a.bin", Some("AKIA1"), &target)
            .unwrap();

        // The owner can; nobody else can, however they hold it wrong — all
        // told the same thing, so a guessed id is never confirmed real.
        assert!(node
            .open_part(&id, "media", "a.bin", Some("AKIA1"), 1)
            .is_ok());
        for wrong in [Some("AKIA2"), None] {
            assert!(node.open_part(&id, "media", "a.bin", wrong, 1).is_err());
            assert!(node.upload_parts(&id, "media", "a.bin", wrong).is_err());
            assert!(!node.abort_upload(&id, "media", "a.bin", wrong).unwrap());
        }
        assert!(node
            .open_part(&id, "media", "elsewhere.bin", Some("AKIA1"), 1)
            .is_err());
        // A listing shows it to its owner and nobody else, and the owner can
        // still abort it.
        assert_eq!(
            node.open_uploads("media", "", Some("AKIA1")).unwrap().len(),
            1
        );
        assert!(node
            .open_uploads("media", "", Some("AKIA2"))
            .unwrap()
            .is_empty());
        assert!(node.open_uploads("media", "", None).unwrap().is_empty());
        assert!(node
            .abort_upload(&id, "media", "a.bin", Some("AKIA1"))
            .unwrap());
        node.shutdown().await.unwrap();
    }
}
