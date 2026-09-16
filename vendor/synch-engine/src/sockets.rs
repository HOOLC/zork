//! Sockets, from the engine's side (`docs/SOCKETS.md`).
//!
//! Three things live here, and they are deliberately separate.
//!
//! **Resolution** answers "what would run, if anything?" — an activation check
//! and a lookup in *this node's own* trie. It is the whole of the rule the
//! design is built on, and it never consults the unified tree: connecting to a
//! socket names an origin, and `newest` would otherwise let any member's
//! `mtime_ns` decide whose program a connection lands on.
//!
//! **Admission** turns that into an invocation: it adds the caller's identity
//! as the handshake established it and the capabilities the program's own
//! manifest declares. The root it resolves is the invocation's snapshot — a
//! deployment landing mid-stream changes what the *next* invocation runs,
//! never what a running one is executing.
//!
//! **The tree host** is what the running program reads through. It is the one
//! place a socket touches the rest of the node, and it is read-only.

use std::sync::Arc;

use synch_core::{
    Declaration, EntryKind, Hash, NodeId, OriginId, RefuseCode, SockOpen, SockStatus,
};
use synch_sock::{
    Admission, DuplexStream, EffectivePolicy, HostError, Limits, ObjectInfo, PeerIdentity,
    PutCondition, PutReceipt, SocketHost, SocketId, SocketWriter,
};
use synch_store::SocketActivation;

use crate::{
    error::{EngineError, Result},
    node::Node,
};

/// What a socket path resolves to right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The content root the tree currently names — the snapshot an invocation
    /// admitted against it will run, however the path moves afterwards.
    pub root: Hash,
    /// Its size in bytes.
    pub size: u64,
    /// The activation that makes the path a socket.
    pub activation: SocketActivation,
}

impl Node {
    /// Makes a path in one of this node's spaces a socket, until
    /// [`Node::socket_deactivate`].
    ///
    /// From the next scan the path publishes as `EntryKind::Socket`, and every
    /// later write to it — an editor save, an adoption, an S3 `PUT` — is an
    /// intentional deployment: the new content serves as soon as it publishes,
    /// under whatever its own manifest declares. That breadth is the grant, and
    /// `synch socket activate` says so where it is asked for.
    pub fn socket_activate(&self, row: &SocketActivation) -> Result<()> {
        let _authorization = self.socket_authorization_write();
        let Some(space) = self.store().source(&row.space)? else {
            return Err(EngineError::invalid(format!(
                "`{}` is not a filesystem source",
                row.space
            )));
        };
        if space.local_path.is_none() {
            return Err(EngineError::invalid(format!(
                "source `{}` is API-only and has no scanner that can publish a socket",
                row.space
            )));
        }
        let mut row = row.clone();
        row.path = synch_core::normalize_path(&row.path)
            .map_err(|e| EngineError::invalid(e.to_string()))?;
        self.store().activate_socket(&row)?;
        // Kind is part of the published entry even when the file's bytes and
        // stat are unchanged. Invalidate the scanner cache so its next pass
        // reaches the activation check instead of returning early.
        self.store().remove_local_file(&row.space, &row.path)?;
        // A re-activation is a new bargain; a session table minted under the
        // old terms is not state the new terms agreed to inherit.
        self.clear_socket_map(&row.qualified());
        Ok(())
    }

    /// Every path this node has activated.
    pub fn socket_ls(&self, space: Option<&str>) -> Result<Vec<SocketActivation>> {
        Ok(match space {
            Some(space) => self.store().socket_activations_in(space)?,
            None => self.store().socket_activations()?,
        })
    }

    /// Removes an activation.
    ///
    /// Admission refuses immediately — the write side of the authorization
    /// gate excludes in-flight admissions — and the next scan republishes the
    /// path as an ordinary file, because the kind comes from the activation
    /// and there is no longer one. Invocations already running keep their
    /// snapshot and finish.
    pub fn socket_deactivate(&self, space: &str, path: &str) -> Result<bool> {
        let _authorization = self.socket_authorization_write();
        let out = self.store().deactivate_socket(space, path)?;
        if out {
            self.store().remove_local_file(space, path)?;
        }
        self.clear_socket_map(&format!("{space}/{path}"));
        Ok(out)
    }

    /// The declaration the program at `root` carries in its manifest.
    ///
    /// A read of this node's own CAS plus a bounded parse — nothing executes.
    /// What `synch socket ls -l` shows, from the same parse admission uses.
    pub fn socket_program_declaration(&self, root: &Hash) -> Result<Declaration> {
        let elf = self.read_socket_bytes(root)?;
        synch_sock::manifest::manifest_declaration(&elf)
            .map_err(|e| EngineError::invalid(e.to_string()))
    }

    /// Resolves a socket path in **this node's own** trie.
    ///
    /// `Ok(None)` means the path is not activated, this node publishes no
    /// entry there, or publishes one that is not a socket. The distinctions
    /// are drawn by the caller, which has a refusal code for each.
    pub fn resolve_socket(&self, space: &str, path: &str) -> Result<Option<Resolved>> {
        let Some(activation) = self.store().socket_activation(space, path)? else {
            return Ok(None);
        };
        let Some(entry) = self.store().entry(self.origin(), space, path)? else {
            return Ok(None);
        };
        if entry.kind != EntryKind::Socket {
            return Ok(None);
        }
        let Some(root) = entry.content else {
            return Ok(None);
        };
        Ok(Some(Resolved {
            root,
            size: entry.size,
            activation,
        }))
    }

    /// Reads a socket's ELF object out of this node's own CAS, sharing the
    /// allocation across every admission of one content root.
    ///
    /// Sixty-four streams into one socket must not mean sixty-four copies of
    /// its program: the bytes are immutable once the root is fixed, so all of
    /// the admissions of a root get the same `Arc` — including the concurrent
    /// ones, which wait on the first load's watch channel rather than each
    /// reading the CAS themselves. The [`ProgramBytesCache`] retains up to
    /// [`MAX_CACHED_PROGRAMS`] of them strongly, oldest first.
    async fn socket_program(&self, resolved: &Resolved) -> Result<Arc<Vec<u8>>> {
        let limits = self.socket_limits();
        if resolved.size > limits.max_program_bytes {
            return Err(EngineError::invalid(format!(
                "the program is {} bytes, past the {} a socket may be",
                resolved.size, limits.max_program_bytes
            )));
        }
        match self.socket_program_load(&resolved.root) {
            ProgramLoad::Ready(bytes) => Ok(bytes),
            ProgramLoad::InFlight(mut receiver) => {
                receiver.wait_for(Option::is_some).await.map_err(|_| {
                    EngineError::invalid("a concurrent socket program load was interrupted")
                })?;
                match receiver.borrow().clone().expect("the loader published") {
                    Ok(bytes) => Ok(bytes),
                    Err(text) => Err(EngineError::invalid(text)),
                }
            }
            ProgramLoad::Loader(guard) => {
                // Its own CAS: the bytes were published by this node, so they
                // are held here. `ensure_cached` covers the one case where
                // they are not — a cloud-backed node whose scratch cache has
                // been replaced. If this future is cancelled or panics before
                // `finish`, the guard's drop publishes the failure and frees
                // the root for a fresh load.
                let outcome: ProgramBytes = (async {
                    self.ensure_blob_cached(&resolved.root, resolved.size)
                        .await
                        .map_err(|e| e.to_string())?;
                    self.cas_backend()
                        .read_range(resolved.root, 0, resolved.size)
                        .await
                        .map_err(|e| e.to_string())
                        .map(Arc::new)
                })
                .await;
                guard.finish(outcome.clone());
                outcome.map_err(EngineError::invalid)
            }
        }
    }

    /// Builds the policy one invocation runs under: what the program's own
    /// manifest declares, capped by the activation's and the daemon's limits.
    fn socket_policy(
        &self,
        declared: &Declaration,
        activation: &SocketActivation,
    ) -> EffectivePolicy {
        EffectivePolicy::granted(
            declared,
            activation.config.clone(),
            activation.max_streams,
            self.socket_limits().max_streams,
        )
    }

    /// Resolves and authorizes an `Open`.
    ///
    /// Every refusal here is a distinct code, because the caller can act on the
    /// difference: `ProgramInvalid` is the operator's to fix by deploying,
    /// `SpaceNotDelegated` is the caller's, and `NoSuchPath` means look again.
    pub(crate) async fn admit_socket(
        &self,
        peer: NodeId,
        addr: String,
        stream_index: u64,
        open: &SockOpen,
    ) -> std::result::Result<Admission, (RefuseCode, String)> {
        if !synch_sock::SUPPORTED {
            return Err((
                RefuseCode::Unsupported,
                "this node has no eBPF runtime: async-ebpf supports Linux, macOS and OpenBSD on \
                 x86-64 and arm64"
                    .into(),
            ));
        }
        // The `Open` names the origin it is addressed to, so a frame relayed or
        // replayed at another node is undeliverable rather than redirected.
        if open.origin != *self.origin() {
            return Err((
                RefuseCode::NoSuchPath,
                format!(
                    "this node is {}, not {}",
                    self.origin().canonical(),
                    open.origin.canonical()
                ),
            ));
        }

        let node = self.clone();
        let scope = crate::blocking::offload(move || {
            Ok(node
                .store()
                .socket_scope_for_key(&peer, synch_core::now_ns())?)
        })
        .await
        .map_err(|e| (RefuseCode::Unauthorized, e.to_string()))?;
        let Some((origin, scope)) = scope else {
            return Err((
                RefuseCode::Unauthorized,
                "no live binding for that device key".into(),
            ));
        };
        if let Some(spaces) = &scope {
            if !spaces.contains(&open.space) {
                return Err((
                    RefuseCode::SpaceNotDelegated,
                    format!("`{}` is not one of your delegated spaces", open.space),
                ));
            }
        }

        let node = self.clone();
        let (space, path) = (open.space.clone(), open.path.clone());
        let resolved = crate::blocking::offload(move || {
            let resolved = node.resolve_socket(&space, &path)?;
            let ordinary = if resolved.is_none() {
                node.store().entry(node.origin(), &space, &path)?.is_some()
            } else {
                false
            };
            Ok((resolved, ordinary))
        })
        .await
        .map_err(|e| (RefuseCode::NoSuchPath, e.to_string()))?;
        let resolved = match resolved {
            (Some(resolved), _) => resolved,
            (None, ordinary) => {
                // Told apart so the caller learns something: a path this node
                // publishes as an ordinary file is a different mistake from a
                // path it publishes nothing for.
                let code = if ordinary {
                    RefuseCode::NotASocket
                } else {
                    RefuseCode::NoSuchPath
                };
                return Err((code, format!("{}/{}", open.space, open.path)));
            }
        };

        // The program's bytes and manifest come before the registry slot: the
        // policy an invocation runs under — its stream cap included — is
        // declared in the object itself, so nothing can be reserved until the
        // object has been read and its manifest parsed. Concurrent cold
        // admissions of one root still coalesce onto one CAS read
        // (`ProgramBytesCache`), and the authorization re-check below closes
        // the window this opens.
        let program = match self.socket_program(&resolved).await {
            Ok(bytes) => bytes,
            Err(e) => return Err((RefuseCode::ProgramInvalid, e.to_string())),
        };
        // The manifest gate: an update whose manifest does not parse — or
        // that has no stream entrypoint at all — keeps the path activated and
        // published, and refuses every connection with a message that names
        // the defect. Deploying a fixed object is the whole remedy.
        let declared = match synch_sock::manifest::manifest_declaration(&program) {
            Ok(declared) => declared,
            Err(e) => return Err((RefuseCode::ProgramInvalid, e.to_string())),
        };
        if !synch_sock::manifest::has_stream_section(&program) {
            return Err((
                RefuseCode::ProgramInvalid,
                "the program has no `synchronicity.stream` entrypoint".into(),
            ));
        }

        // Authorization is deliberately not held across the CAS wait above,
        // so it is checked again while the registry slot is made live.
        // Activation and deactivation take the write side of this gate, so
        // either this admission becomes in-flight first or the deactivation
        // wins and this request is refused.
        let qualified = format!("{}/{}", open.space, open.path);
        let node = self.clone();
        let (space, path) = (open.space.clone(), open.path.clone());
        let checked_root = resolved.root;
        let qualified_for_slot = qualified.clone();
        let peer_name = origin.canonical();
        // The store read and the registry reservation are one blocking-pool
        // closure so the authorization read guard spans both; there is no
        // unchecked gap between them. A concurrent *deployment* — the content
        // replaced mid-admission — refuses this admission too: the caller
        // retries and lands on the new root, so a new invocation never runs
        // bytes the tree no longer names. The effective policy is computed from
        // the activation this re-read observes, not the one taken before the
        // CAS wait: a re-activation that rotated the config or tightened the
        // stream cap in that window is a new bargain, and the slot is reserved
        // against its cap.
        let prepared = crate::blocking::offload(move || {
            let _authorization = node.socket_authorization_read();
            let current = match node.resolve_socket(&space, &path)? {
                Some(current) => current,
                None => {
                    return Ok(Err((
                        RefuseCode::NotActivated,
                        "the socket was deactivated or republished during admission".into(),
                    )))
                }
            };
            if current.root != checked_root {
                return Ok(Err((
                    RefuseCode::NotActivated,
                    "the socket's content was replaced during admission; connect again to \
                     reach the new program"
                        .into(),
                )));
            }
            // The parsed manifest still matches the content, since the root is
            // unchanged; the operator half of the policy comes from the row
            // this closure just read under the lock.
            let policy = node.socket_policy(&declared, &current.activation);
            let max_streams = policy.max_streams;

            // The pool-wide bound, before the socket's own cap: one caller
            // who can reach many sockets must not be able to fill every
            // worker's queue past the documented daemon limit.
            if node.socket_pool_full() {
                return Ok(Err((
                    RefuseCode::Busy,
                    "the node's socket workers are at capacity; try again shortly".into(),
                )));
            }
            let id = node.next_socket_id();
            // The concurrency cap is taken at admission, before the guest
            // starts, so a caller cannot open idle streams past the cap.
            let Some(slot) = node.reserve_socket_slot(
                id,
                &qualified_for_slot,
                &peer_name,
                peer,
                checked_root,
                max_streams,
            ) else {
                return Ok(Err((
                    RefuseCode::Busy,
                    format!(
                        "{qualified_for_slot} is at its limit of {max_streams} concurrent \
                         invocations"
                    ),
                )));
            };
            Ok(Ok((policy, id, slot)))
        })
        .await
        .map_err(|e| (RefuseCode::NotActivated, e.to_string()))?;
        let (policy, id, slot) = prepared?;

        let peer_label = origin.canonical();
        Ok(Admission {
            program,
            program_root: resolved.root,
            socket: SocketId::new(&open.space, &open.path),
            peer: PeerIdentity {
                origin,
                device_key: peer,
                spaces: scope,
                addr,
                stream_index,
            },
            policy,
            meta: open.meta.clone(),
            self_origin: self.origin().clone(),
            host: Arc::new(TreeHost {
                node: self.clone(),
                own_origin: self.origin().clone(),
                socket: qualified,
                invocation: id,
                peer: peer_label,
            }),
            id,
            slot: Some(slot),
        })
    }

    /// Runs an admitted invocation.
    ///
    /// `peer_gone` is the caller's connection closing, as the net layer
    /// observed it: the invocation ends with `Deadline` when it fires — the
    /// same non-fault ending as a failed stream — while `synch socket kill`
    /// keeps its own channel and its `Killed` ending.
    pub(crate) async fn run_socket(
        &self,
        admission: Admission,
        stream: DuplexStream,
        peer_gone: tokio::sync::oneshot::Receiver<SockStatus>,
    ) -> SockStatus {
        let socket = admission.socket.clone();
        let program_root = admission.program_root;
        let Some(pool) = self.socket_workers() else {
            return SockStatus::Shutdown;
        };
        let status = match pool
            .run_cancellable(admission.with_stream(stream), peer_gone)
            .await
        {
            Ok(outcome) => outcome.status,
            Err(e) => {
                tracing::warn!("socket invocation failed: {e}");
                SockStatus::Fault(synch_core::FaultKind::Load)
            }
        };
        // The pool records every outcome against the socket's fault history.
        // Nothing is deactivated for it — activation is the operator's
        // statement about the path, not a judgement about these bytes — but a
        // program faulting for most of its callers is worth one loud line an
        // operator will find. Deploying a fixed object is the remedy, and it
        // clears the window.
        if pool.should_quarantine(&socket.qualified(), program_root) {
            tracing::error!(
                socket = socket.qualified(),
                program = %program_root,
                "socket program faulted on most of its recent invocations, from more than \
                 one caller; it stays activated — deploy a fixed program to this path"
            );
        }
        status
    }

    /// Every invocation running right now, optionally for one socket.
    pub fn socket_ps(&self, socket: Option<&str>) -> Vec<synch_sock::InvocationInfo> {
        match self.socket_workers() {
            Some(pool) => pool.snapshot(socket),
            None => Vec::new(),
        }
    }

    /// Ends one invocation, reporting whether there was one to end.
    pub fn socket_kill(&self, id: u64) -> bool {
        self.socket_workers().is_some_and(|pool| pool.kill(id))
    }

    /// What one socket's programs have written recently.
    pub fn socket_log(&self, space: &str, path: &str) -> Vec<synch_sock::LogLine> {
        match self.socket_workers() {
            Some(pool) => pool.logs(&format!("{space}/{path}")),
            None => Vec::new(),
        }
    }
}

/// The tree, as a running program reaches it.
///
/// Reads are scoped by default to this node's own view — the same scope the
/// program itself came from — and unrestricted (`docs/SOCKETS.md` §7.6).
/// Writes go through [`SocketHost::put_open`] and exist only behind an armed
/// tree-write declaration (`docs/TREE-WRITES.md`): the runtime checks the
/// grant before this host is asked, and this host re-takes the engine's own
/// durable gates — the declared-socket refusal, `.syncignore`, recovery —
/// at open and again at commit.
#[derive(Debug)]
struct TreeHost {
    node: Node,
    own_origin: OriginId,
    /// `<space>/<path>` of the socket being served, for the tree-write audit
    /// log.
    socket: String,
    /// The invocation id, likewise.
    invocation: u64,
    /// The caller's canonical origin, likewise.
    peer: String,
}

impl TreeHost {
    /// Splits `space/path`.
    fn split(path: &str) -> std::result::Result<(&str, &str), HostError> {
        path.split_once('/')
            .filter(|(space, rest)| !space.is_empty() && !rest.is_empty())
            .ok_or_else(|| HostError::NotReadable("a path is `<space>/<path>`".into()))
    }

    fn info(entry: &synch_store::EntryRow) -> std::result::Result<ObjectInfo, HostError> {
        // Every entry with content is readable, socket entries included. The
        // bytes are not secret — any member reads them out of the tree — and
        // refusing them here bought nothing while `sy_open_root` handed the
        // same bytes over by hash. What executes on this node is decided by the
        // arming table, not by who can read an ELF.
        let root = entry
            .content
            .ok_or_else(|| HostError::NotReadable("that path has no content".into()))?;
        Ok(ObjectInfo {
            root,
            size: entry.size,
            mtime_ns: entry.mtime_ns,
            mode: entry.unix_mode.unwrap_or(0),
            kind: match entry.kind {
                EntryKind::File => 0,
                EntryKind::Dir => 1,
                EntryKind::Symlink => 2,
                EntryKind::Tombstone => 3,
                EntryKind::Socket => 4,
            },
        })
    }
}

#[async_trait::async_trait]
impl SocketHost for TreeHost {
    fn open(&self, origin: Option<&str>, path: &str) -> std::result::Result<ObjectInfo, HostError> {
        let (space, rest) = TreeHost::split(path)?;
        let origin = match origin {
            None => self.own_origin.clone(),
            Some(text) => text
                .parse::<OriginId>()
                .map_err(|e| HostError::NotReadable(e.to_string()))?,
        };
        let entry = self
            .node
            .store()
            .entry(&origin, space, rest)
            .map_err(|e| HostError::Unavailable(e.to_string()))?
            .ok_or(HostError::NotFound)?;
        TreeHost::info(&entry)
    }

    fn open_root(&self, root: &Hash) -> std::result::Result<ObjectInfo, HostError> {
        let size = self
            .node
            .store()
            .blob(root)
            .map_err(|e| HostError::Unavailable(e.to_string()))?
            .ok_or(HostError::NotFound)?
            .size;
        Ok(ObjectInfo {
            root: *root,
            size,
            mtime_ns: 0,
            mode: 0,
            kind: 0,
        })
    }

    fn list_page(
        &self,
        prefix: &str,
        start_after: Option<&str>,
        limit: usize,
    ) -> std::result::Result<synch_sock::ListPage, HostError> {
        let (space, rest) = match prefix.split_once('/') {
            Some((space, rest)) => (space, rest),
            None => (prefix, ""),
        };
        let qualified = format!("{space}/");
        let start_after = start_after
            .and_then(|name| name.strip_prefix(&qualified))
            .or(start_after);
        let rows = self
            .node
            .store()
            .list_entries(
                Some(&self.own_origin),
                space,
                rest,
                start_after,
                Some(limit),
            )
            .map_err(|e| HostError::Unavailable(e.to_string()))?;
        let next = (rows.len() == limit)
            .then(|| rows.last().map(|row| format!("{space}/{}", row.path)))
            .flatten();
        let entries = rows
            .into_iter()
            .filter(|row| row.kind != EntryKind::Tombstone)
            .map(|row| format!("{space}/{}", row.path))
            .collect();
        Ok(synch_sock::ListPage { entries, next })
    }

    async fn pread(
        &self,
        root: Hash,
        offset: u64,
        len: u64,
    ) -> std::result::Result<Vec<u8>, HostError> {
        let size = self
            .node
            .store()
            .blob(&root)
            .map_err(|e| HostError::Unavailable(e.to_string()))?
            .ok_or(HostError::NotFound)?
            .size;
        if offset >= size {
            return Ok(Vec::new());
        }
        let end = (offset + len).min(size);
        // Fetches whatever is missing, verified per 16 KiB group like every
        // other read. This is the call that can wait on the network, which is
        // why the helper in front of it returns `SY_EAGAIN` and makes the
        // handle pollable rather than stalling the program.
        self.node
            .fetch_range(&root, size, offset, end)
            .await
            .map_err(|e| HostError::Unavailable(e.to_string()))?;
        self.node
            .cas_backend()
            .read_range(root, offset, end - offset)
            .await
            .map_err(|e| HostError::Unavailable(e.to_string()))
    }

    /// The kind of one resolved path, subtree-aware.
    ///
    /// The SFTP backend answers `STAT`/`READDIR` with it, so a directory has
    /// to be told apart from a path that does not exist — and the local
    /// scanner publishes no `Dir` rows, so a directory is a prefix with
    /// entries under it rather than a row of its own. A socket refuses like
    /// `open` refuses one: the kind says what would serve, not what the
    /// neighbour's code is.
    fn entry_kind(
        &self,
        origin: Option<&str>,
        path: &str,
    ) -> std::result::Result<synch_sock::HostEntryKind, HostError> {
        let (space, rest) = TreeHost::split(path)?;
        let origin = match origin {
            None => self.own_origin.clone(),
            Some(text) => text
                .parse::<OriginId>()
                .map_err(|e| HostError::NotReadable(e.to_string()))?,
        };
        let entry = self
            .node
            .store()
            .entry(&origin, space, rest)
            .map_err(|e| HostError::Unavailable(e.to_string()))?;
        match entry {
            Some(row) => {
                if row.kind == EntryKind::Socket {
                    return Err(HostError::NotReadable(
                        "that path is a socket; a socket does not read out its neighbours' code"
                            .into(),
                    ));
                }
                Ok(match row.kind {
                    EntryKind::File => synch_sock::HostEntryKind::File,
                    EntryKind::Dir => synch_sock::HostEntryKind::Directory,
                    EntryKind::Symlink => synch_sock::HostEntryKind::Symlink,
                    EntryKind::Tombstone => synch_sock::HostEntryKind::Tombstone,
                    EntryKind::Socket => synch_sock::HostEntryKind::Socket,
                })
            }
            // No row of its own: the path is a directory only if something is
            // published under it, and one row's existence check is enough.
            None => {
                let children = self
                    .node
                    .store()
                    .list_entries(Some(&origin), space, &format!("{rest}/"), None, Some(1))
                    .map_err(|e| HostError::Unavailable(e.to_string()))?;
                if children.is_empty() {
                    Err(HostError::NotFound)
                } else {
                    Ok(synch_sock::HostEntryKind::Directory)
                }
            }
        }
    }

    /// Opens a writer that will publish `space/path` as this node's own new
    /// version (`docs/TREE-WRITES.md` §6).
    ///
    /// The runtime has already matched the armed grant's prefix, modes and
    /// size bound; what is taken here — and re-taken inside the commit — are
    /// the engine's own gates, the same ones an S3 `PUT` goes through.
    fn put_open(
        &self,
        path: &str,
        modes: u32,
    ) -> std::result::Result<Box<dyn SocketWriter>, HostError> {
        let (space, rest) = TreeHost::split(path)?;
        let rest = crate::scanner::normalized_adoption_path(rest)
            .map_err(|e| HostError::Denied(e.to_string()))?;
        if self
            .node
            .store()
            .source(space)
            .map_err(|e| HostError::Io(e.to_string()))?
            .is_none()
        {
            return Err(HostError::NotFound);
        }
        refuse_socket_path(&self.node, space, &rest)?;
        self.node
            .ensure_adoptable(space, &rest)
            .map_err(write_refusal)?;
        Ok(Box::new(TreeWriter {
            node: self.node.clone(),
            space: space.to_string(),
            path: rest,
            modes,
            staged: None,
            staging_lost: false,
            unix_mode: None,
            mtime_ns: None,
            via: format!(
                "socket {} invocation {} peer {}",
                self.socket, self.invocation, self.peer
            ),
        }))
    }
}

/// An activated path is never writable through a program
/// (`docs/TREE-WRITES.md` §2).
///
/// This is the rule that keeps tree-write grants and activation composable:
/// without it, a socket whose manifest writes a prefix containing an
/// activated path is remote code persistence in two moves (write the ELF,
/// invoke it). With it, code reaches executability only over write channels
/// outside the socket runtime — channels the operator accepted as deployment
/// channels when activating the path.
fn refuse_socket_path(node: &Node, space: &str, path: &str) -> std::result::Result<(), HostError> {
    match node.store().is_activated_socket(space, path) {
        Ok(true) => Err(HostError::Denied(format!(
            "{space}/{path} is an activated socket, and sockets are never writable through a \
             program"
        ))),
        Ok(false) => Ok(()),
        Err(e) => Err(HostError::Io(e.to_string())),
    }
}

/// Maps an engine failure on the write path onto the guest's errno classes.
fn write_refusal(e: EngineError) -> HostError {
    match e {
        EngineError::NotFound(_) => HostError::NotFound,
        // A gate saying no — recovery, an ignore rule, an invalid path — is
        // policy, not breakage: the guest gets `SY_EPERM` and should stop
        // asking rather than retry.
        EngineError::InRecovery { .. } => HostError::Denied(e.to_string()),
        EngineError::Invalid(_) => HostError::Denied(e.to_string()),
        other => HostError::Io(other.to_string()),
    }
}

/// The create/replace condition, evaluated against this node's own live entry
/// under the tree-write lock, immediately before the staging lands
/// (`docs/TREE-WRITES.md` §5.3).
fn evaluate_put_condition(
    node: &Node,
    space: &str,
    path: &str,
    modes: u32,
    expected: PutCondition,
) -> std::result::Result<(), HostError> {
    // Re-taken inside the lock: a socket declaration may have arrived at this
    // path since the writer opened.
    refuse_socket_path(node, space, path)?;
    let entry = node
        .store()
        .entry(node.origin(), space, path)
        .map_err(|e| HostError::Io(e.to_string()))?;
    let live = entry.filter(|entry| entry.kind != EntryKind::Tombstone);
    let live_root = live.as_ref().and_then(|entry| entry.content);
    let create = modes & synch_core::TREE_WRITE_CREATE != 0;
    let replace = modes & synch_core::TREE_WRITE_REPLACE != 0;
    match expected {
        // The mode's own condition: `SY_EPERM`, because the answer will not
        // change until the tree does — unlike a lost expectation below, which
        // a re-read can repair.
        PutCondition::Any => {
            if live.is_some() && !replace {
                return Err(HostError::Denied(format!(
                    "{space}/{path} already has a live version here and the grant cannot replace"
                )));
            }
            if live.is_none() && !create {
                return Err(HostError::Denied(format!(
                    "{space}/{path} has no live version here and the grant cannot create"
                )));
            }
        }
        PutCondition::Absent => {
            if !create {
                return Err(HostError::Denied(format!(
                    "the grant for {space}/{path} carries no create mode"
                )));
            }
            if live.is_some() {
                return Err(HostError::Conflict(format!(
                    "{space}/{path} now has a live version here"
                )));
            }
        }
        PutCondition::Root(expected) => {
            if !replace {
                return Err(HostError::Denied(format!(
                    "the grant for {space}/{path} carries no replace mode"
                )));
            }
            match live_root {
                Some(root) if root == expected => {}
                _ => {
                    return Err(HostError::Conflict(format!(
                        "{space}/{path} no longer has the expected version here"
                    )))
                }
            }
        }
        // A condition on the unified tree's selection rather than on this
        // node's own entry (`docs/CLOUD-WRITES.md` §4.3). The mode gate is the
        // same as `Any`'s: what this node is about to do is create or replace
        // *its own* version, whichever version the caller read.
        PutCondition::Selected { from, root } => {
            if live.is_some() && !replace {
                return Err(HostError::Denied(format!(
                    "{space}/{path} already has a live version here and the grant cannot replace"
                )));
            }
            if live.is_none() && !create {
                return Err(HostError::Denied(format!(
                    "{space}/{path} has no live version here and the grant cannot create"
                )));
            }
            expect_selected(node, space, path, from, root)?;
        }
    }
    Ok(())
}

/// Whether the version the unified tree selects for a path has the expected
/// root — or, for `None`, whether the tree selects nothing at all.
///
/// Best-effort against the rest of the cluster in the way DESIGN §8 already
/// states: this node's view lags anti-entropy, so a version published
/// elsewhere a moment ago may not be in it yet. What is closed is the window
/// against other writers *on this node*, which is what the tree-write lock
/// around the caller is for.
fn expect_selected(
    node: &Node,
    space: &str,
    path: &str,
    from: Option<synch_core::OriginId>,
    expected: Option<Hash>,
) -> std::result::Result<(), HostError> {
    let policy = match from {
        Some(origin) => synch_store::VersionPolicy::Origin(origin),
        None => synch_store::VersionPolicy::Newest,
    };
    let found = match node.resolve(space, path, &policy) {
        Ok(row) if row.kind == EntryKind::Tombstone => None,
        Ok(row) => row.content,
        Err(EngineError::NotFound(_)) => None,
        // A divergent path under `strict` cannot happen here — the two
        // policies above always select — so anything else is a store fault.
        Err(e) => return Err(HostError::Io(e.to_string())),
    };
    if found == expected {
        return Ok(());
    }
    Err(HostError::Conflict(match (found, expected) {
        (Some(found), Some(_)) => {
            format!("{space}/{path} now selects {found}, not the expected version")
        }
        (Some(found), None) => format!("{space}/{path} now has a live version, {found}"),
        (None, _) => format!("{space}/{path} no longer has a live version"),
    }))
}

/// The delete counterpart to `evaluate_put_condition`. This is evaluated
/// while holding the same tree-write lock as the tombstone publication, so a
/// protocol adapter cannot delete a version that replaced the one it read.
fn evaluate_delete_condition(
    node: &Node,
    space: &str,
    path: &str,
    expected: PutCondition,
) -> std::result::Result<(), HostError> {
    refuse_socket_path(node, space, path)?;
    let entry = node
        .store()
        .entry(node.origin(), space, path)
        .map_err(|e| HostError::Io(e.to_string()))?;
    let live = entry.filter(|entry| entry.kind != EntryKind::Tombstone);
    let live_root = live.as_ref().and_then(|entry| entry.content);
    match expected {
        PutCondition::Any => Ok(()),
        PutCondition::Absent if live.is_none() => Ok(()),
        PutCondition::Absent => Err(HostError::Conflict(format!(
            "{space}/{path} now has a live version here"
        ))),
        PutCondition::Root(expected) if live_root == Some(expected) => Ok(()),
        PutCondition::Root(_) => Err(HostError::Conflict(format!(
            "{space}/{path} no longer has the expected version here"
        ))),
        PutCondition::Selected { from, root } => expect_selected(node, space, path, from, root),
    }
}

/// A single write into this node's own tree (`docs/TREE-WRITES.md` §6).
///
/// The engine's one tree-write seam, driven by three callers: a `sy_put_*`
/// writer handle in the socket runtime, the SFTP adapter's file handles, and
/// the cloud data plane's write tunnel (`docs/CLOUD-WRITES.md` §6.3), which
/// opens one through [`Node::open_tree_write`].
///
/// A re-composition of what the control-service `Put` handler does, gate for
/// gate: bytes stream into an [`Adoption`](crate::Adoption) beside the target
/// — or the daemon's scratch, for an API source — and a commit is the
/// adoption's rename plus the ordinary publish path (`scan_publish_push` for
/// a filesystem source, `commit_api_file` plus a flush for an API-source
/// one). Dropping it uncommitted drops the adoption, whose own `Drop` removes
/// the staging file.
pub struct TreeWriter {
    node: Node,
    space: String,
    path: String,
    /// The armed grant's `TREE_WRITE_*` bits, for the commit-time condition.
    modes: u32,
    staged: Option<crate::Adoption>,
    /// A mutation may have partially changed the staging before failing, or a
    /// commit may have consumed it before failing. In either case the bytes
    /// are no longer safe to publish or silently replace with empty staging,
    /// so every later operation refuses instead.
    staging_lost: bool,
    /// Host-originated metadata to carry into an API-source record.
    unix_mode: Option<u32>,
    mtime_ns: Option<i64>,
    /// Who is writing, for the log line a commit leaves: the socket, its
    /// invocation and the peer for a program; the relaying session for an
    /// embedder.
    via: String,
}

impl std::fmt::Debug for TreeWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TreeWriter")
            .field("space", &self.space)
            .field("path", &self.path)
            .field("via", &self.via)
            .finish_non_exhaustive()
    }
}

impl Node {
    /// Opens a write of `space/path` as this node's own new version, for an
    /// embedder (`docs/CLOUD-WRITES.md` §7 (e)).
    ///
    /// The same gates the socket runtime's writer takes at open — the space
    /// must be a source of this node's, the path normalizes, an activated
    /// socket path is refused, and the node must be able to publish — with
    /// every mode granted: the caller is the host, not a program under a
    /// manifest. `via` names the caller in the commit's log line.
    pub fn open_tree_write(&self, space: &str, path: &str, via: &str) -> Result<TreeWriter> {
        let rest = crate::scanner::normalized_adoption_path(path)?;
        if self.store().source(space)?.is_none() {
            return Err(EngineError::not_found(format!(
                "space {space} is not a source here"
            )));
        }
        refuse_socket_path(self, space, &rest).map_err(|e| EngineError::invalid(e.to_string()))?;
        self.ensure_adoptable(space, &rest)?;
        Ok(TreeWriter {
            node: self.clone(),
            space: space.to_string(),
            path: rest,
            modes: synch_core::TREE_WRITE_CREATE
                | synch_core::TREE_WRITE_REPLACE
                | synch_core::TREE_WRITE_DELETE,
            staged: None,
            staging_lost: false,
            unix_mode: None,
            mtime_ns: None,
            via: via.to_string(),
        })
    }
}

impl TreeWriter {
    fn ensure_usable(&self) -> std::result::Result<(), HostError> {
        if self.staging_lost {
            return Err(HostError::Io(
                "the staged bytes are unusable after a failed mutation or commit; open a new writer"
                    .into(),
            ));
        }
        Ok(())
    }

    /// Opens the adoption lazily, so a delete-only writer stages nothing.
    async fn ensure_staged(&mut self) -> std::result::Result<(), HostError> {
        self.ensure_usable()?;
        if self.staged.is_some() {
            return Ok(());
        }
        let node = self.node.clone();
        let (space, path) = (self.space.clone(), self.path.clone());
        let adoption = crate::blocking::offload(move || node.open_adoption(&space, &path))
            .await
            .map_err(write_refusal)?;
        self.staged = Some(adoption);
        Ok(())
    }
}

#[async_trait::async_trait]
impl SocketWriter for TreeWriter {
    async fn write(&mut self, data: Vec<u8>) -> std::result::Result<(), HostError> {
        self.ensure_usable()?;
        if data.is_empty() {
            return Ok(());
        }
        self.ensure_staged().await?;
        let mut adoption = self.staged.take().expect("just staged");
        let outcome = crate::blocking::offload(move || {
            let result = adoption.write(&data);
            Ok((adoption, result))
        })
        .await;
        match outcome {
            Ok((adoption, Ok(()))) => {
                self.staged = Some(adoption);
                Ok(())
            }
            Ok((adoption, Err(error))) => {
                drop(adoption);
                self.staging_lost = true;
                Err(write_refusal(error))
            }
            Err(e) => {
                self.staging_lost = true;
                Err(write_refusal(e))
            }
        }
    }

    async fn read_at(&mut self, offset: u64, len: u64) -> std::result::Result<Vec<u8>, HostError> {
        self.ensure_staged().await?;
        let mut adoption = self.staged.take().expect("just staged");
        let outcome = crate::blocking::offload(move || {
            let result = adoption.read_at(offset, len);
            Ok((adoption, result))
        })
        .await;
        match outcome {
            Ok((adoption, result)) => {
                self.staged = Some(adoption);
                result.map_err(write_refusal)
            }
            Err(e) => Err(write_refusal(e)),
        }
    }

    async fn write_at(&mut self, offset: u64, data: Vec<u8>) -> std::result::Result<(), HostError> {
        self.ensure_usable()?;
        if data.is_empty() {
            return Ok(());
        }
        self.ensure_staged().await?;
        let mut adoption = self.staged.take().expect("just staged");
        let outcome = crate::blocking::offload(move || {
            let result = adoption.write_at(offset, &data);
            Ok((adoption, result))
        })
        .await;
        match outcome {
            Ok((adoption, Ok(()))) => {
                self.staged = Some(adoption);
                Ok(())
            }
            Ok((adoption, Err(error))) => {
                drop(adoption);
                self.staging_lost = true;
                Err(write_refusal(error))
            }
            Err(e) => {
                self.staging_lost = true;
                Err(write_refusal(e))
            }
        }
    }

    async fn set_len(&mut self, len: u64) -> std::result::Result<(), HostError> {
        self.ensure_staged().await?;
        let mut adoption = self.staged.take().expect("just staged");
        let outcome = crate::blocking::offload(move || {
            let result = adoption.set_len(len);
            Ok((adoption, result))
        })
        .await;
        match outcome {
            Ok((adoption, Ok(()))) => {
                self.staged = Some(adoption);
                Ok(())
            }
            Ok((adoption, Err(error))) => {
                drop(adoption);
                self.staging_lost = true;
                Err(write_refusal(error))
            }
            Err(e) => {
                self.staging_lost = true;
                Err(write_refusal(e))
            }
        }
    }

    async fn set_metadata(
        &mut self,
        unix_mode: Option<u32>,
        mtime_ns: Option<i64>,
    ) -> std::result::Result<(), HostError> {
        self.ensure_usable()?;
        if unix_mode.is_none() && mtime_ns.is_none() {
            return Ok(());
        }
        self.ensure_staged().await?;
        let mut adoption = self.staged.take().expect("just staged");
        let outcome = crate::blocking::offload(move || {
            let result = adoption.set_metadata(unix_mode, mtime_ns);
            Ok((adoption, result))
        })
        .await;
        match outcome {
            Ok((adoption, Ok(()))) => {
                self.staged = Some(adoption);
                if unix_mode.is_some() {
                    self.unix_mode = unix_mode;
                }
                if mtime_ns.is_some() {
                    self.mtime_ns = mtime_ns;
                }
                Ok(())
            }
            Ok((adoption, Err(error))) => {
                drop(adoption);
                self.staging_lost = true;
                Err(write_refusal(error))
            }
            Err(error) => {
                self.staging_lost = true;
                Err(write_refusal(error))
            }
        }
    }

    async fn commit(
        &mut self,
        expected: PutCondition,
    ) -> std::result::Result<PutReceipt, HostError> {
        self.ensure_staged().await?;
        // One socket commit at a time (`docs/TREE-WRITES.md` §5.3): the
        // condition and the staging that follows it must not interleave with
        // another writer's commit of the same path. The scanner does not take
        // this lock — a concurrent local edit races a socket commit exactly
        // as it races an S3 `PUT`.
        let node = self.node.clone();
        let _guard = node.tree_write_lock().lock().await;
        let check = self.node.clone();
        let (space, path, modes) = (self.space.clone(), self.path.clone(), self.modes);
        let (condition, api_source) = crate::blocking::offload(move || {
            check.ensure_publishable()?;
            let api_source = check.is_api_source(&space)?;
            Ok((
                evaluate_put_condition(&check, &space, &path, modes, expected),
                api_source,
            ))
        })
        .await
        .map_err(write_refusal)?;
        // A refused condition leaves the staging in place: a lost expectation
        // is retryable once the guest has read the tree again.
        condition?;

        let adoption = self.staged.take().expect("just staged");
        // From here down the staging is consumed: should any step below
        // fail, a retry must not quietly re-stage zero bytes.
        self.staging_lost = true;
        let (unix_mode, mtime_ns) = (self.unix_mode, self.mtime_ns);
        let (root, size) = if api_source {
            let scratch = crate::blocking::offload(move || adoption.commit())
                .await
                .map_err(write_refusal)?;
            let committed = self
                .node
                .commit_api_file_with_metadata(
                    &self.space,
                    &self.path,
                    &scratch,
                    mtime_ns.unwrap_or_else(synch_core::now_ns),
                    unix_mode,
                )
                .await;
            let cleanup = scratch.clone();
            let _ = crate::blocking::offload(move || {
                let _ = std::fs::remove_file(&cleanup);
                Ok(())
            })
            .await;
            let (root, size) = committed.map_err(write_refusal)?;
            self.node.flush_staged().await.map_err(write_refusal)?;
            (root, size)
        } else {
            let (root, size) = crate::blocking::offload(move || {
                let mut adoption = adoption;
                let size = adoption.written();
                let root = adoption.hash_staged()?;
                adoption.commit()?;
                Ok((root, size))
            })
            .await
            .map_err(write_refusal)?;
            self.node.scan_publish_push().await.map_err(write_refusal)?;
            (root, size)
        };
        tracing::info!(
            via = %self.via,
            path = format!("{}/{}", self.space, self.path),
            root = %root,
            size,
            "published a tree write"
        );
        Ok(PutReceipt { root, size })
    }

    async fn delete(&mut self) -> std::result::Result<(), HostError> {
        self.delete_if(PutCondition::Any).await
    }

    async fn delete_if(&mut self, expected: PutCondition) -> std::result::Result<(), HostError> {
        self.ensure_usable()?;
        // Checked by the runtime against the grant already; re-taken here so
        // an embedder's host cannot be talked past it.
        if self.modes & synch_core::TREE_WRITE_DELETE == 0 {
            return Err(HostError::Denied(format!(
                "the grant for {}/{} carries no delete mode",
                self.space, self.path
            )));
        }
        let node = self.node.clone();
        let _guard = node.tree_write_lock().lock().await;
        let check = self.node.clone();
        let (space, path) = (self.space.clone(), self.path.clone());
        crate::blocking::offload(move || {
            check.ensure_publishable()?;
            Ok(evaluate_delete_condition(&check, &space, &path, expected))
        })
        .await
        .map_err(write_refusal)??;
        let deleted = self
            .node
            .delete_object(&self.space, &self.path)
            .await
            .map_err(write_refusal)?;
        tracing::info!(
            via = %self.via,
            path = format!("{}/{}", self.space, self.path),
            still_published = deleted.still_published,
            "published a tree delete"
        );
        Ok(())
    }
}

/// The engine's implementation of the network layer's service.
///
/// Filled in after the node exists, because the endpoint is bound *by* the
/// node's constructor and the ALPN has to be mounted on it: the handler
/// therefore has to exist before the thing it dispatches to. A `OnceLock`
/// rather than a lock, because this is written once during startup and read on
/// every connection thereafter.
#[derive(Debug, Clone, Default)]
pub(crate) struct SocketDispatch {
    node: Arc<std::sync::OnceLock<crate::node::WeakNode>>,
}

impl SocketDispatch {
    /// An unbound dispatcher, ready to be mounted.
    pub(crate) fn new() -> Self {
        SocketDispatch::default()
    }

    /// Binds it to the node it serves. Called once, at the end of startup.
    pub(crate) fn bind(&self, node: &Node) {
        let _ = self.node.set(node.downgrade());
    }

    /// The node, while it is open.
    fn node(&self) -> Option<Node> {
        self.node.get().and_then(|weak| weak.upgrade())
    }
}

#[async_trait::async_trait]
impl synch_net::sock::SocketService for SocketDispatch {
    async fn admit(
        &self,
        peer: NodeId,
        addr: String,
        stream_index: u64,
        open: &SockOpen,
    ) -> std::result::Result<Admission, (RefuseCode, String)> {
        // The window between the endpoint accepting connections and the node
        // finishing startup is real, if short. `Busy` rather than a harder
        // code because it is exactly what it says: try again.
        let Some(node) = self.node() else {
            return Err((RefuseCode::Busy, "this node is still starting".into()));
        };
        node.admit_socket(peer, addr, stream_index, open).await
    }

    async fn run(
        &self,
        admission: Admission,
        stream: DuplexStream,
        peer_gone: tokio::sync::oneshot::Receiver<SockStatus>,
    ) -> SockStatus {
        match self.node() {
            // A node that has gone away between admission and here is a node
            // shutting down, which is exactly what the caller should be told.
            Some(node) => node.run_socket(admission, stream, peer_gone).await,
            None => SockStatus::Shutdown,
        }
    }
}

/// The limits every socket on this node runs under.
pub(crate) fn default_limits() -> Limits {
    Limits::default()
}

/// The worker pool, where there is one.
///
/// Two implementations of the same small API rather than `cfg` at every call
/// site: async-ebpf exists on Linux, macOS and OpenBSD on x86-64 and arm64 and
/// nowhere else, and the engine has to build everywhere. What a node without
/// it loses is serving — declaring, publishing, replicating and materializing
/// socket entries all work, and so does `synch socket connect`, because the
/// connecting side executes nothing.
#[cfg(all(
    any(target_os = "linux", target_os = "macos", target_os = "openbsd"),
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod pool {
    use super::*;

    /// A started pool.
    #[derive(Debug, Clone)]
    pub(crate) struct SocketPool(synch_sock::WorkerHandle);

    impl SocketPool {
        /// Starts workers with the node's persistent SSH host key.
        pub(crate) fn start_with_ssh_host_key(
            workers: usize,
            limits: Limits,
            host_key: synch_sock::SshHostKey,
        ) -> Option<SocketPool> {
            Some(SocketPool(
                synch_sock::WorkerHandle::start_with_ssh_host_key(workers, limits, host_key),
            ))
        }

        /// The limits every invocation runs under.
        pub(crate) fn limits(&self) -> Limits {
            self.0.limits().clone()
        }

        /// The next invocation id.
        pub(crate) fn next_id(&self) -> u64 {
            self.0.next_id()
        }

        /// Drops what one socket's map held.
        pub(crate) fn clear_map(&self, socket: &str) {
            self.0.clear_map(socket);
        }

        /// Runs one invocation that may be cut short by `synch socket kill`
        /// or by the caller's connection closing (`peer_gone`).
        ///
        /// The kill sender is attached to the registry here, exactly as
        /// [`SocketPool::run`] does; the peer-gone receiver is the
        /// connection-close signal the net layer watched.
        pub(crate) async fn run_cancellable(
            &self,
            invocation: synch_sock::Invocation,
            peer_gone: tokio::sync::oneshot::Receiver<synch_core::SockStatus>,
        ) -> std::result::Result<synch_sock::Outcome, synch_sock::SockError> {
            let (kill_tx, kill_rx) = tokio::sync::oneshot::channel();
            if invocation.slot.is_some() {
                self.0.registry().attach_cancel(invocation.id, kill_tx);
            }
            self.0.run_cancellable(invocation, kill_rx, peer_gone).await
        }

        /// Whether every worker is at its queued cap.
        pub(crate) fn full(&self) -> bool {
            self.0.full()
        }

        /// Cancels and drains every invocation, then joins all worker threads.
        pub(crate) async fn shutdown(&self) {
            self.0.shutdown().await;
        }

        /// Takes a concurrency slot, or reports the socket full.
        #[allow(
            clippy::too_many_arguments,
            reason = "a pass-through to `Registry::reserve`, whose arguments are                       the facts an entry is made of"
        )]
        pub(crate) fn reserve(
            &self,
            id: u64,
            socket: &str,
            peer: &str,
            peer_key: synch_core::NodeId,
            program: Hash,
            max_streams: usize,
        ) -> Option<synch_sock::SlotGuard> {
            self.0.registry().reserve(
                id,
                socket,
                peer,
                peer_key,
                program,
                max_streams,
                std::time::Instant::now(),
            )
        }

        /// Whether a socket has been faulting enough to be disarmed.
        pub(crate) fn should_quarantine(&self, socket: &str, program: Hash) -> bool {
            // The pool recorded the outcome as the invocation ended; this only
            // reads the verdict it reached.
            self.0.registry().take_quarantine(socket, program)
        }

        /// Everything running.
        pub(crate) fn snapshot(&self, socket: Option<&str>) -> Vec<synch_sock::InvocationInfo> {
            self.0
                .registry()
                .snapshot(socket, std::time::Instant::now())
        }

        /// Ends one invocation.
        pub(crate) fn kill(&self, id: u64) -> bool {
            self.0.registry().kill(id)
        }

        /// One socket's recent log lines.
        pub(crate) fn logs(&self, socket: &str) -> Vec<synch_sock::LogLine> {
            self.0.registry().logs(socket)
        }
    }
}

#[cfg(not(all(
    any(target_os = "linux", target_os = "macos", target_os = "openbsd"),
    any(target_arch = "x86_64", target_arch = "aarch64")
)))]
mod pool {
    use super::*;

    /// A pool that does not exist on this platform.
    #[derive(Debug, Clone)]
    pub(crate) struct SocketPool;

    impl SocketPool {
        /// Never starts: there is no runtime to start.
        pub(crate) fn start(_workers: usize, _limits: Limits) -> Option<SocketPool> {
            None
        }

        /// The documented defaults, so `synch socket ls` prints the same
        /// numbers everywhere even where nothing can run.
        pub(crate) fn limits(&self) -> Limits {
            Limits::default()
        }

        /// Unreachable: nothing admits an invocation without a pool.
        pub(crate) fn next_id(&self) -> u64 {
            0
        }

        /// Nothing to clear.
        pub(crate) fn clear_map(&self, _socket: &str) {}

        /// Unreachable: nothing admits an invocation without a pool.
        pub(crate) async fn run_cancellable(
            &self,
            _invocation: synch_sock::Invocation,
            _peer_gone: tokio::sync::oneshot::Receiver<synch_core::SockStatus>,
        ) -> std::result::Result<synch_sock::Outcome, synch_sock::SockError> {
            Err(synch_sock::SockError::Unsupported)
        }

        /// Nothing runs on an unsupported platform.
        pub(crate) async fn shutdown(&self) {}

        /// Nothing is ever at capacity when nothing can run: admission is
        /// refused before it reaches this.
        pub(crate) fn full(&self) -> bool {
            false
        }

        /// Unreachable: admission refuses before it reaches this.
        #[allow(
            clippy::too_many_arguments,
            reason = "matches the shape of the implementation it stands in for"
        )]
        pub(crate) fn reserve(
            &self,
            _id: u64,
            _socket: &str,
            _peer: &str,
            _peer_key: synch_core::NodeId,
            _program: Hash,
            _max_streams: usize,
        ) -> Option<synch_sock::SlotGuard> {
            None
        }

        /// Nothing runs, so nothing faults.
        pub(crate) fn should_quarantine(&self, _socket: &str, _program: Hash) -> bool {
            false
        }

        /// Nothing runs here.
        pub(crate) fn snapshot(&self, _socket: Option<&str>) -> Vec<synch_sock::InvocationInfo> {
            Vec::new()
        }

        /// Nothing runs here.
        pub(crate) fn kill(&self, _id: u64) -> bool {
            false
        }

        /// Nothing runs here.
        pub(crate) fn logs(&self, _socket: &str) -> Vec<synch_sock::LogLine> {
            Vec::new()
        }
    }
}

pub(crate) use pool::SocketPool;

/// How many socket programs the node keeps in memory after their
/// invocations end.
///
/// A bound on the cache's strong entries: each holds up to
/// `max_program_bytes` (4 MiB), so the whole cache is at most
/// `MAX_CACHED_PROGRAMS * 4 MiB`. Oldest first, like the workers' compiled-
/// program cache.
const MAX_CACHED_PROGRAMS: usize = 4;

/// One root's program bytes, or why the load failed.
type ProgramBytes = std::result::Result<Arc<Vec<u8>>, String>;

/// The watch channel an in-flight load publishes its outcome on.
type LoadWatch = tokio::sync::watch::Sender<Option<ProgramBytes>>;

/// What a claim on [`ProgramBytesCache`] for one root came back as.
pub(crate) enum ProgramLoad {
    /// The bytes were already loaded; share them.
    Ready(Arc<Vec<u8>>),
    /// Another admission is reading this root from the CAS; wait for it.
    InFlight(tokio::sync::watch::Receiver<Option<ProgramBytes>>),
    /// This admission is the one that reads the CAS.
    Loader(LoadGuard),
}

/// The loader's claim on [`ProgramBytesCache`] for one root.
///
/// The claim is released when the load ends, *however* it ends. The happy
/// path calls [`LoadGuard::finish`], which publishes the outcome and
/// remembers the bytes. A load that is cancelled or panics during the CAS
/// await drops the guard instead: the drop removes the root from the
/// in-flight table and publishes the failure to every waiter, so the root's
/// later admissions start a fresh load rather than waiting forever on a
/// sender nobody will ever fill.
#[derive(Debug)]
pub(crate) struct LoadGuard {
    cache: Arc<ProgramBytesCache>,
    root: Hash,
    sender: LoadWatch,
    finished: bool,
}

impl LoadGuard {
    /// Publishes the outcome and releases the loader role.
    pub(crate) fn finish(mut self, outcome: ProgramBytes) {
        self.finished = true;
        self.cache
            .finish_load(self.root, self.sender.clone(), outcome);
    }
}

impl Drop for LoadGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let mut inner = self.cache.inner.lock().expect("socket program cache");
        inner.loading.remove(&self.root);
        // Wake the waiters with the failure rather than leaving them parked
        // on a sender that will never send.
        let _ = self
            .sender
            .send(Some(Err("socket program load was interrupted".to_string())));
    }
}

/// Socket program bytes, shared across the admissions of one content root.
///
/// Sixty-four streams into one socket must not mean sixty-four copies of
/// its program, and the sharing has to survive *concurrent* cold admissions
/// — the first admission of a root is still reading the CAS while the
/// fifty-ninth arrives. Two structures make it one copy per root:
///
/// * `loading` coalesces those concurrent cold admissions: the first claims
///   the loader role and publishes the outcome on a watch channel; the rest
///   subscribe and share the allocation.
/// * `ready` holds completed programs strongly, FIFO-evicted at
///   [`MAX_CACHED_PROGRAMS`], so a burst of admissions re-reads nothing and
///   memory stays bounded now that the entries are not weak.
#[derive(Debug, Default)]
pub(crate) struct ProgramBytesCache {
    inner: std::sync::Mutex<ProgramBytesInner>,
}

#[derive(Debug, Default)]
struct ProgramBytesInner {
    ready: std::collections::HashMap<Hash, Arc<Vec<u8>>>,
    order: std::collections::VecDeque<Hash>,
    loading: std::collections::HashMap<Hash, LoadWatch>,
}

impl ProgramBytesCache {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(ProgramBytesCache::default())
    }

    /// Claims the cache for one root: the loaded bytes, a seat on an
    /// in-flight load, or the loader role. The lock is held only for the
    /// claim — the CAS read happens outside it, which is what lets the other
    /// admissions wait on the watch channel instead of on the cache.
    pub(crate) fn begin_load(self: &Arc<Self>, root: &Hash) -> ProgramLoad {
        let mut inner = self.inner.lock().expect("socket program cache");
        if let Some(bytes) = inner.ready.get(root) {
            return ProgramLoad::Ready(bytes.clone());
        }
        if let Some(sender) = inner.loading.get(root) {
            return ProgramLoad::InFlight(sender.subscribe());
        }
        let (sender, _receiver) = tokio::sync::watch::channel(None);
        inner.loading.insert(*root, sender.clone());
        ProgramLoad::Loader(LoadGuard {
            cache: self.clone(),
            root: *root,
            sender,
            finished: false,
        })
    }

    /// Publishes the outcome of a load to its waiters, and remembers the
    /// bytes for the next admission. Called by [`LoadGuard::finish`].
    fn finish_load(&self, root: Hash, sender: LoadWatch, outcome: ProgramBytes) {
        let mut inner = self.inner.lock().expect("socket program cache");
        inner.loading.remove(&root);
        if let Ok(bytes) = &outcome {
            inner.ready.insert(root, bytes.clone());
            inner.order.push_back(root);
            while inner.order.len() > MAX_CACHED_PROGRAMS {
                if let Some(oldest) = inner.order.pop_front() {
                    inner.ready.remove(&oldest);
                }
            }
        }
        let _ = sender.send(Some(outcome));
    }
}

impl Node {
    /// Notes a deployment: the scanner republished an activated socket path
    /// with new content (`docs/SOCKETS.md` §3).
    ///
    /// Nothing about the activation changes — that is the model — but the
    /// per-socket map does not survive the program it was minted by: a session
    /// table the old bytes built is not state the new bytes agreed to inherit.
    /// Invocations already running keep the snapshot they were admitted with;
    /// the next admission serves the new root.
    pub(crate) fn socket_content_deployed(&self, space: &str, path: &str, root: &Hash) {
        self.clear_socket_map(&format!("{space}/{path}"));
        tracing::info!(
            socket = format!("{space}/{path}"),
            root = %root,
            "socket content deployed; new connections serve the new program"
        );
    }

    /// Reads an object out of the local CAS synchronously.
    ///
    /// The scanner already runs off the runtime, which is why this can be a
    /// blocking read rather than the async CAS path the serving side uses.
    fn read_socket_bytes(&self, root: &Hash) -> Result<Vec<u8>> {
        let size = self
            .store()
            .blob(root)
            .map_err(EngineError::from)?
            .ok_or_else(|| EngineError::invalid(format!("no local bytes for {root}")))?
            .size;
        let limits = self.socket_limits();
        if size > limits.max_program_bytes {
            return Err(EngineError::invalid(format!(
                "the program is {size} bytes, past the {} a socket may be",
                limits.max_program_bytes
            )));
        }
        Ok(self.store().read_range(root, 0, size)?)
    }
}

impl Node {
    /// Opens a socket on a node (`docs/SOCKETS.md` §4), including this one.
    ///
    /// The connecting side of the design, and it executes nothing: it names a
    /// path, and everything that decides what runs is state the callee already
    /// holds. That is why this half works on platforms where the runtime does
    /// not exist at all.
    ///
    /// One QUIC connection per call rather than a reused session: a socket
    /// stream's lifetime is the caller's, and sharing a connection between two
    /// unrelated `synch socket connect` invocations would let one close the other's.
    pub async fn connect_socket(
        &self,
        origin: &OriginId,
        space: &str,
        path: &str,
        meta: Vec<(String, String)>,
    ) -> Result<SocketConnection> {
        let open = SockOpen::new(origin.clone(), space, path, meta);
        open.validate()
            .map_err(|e| EngineError::invalid(e.to_string()))?;

        if origin == self.origin() {
            let admission = self
                .admit_socket(self.node_id(), "local".into(), 0, &open)
                .await
                .map_err(|(code, message)| {
                    EngineError::invalid(format!(
                        "{} refused {space}/{path}: {}: {message}",
                        origin.canonical(),
                        code.as_str()
                    ))
                })?;
            let program = admission.program_root;
            let invocation = admission.id;
            let (caller, guest) = tokio::io::duplex(self.socket_limits().ring_bytes);
            let node = self.clone();
            // A real peer-gone signal, not a dropped sender. The runtime reads
            // a dropped sender as "this caller never leaves" (`run_job` maps it
            // to `pending`), so the local path used to pin a concurrency slot
            // for the daemon's lifetime whenever a caller went away while the
            // program still had an upstream making progress. `LocalStream`
            // fires it when the caller's half is dropped.
            let (gone_tx, peer_gone) = tokio::sync::oneshot::channel();
            let completion = tokio::spawn(async move {
                node.run_socket(admission, DuplexStream::from_split(guest), peer_gone)
                    .await
            });
            return Ok(SocketConnection::Local {
                program,
                invocation,
                stream: LocalStream {
                    inner: caller,
                    gone: Some(gone_tx),
                },
                completion,
            });
        }

        let node = self.clone();
        let remote = origin.clone();
        let keys = crate::blocking::offload(move || {
            Ok(node
                .store()
                .keys_for_origin(&remote, synch_core::now_ns())?)
        })
        .await?;
        if keys.is_empty() {
            return Err(EngineError::not_found(format!(
                "no live binding for {} — this node does not know its device key",
                origin.canonical()
            )));
        }

        let mut last: Option<EngineError> = None;
        for key in keys {
            // `peers_seen.last_addr` is only a hint. A live binding with no
            // stored address is the normal shape for a peer reached through
            // iroh/Pkarr discovery, so give iroh the authenticated key and let
            // its configured address lookup resolve it.
            let addr = self
                .peer_addr_off_runtime(&key)
                .await?
                .unwrap_or_else(|| iroh::EndpointAddr::new(key));
            let client = match self.net().connect_sock(addr).await {
                Ok(client) => client,
                Err(e) => {
                    last = Some(EngineError::Net(e));
                    continue;
                }
            };
            // Accepted before the first `Open`, because the callee opens it at
            // connection setup and a status has to have somewhere to arrive.
            let control = client.control().await.map_err(EngineError::Net)?;
            return match client.open(&open).await.map_err(EngineError::Net)? {
                Ok(stream) => Ok(SocketConnection::Remote {
                    client,
                    control,
                    stream,
                }),
                Err(refused) => Err(EngineError::invalid(format!(
                    "{} refused {space}/{path}: {refused}",
                    origin.canonical()
                ))),
            };
        }
        Err(last.unwrap_or_else(|| {
            EngineError::not_found(format!("could not reach {}", origin.canonical()))
        }))
    }
}

/// The caller's half of a local invocation, which reports its own departure.
///
/// A remote invocation learns that its caller is gone from
/// `Connection::closed`, and must: an invocation that keeps running is a slot
/// held for nobody (`synch_net::sock::SockRunner::run`). In memory there is no
/// connection to watch, and dropping a `tokio::io::duplex` half is only a clean
/// EOF — which the runtime treats as an ordinary half-close, because for a
/// proxy that is exactly what it is. So the drop itself is the signal.
///
/// The guard lives inside the stream rather than beside it in
/// [`SocketConnection::Local`] because callers destructure that variant and
/// split the stream; a sibling field would fire the moment the variant came
/// apart, while the stream was still in use. Held here, it fires when both
/// split halves are gone and not before.
#[derive(Debug)]
pub struct LocalStream {
    inner: tokio::io::DuplexStream,
    gone: Option<tokio::sync::oneshot::Sender<SockStatus>>,
}

impl Drop for LocalStream {
    fn drop(&mut self) {
        if let Some(gone) = self.gone.take() {
            // `Deadline`, matching the remote path: a non-fault ending that
            // says the caller left, not that the program did anything wrong.
            let _ = gone.send(SockStatus::Deadline);
        }
    }
}

impl tokio::io::AsyncRead for LocalStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for LocalStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// A live socket connection on the caller's side.
#[derive(Debug)]
pub enum SocketConnection {
    /// A connection to a peer over `sync/sock/1`.
    Remote {
        /// Kept alive: dropping it closes the QUIC connection under the stream.
        client: synch_net::sock::SockClient,
        /// Where the invocation's exit status arrives.
        control: iroh::endpoint::RecvStream,
        /// The invocation itself.
        stream: synch_net::sock::SockStream,
    },
    /// A connection to this node, carried in memory but admitted and run by the
    /// same engine path as a remote invocation.
    Local {
        /// The content root actually running.
        program: Hash,
        /// The callee's invocation id.
        invocation: u64,
        /// Opaque bytes in both directions. Dropping it ends the invocation.
        stream: LocalStream,
        /// The invocation's eventual status.
        completion: tokio::task::JoinHandle<SockStatus>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::node;
    use synch_core::{record::ChunkParams, FileEntry, RECORD_VERSION};

    #[tokio::test]
    async fn concurrent_cold_loads_coalesce_onto_one_allocation() {
        let cache = ProgramBytesCache::new();
        let root = Hash::new(b"program");

        // The first claim is the loader.
        let ProgramLoad::Loader(guard) = cache.begin_load(&root) else {
            panic!("the first claim should be the loader");
        };
        // A concurrent admission of the same root waits on the watch channel
        // instead of reading the CAS itself.
        let ProgramLoad::InFlight(receiver) = cache.begin_load(&root) else {
            panic!("a concurrent claim should be in flight, not a fresh load");
        };
        let waiter = tokio::spawn(async move {
            let mut receiver = receiver;
            receiver.wait_for(Option::is_some).await.unwrap();
            let outcome = receiver.borrow().clone();
            outcome.unwrap().unwrap()
        });

        // The loader publishes one allocation...
        let bytes = Arc::new(b"the program".to_vec());
        guard.finish(Ok(bytes.clone()));
        // ...which the waiter shares, pointer and all.
        let seen = waiter.await.unwrap();
        assert!(
            Arc::ptr_eq(&seen, &bytes),
            "the waiting admission allocated its own copy"
        );

        // And the next admission is served from the ready cache.
        let ProgramLoad::Ready(bytes) = cache.begin_load(&root) else {
            panic!("a loaded root should be ready");
        };
        assert_eq!(&*bytes, b"the program");
    }

    #[tokio::test]
    async fn an_interrupted_load_wakes_its_waiters_and_frees_the_root() {
        let cache = ProgramBytesCache::new();
        let root = Hash::new(b"program");

        let ProgramLoad::Loader(guard) = cache.begin_load(&root) else {
            panic!("the first claim should be the loader");
        };
        let ProgramLoad::InFlight(receiver) = cache.begin_load(&root) else {
            panic!("a concurrent claim should be in flight");
        };
        let waiter = tokio::spawn(async move {
            let mut receiver = receiver;
            let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                receiver.wait_for(Option::is_some).await.unwrap();
                receiver.borrow().clone()
            })
            .await
            .expect("the interrupted load never woke its waiter");
            outcome
        });

        // The loader dies — cancellation, a panic — without finishing.
        drop(guard);

        // The waiter is told the load failed, not left parked forever...
        let outcome = waiter.await.unwrap();
        assert!(
            matches!(outcome, Some(Err(_))),
            "waiters should see the failure, got {outcome:?}"
        );
        // ...and the root is free for a fresh load.
        assert!(
            matches!(cache.begin_load(&root), ProgramLoad::Loader(_)),
            "an interrupted load left its in-flight entry behind"
        );
    }

    #[test]
    fn the_ready_cache_is_fifo_bounded() {
        let cache = ProgramBytesCache::new();
        for i in 0..MAX_CACHED_PROGRAMS + 4 {
            let root = Hash::new(format!("program {i}").as_bytes());
            let ProgramLoad::Loader(guard) = cache.begin_load(&root) else {
                panic!("a fresh root should claim the loader role");
            };
            guard.finish(Ok(Arc::new(vec![i as u8])));
        }
        // The oldest roots are gone; the newest are served from memory.
        let oldest = Hash::new(b"program 0");
        assert!(
            !matches!(cache.begin_load(&oldest), ProgramLoad::Ready(_)),
            "the oldest program was not evicted"
        );
        let newest = Hash::new(format!("program {}", MAX_CACHED_PROGRAMS + 3).as_bytes());
        assert!(matches!(cache.begin_load(&newest), ProgramLoad::Ready(_)));
    }

    #[tokio::test]
    async fn entry_kind_classifies_rows_dirs_and_missing_paths() {
        let (_data, node) = node().await;
        let origin = node.origin().clone();
        let store = node.store();
        store
            .put_entry(
                &origin,
                "media",
                "guide.md",
                &FileEntry::file(3, 0, Hash::new(b"guide"), 1),
            )
            .unwrap();
        store
            .put_entry(
                &origin,
                "media",
                "docs/inner.md",
                &FileEntry::file(3, 0, Hash::new(b"inner"), 2),
            )
            .unwrap();
        store
            .put_entry(
                &origin,
                "media",
                "old.txt",
                &FileEntry::tombstone(0, 3, None),
            )
            .unwrap();
        store
            .put_entry(
                &origin,
                "media",
                "git.sock",
                &FileEntry::socket(3, 0, Hash::new(b"sock"), 4),
            )
            .unwrap();
        store
            .put_entry(
                &origin,
                "media",
                "docs",
                &FileEntry {
                    v: RECORD_VERSION,
                    kind: EntryKind::Dir,
                    size: 0,
                    mtime_ns: 0,
                    unix_mode: None,
                    content: None,
                    chunking: ChunkParams::DEFAULT,
                    seq: 5,
                    prev: None,
                    symlink_target: None,
                },
            )
            .unwrap();
        let host = TreeHost {
            node: node.clone(),
            own_origin: origin.clone(),
            socket: "media/test.sock".into(),
            invocation: 0,
            peer: "test".into(),
        };

        assert_eq!(
            host.entry_kind(None, "media/guide.md").unwrap(),
            synch_sock::HostEntryKind::File
        );
        assert_eq!(
            host.entry_kind(None, "media/docs").unwrap(),
            synch_sock::HostEntryKind::Directory
        );
        assert_eq!(
            host.entry_kind(None, "media/old.txt").unwrap(),
            synch_sock::HostEntryKind::Tombstone
        );
        // A socket refuses like open() refuses one.
        assert!(matches!(
            host.entry_kind(None, "media/git.sock"),
            Err(HostError::NotReadable(_))
        ));

        // The local scanner publishes no Dir rows, so once the row is gone
        // the path is still a directory as long as entries exist under it...
        store.delete_entry(&origin, "media", "docs").unwrap();
        assert_eq!(
            host.entry_kind(None, "media/docs").unwrap(),
            synch_sock::HostEntryKind::Directory
        );
        // ...and a path with neither a row nor anything under it is not found.
        assert!(matches!(
            host.entry_kind(None, "media/missing"),
            Err(HostError::NotFound)
        ));

        // The origin resolves exactly as open() resolves it.
        assert!(matches!(
            host.entry_kind(Some("not an origin"), "media/guide.md"),
            Err(HostError::NotReadable(_))
        ));
        let other = OriginId::named("other", "x.example").unwrap();
        assert!(matches!(
            host.entry_kind(Some(&other.canonical()), "media/guide.md"),
            Err(HostError::NotFound)
        ));
        // And a malformed path never reaches the store.
        assert!(matches!(
            host.entry_kind(None, "not a space path"),
            Err(HostError::NotReadable(_))
        ));
    }
}
