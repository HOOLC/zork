//! Name-status comparison between two origins' published trees (§8).
//!
//! `synch compare` answers "which files differ" between a baseline origin and a
//! target origin, without fetching a single content byte. Both origins' trees
//! are already materialized locally — this node replicates every trusted origin
//! (§2) — so the comparison is a set difference over the `entries` view, scoped
//! to one space, and works for local-vs-remote and remote-vs-remote alike.
//!
//! A path's *version identity* is its content root (a symlink's is its target),
//! §8, so "modified" means the bytes differ: two entries that share a content
//! root are identical here even when their mtime or publish seq does not match.

use std::collections::BTreeMap;

use synch_core::{EntryKind, Hash, OriginId};
use synch_store::EntryRow;

use crate::{
    error::{EngineError, Result},
    node::Node,
};

/// How a path differs between the baseline (`from`) and the target (`to`).
///
/// Stated relative to the baseline: `Created` means the target has a file the
/// baseline does not, `Deleted` means the baseline has one the target does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareStatus {
    /// Present in `to`, absent (or tombstoned) in `from`.
    Created,
    /// Present in both as files, with different content.
    Modified,
    /// Present in `from`, absent (or tombstoned) in `to`.
    Deleted,
}

impl CompareStatus {
    /// The single-letter marker used in `git status --short` style output.
    pub fn marker(self) -> char {
        match self {
            CompareStatus::Created => 'A',
            CompareStatus::Modified => 'M',
            CompareStatus::Deleted => 'D',
        }
    }
}

/// One differing path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompareChange {
    /// The path within the space.
    pub path: String,
    /// How it differs.
    pub status: CompareStatus,
}

/// The result of comparing two origins over one space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompareReport {
    /// The baseline origin.
    pub from: OriginId,
    /// The target origin.
    pub to: OriginId,
    /// The space compared.
    pub space: String,
    /// The differing paths, in lexicographic order.
    pub changes: Vec<CompareChange>,
}

impl CompareReport {
    /// How many paths were created in the target.
    pub fn created(&self) -> usize {
        self.count(CompareStatus::Created)
    }

    /// How many paths were modified.
    pub fn modified(&self) -> usize {
        self.count(CompareStatus::Modified)
    }

    /// How many paths were deleted in the target.
    pub fn deleted(&self) -> usize {
        self.count(CompareStatus::Deleted)
    }

    fn count(&self, status: CompareStatus) -> usize {
        self.changes.iter().filter(|c| c.status == status).count()
    }
}

/// The content identity of a file-like entry: what makes two versions "the same
/// file". A regular file is its content root; a symlink is its target (§8). A
/// directory marker and a tombstone are not file-like and never enter the map.
///
/// A socket is its content root *and* the fact that it is a socket
/// (`docs/SOCKETS.md` §11). Folding it into `File` would make one origin's
/// socket and another's plain file over the same ELF the same version, and they
/// are not: one of those origins will execute those bytes for a peer that
/// connects, and the other will hand them over as a file. Two origins agree
/// about a socket only if they agree that it *is* one.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Identity {
    File(Hash),
    Socket(Hash),
    Symlink(String),
}

fn identity(row: &EntryRow) -> Option<Identity> {
    match row.kind {
        EntryKind::File => row.content.map(Identity::File),
        EntryKind::Socket => row.content.map(Identity::Socket),
        EntryKind::Symlink => row.symlink_target.clone().map(Identity::Symlink),
        EntryKind::Dir | EntryKind::Tombstone => None,
    }
}

fn live_map(rows: Vec<EntryRow>) -> BTreeMap<String, Identity> {
    rows.into_iter()
        .filter_map(|row| identity(&row).map(|id| (row.path, id)))
        .collect()
}

impl Node {
    /// Compares the published trees of two origins over one space, returning the
    /// created/modified/deleted paths under `prefix` (empty for the whole
    /// space). Reads only local metadata — no content is fetched.
    pub fn compare(
        &self,
        space: &str,
        prefix: &str,
        from: &OriginId,
        to: &OriginId,
    ) -> Result<CompareReport> {
        // A typo would otherwise read as "the whole space was created/deleted".
        // The local origin is always valid; any other must be an origin this
        // node has actually synced entries for.
        let known = self.store().entry_origins()?;
        let is_known = |o: &OriginId| o == self.origin() || known.iter().any(|k| k == o);
        for origin in [from, to] {
            if !is_known(origin) {
                return Err(EngineError::not_found(format!(
                    "no synced tree for origin {origin}; is it a trusted member that has published?"
                )));
            }
        }

        let from_map =
            live_map(
                self.store()
                    .list_entries(Some(from), space, prefix, None, None)?,
            );
        let to_map = live_map(
            self.store()
                .list_entries(Some(to), space, prefix, None, None)?,
        );

        let mut changes = Vec::new();
        // A BTreeMap iterates in sorted key order, so merging the two sorted
        // key streams yields changes already in lexicographic path order.
        let mut fi = from_map.iter().peekable();
        let mut ti = to_map.iter().peekable();
        loop {
            match (fi.peek(), ti.peek()) {
                (Some((fp, fid)), Some((tp, tid))) => {
                    use std::cmp::Ordering::*;
                    match fp.cmp(tp) {
                        Less => {
                            changes.push(deleted(fi.next().unwrap().0));
                        }
                        Greater => {
                            changes.push(created(ti.next().unwrap().0));
                        }
                        Equal => {
                            if fid != tid {
                                changes.push(modified(fp));
                            }
                            fi.next();
                            ti.next();
                        }
                    }
                }
                (Some(_), None) => changes.push(deleted(fi.next().unwrap().0)),
                (None, Some(_)) => changes.push(created(ti.next().unwrap().0)),
                (None, None) => break,
            }
        }

        Ok(CompareReport {
            from: from.clone(),
            to: to.clone(),
            space: space.to_string(),
            changes,
        })
    }
}

fn created(path: &str) -> CompareChange {
    CompareChange {
        path: path.to_string(),
        status: CompareStatus::Created,
    }
}

fn modified(path: &str) -> CompareChange {
    CompareChange {
        path: path.to_string(),
        status: CompareStatus::Modified,
    }
}

fn deleted(path: &str) -> CompareChange {
    CompareChange {
        path: path.to_string(),
        status: CompareStatus::Deleted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::node;
    use synch_core::{now_ns, FileEntry, OriginId};

    fn a() -> OriginId {
        OriginId::named("nas", "x.example").unwrap()
    }

    fn b() -> OriginId {
        OriginId::named("laptop", "x.example").unwrap()
    }

    fn c() -> OriginId {
        OriginId::named("other", "x.example").unwrap()
    }

    fn put_file(node: &Node, origin: &OriginId, path: &str, content: &[u8], mtime: i64) {
        let root = node.store().ingest_bytes(content, now_ns()).unwrap();
        node.store()
            .put_entry(
                origin,
                "media",
                path,
                &FileEntry::file(content.len() as u64, mtime, root, 1),
            )
            .unwrap();
    }

    fn statuses(report: &CompareReport) -> Vec<(&str, CompareStatus)> {
        report
            .changes
            .iter()
            .map(|c| (c.path.as_str(), c.status))
            .collect()
    }

    #[tokio::test]
    async fn reports_created_modified_and_deleted() {
        let (_d, node) = node().await;
        // Shared, identical content — with different mtimes, so content
        // identity, not mtime, is what defines modified (§8).
        put_file(&node, &a(), "keep.txt", b"same", 1);
        put_file(&node, &b(), "keep.txt", b"same", 999);
        // Only in the baseline, only in the target, or in both with different
        // bytes: each shape lands in exactly one status.
        put_file(&node, &a(), "only_a.txt", b"x", 1);
        put_file(&node, &b(), "only_b.txt", b"y", 1);
        put_file(&node, &a(), "changed.txt", b"v1", 1);
        put_file(&node, &b(), "changed.txt", b"v2", 1);
        // A tombstone in the target reads as deleted.
        put_file(&node, &a(), "gone.txt", b"here", 1);
        node.store()
            .put_entry(&b(), "media", "gone.txt", &FileEntry::tombstone(2, 1, None))
            .unwrap();

        let report = node.compare("media", "", &a(), &b()).unwrap();
        assert_eq!(
            statuses(&report),
            vec![
                ("changed.txt", CompareStatus::Modified),
                ("gone.txt", CompareStatus::Deleted),
                ("only_a.txt", CompareStatus::Deleted),
                ("only_b.txt", CompareStatus::Created),
            ]
        );

        // A prefix scopes the same comparison: only paths under it appear.
        put_file(&node, &a(), "photos/x.jpg", b"1", 1);
        put_file(&node, &b(), "photos/x.jpg", b"2", 1);
        put_file(&node, &b(), "docs/y.txt", b"3", 1);
        let report = node.compare("media", "photos/", &a(), &b()).unwrap();
        assert_eq!(
            statuses(&report),
            vec![("photos/x.jpg", CompareStatus::Modified)]
        );

        // An origin this node has never synced is a typo, not an empty tree.
        assert!(node.compare("media", "", &a(), &c()).is_err());
        node.shutdown().await.unwrap();
    }
}
