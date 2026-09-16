//! The filesystem watcher (§7.1).
//!
//! Hints from `notify` are debounced and only ever *schedule rescans* —
//! correctness never depends on watcher completeness, so a missed event costs
//! at most one scan interval of latency, never a lost file.

use std::{collections::HashSet, path::PathBuf, time::Duration};

use notify::{
    event::{AccessKind, AccessMode},
    EventKind, RecommendedWatcher, RecursiveMode, Watcher as _,
};
use tokio::sync::mpsc;

use crate::{error::Result, node::Node};

/// Whether an event could have changed what a scan would find.
///
/// Reading is not changing, and the distinction is not a nicety here: a scan
/// opens and reads every filesystem-source directory and every file it has to hash, and
/// inotify reports those opens, reads, and closes as events in their own
/// right. Hinting on them makes the watcher chase its own tail — hint,
/// rescan, read, hint — one full rescan of every space per debounce window,
/// forever, on a tree nobody has touched. That is a large fraction of a core
/// for an idle daemon to spend, and for an idle *peer* to spend with it,
/// because each of those rescans goes on to stage, publish, and push.
///
/// So access events are dropped, with one exception: a close-after-write is
/// how an editor's final write shows up on some platforms. Everything else —
/// create, modify, remove, and anything the backend could not classify —
/// still hints, and the scan itself decides whether anything really moved.
fn changes_something(event: &notify::Event) -> bool {
    match event.kind {
        EventKind::Access(access) => matches!(access, AccessKind::Close(AccessMode::Write)),
        _ => true,
    }
}

/// A debounced rescan trigger over every configured space.
#[derive(Debug)]
pub(crate) struct SpaceWatcher {
    watcher: RecommendedWatcher,
    hints: mpsc::Receiver<()>,
    debounce: Duration,
    /// The space roots currently registered with `notify`, so a re-register
    /// pass can tell what is new and what has gone.
    watching: HashSet<PathBuf>,
}

impl SpaceWatcher {
    /// Starts watching a configured set of space roots the caller has read.
    pub(crate) fn start_with(node: &Node, configured: &HashSet<PathBuf>) -> Result<SpaceWatcher> {
        let (tx, rx) = mpsc::channel(64);
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if event.is_ok_and(|event| changes_something(&event)) {
                // A full channel already means a rescan is pending, so dropping
                // the hint loses nothing.
                let _ = tx.try_send(());
            }
        })
        .map_err(watch_error)?;

        let mut out = SpaceWatcher {
            watcher,
            hints: rx,
            debounce: node.config().watch_debounce,
            watching: HashSet::new(),
        };
        out.resync_to(configured);
        Ok(out)
    }

    /// The space roots the store currently names.
    ///
    /// Split out from [`SpaceWatcher::resync_to`] so the standing loop can
    /// read it on the blocking pool: this is a `spaces` query and the loop
    /// runs on a runtime worker (§10).
    pub(crate) fn configured_spaces(node: &Node) -> Result<HashSet<PathBuf>> {
        Ok(node
            .store()
            .sources()?
            .into_iter()
            .filter_map(|space| space.local_path.map(PathBuf::from))
            .collect())
    }

    /// Registers spaces added since the last pass and drops ones removed,
    /// given a configured set that has already been read.
    ///
    /// A daemon runs for weeks and `synch source add` lands whenever an
    /// operator says so, so the watched set cannot be fixed at startup: an
    /// unregistered space would be covered only by the hourly rescan, and a
    /// removed one would keep waking the watcher for a directory nobody
    /// indexes. Failing to watch a root is not fatal — the periodic scan is
    /// the guarantee (§7.1).
    pub(crate) fn resync_to(&mut self, configured: &HashSet<PathBuf>) -> usize {
        let mut changed = 0;
        for path in configured.difference(&self.watching.clone()) {
            match self.watcher.watch(path, RecursiveMode::Recursive) {
                Ok(()) => {
                    self.watching.insert(path.clone());
                    changed += 1;
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "cannot watch space; relying on periodic scans");
                }
            }
        }
        for path in self.watching.clone().difference(configured) {
            let _ = self.watcher.unwatch(path);
            self.watching.remove(path);
            changed += 1;
        }
        changed
    }

    /// Waits for at least one hint, then swallows further hints for the
    /// debounce window so a burst of writes costs one rescan.
    ///
    /// Returns `false` once the watcher has shut down.
    pub(crate) async fn next_rescan(&mut self) -> bool {
        if self.hints.recv().await.is_none() {
            return false;
        }
        let deadline = tokio::time::Instant::now() + self.debounce;
        loop {
            match tokio::time::timeout_at(deadline, self.hints.recv()).await {
                Ok(Some(())) => continue,
                Ok(None) => return false,
                Err(_) => return true,
            }
        }
    }
}

fn watch_error(e: notify::Error) -> crate::error::EngineError {
    crate::error::EngineError::Io(std::io::Error::other(e.to_string()))
}

impl Node {
    /// Watches every space and rescans on change until `shutdown` resolves.
    ///
    /// A rescan *stages*; what publishes is
    /// [`run_publisher`](Node::run_publisher), which every host running this
    /// loop is expected to run beside it.
    pub async fn run_watcher(&self, shutdown: impl std::future::Future<Output = ()>) {
        // The configured set is a store read and this loop runs on a runtime
        // worker, so every read of it goes over to the blocking pool (§10).
        let spaces = |node: Node| async move {
            crate::blocking::offload(move || SpaceWatcher::configured_spaces(&node)).await
        };
        let configured = match spaces(self.clone()).await {
            Ok(configured) => configured,
            Err(e) => {
                tracing::warn!(error = %e, "cannot read the configured spaces; watcher not started");
                return;
            }
        };
        let mut watcher = match SpaceWatcher::start_with(self, &configured) {
            Ok(watcher) => watcher,
            Err(e) => {
                tracing::warn!(error = %e, "watcher unavailable; relying on periodic scans");
                return;
            }
        };
        let mut shutdown = std::pin::pin!(shutdown);
        let spaces_changed = self.spaces_changed_signal();
        loop {
            tokio::select! {
                _ = &mut shutdown => return,
                // `source add` / `source rm` ring this so a new root is watched
                // at once rather than at the next filesystem hint.
                _ = spaces_changed.notified() => {
                    match spaces(self.clone()).await {
                        Ok(configured) => { watcher.resync_to(&configured); }
                        Err(e) => tracing::warn!(error = %e, "re-registering spaces failed"),
                    }
                }
                triggered = watcher.next_rescan() => {
                    if !triggered {
                        return;
                    }
                    // Every pass re-registers, so a space that appeared while
                    // the daemon ran is watched from here on even if the nudge
                    // was missed.
                    match spaces(self.clone()).await {
                        Ok(configured) => { watcher.resync_to(&configured); }
                        Err(e) => tracing::warn!(error = %e, "re-registering spaces failed"),
                    }
                    // Staged, not published: a burst of editor saves is one
                    // batch and therefore one head (§7.1). Off the runtime,
                    // because the rescan walks every space and re-hashes
                    // whatever moved — unbounded work that must not sit on a
                    // worker thread the endpoint needs (§10).
                    if let Err(e) = self.scan_and_stage_async().await {
                        tracing::warn!(error = %e, "rescan failed");
                    }
                }
            }
        }
    }

    /// Runs the periodic full scan until `shutdown` resolves (§7.1).
    ///
    /// Like the watcher, this stages into the publisher rather than publishing
    /// on its own.
    pub async fn run_scanner(&self, shutdown: impl std::future::Future<Output = ()>) {
        let mut shutdown = std::pin::pin!(shutdown);
        loop {
            tokio::select! {
                _ = &mut shutdown => return,
                _ = tokio::time::sleep(self.config().scan_interval) => {
                    if let Err(e) = self.scan_and_stage_async().await {
                        tracing::warn!(error = %e, "periodic scan failed");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::node;
    use notify::event::{CreateKind, DataChange, ModifyKind, RemoveKind};

    fn event(kind: EventKind) -> notify::Event {
        notify::Event::new(kind)
    }

    /// The watcher must not hint on its own reading: a scan opens and reads
    /// every file it hashes, and inotify reports each as an event — hinting
    /// would cost a full rescan per debounce window, forever, on an idle tree.
    #[test]
    fn reads_are_not_change_hints_but_writes_are() {
        // One literal per classifier arm: every non-`Close(Write)` access
        // variant is read-shaped, and a scan's own reads must not rescan.
        for read in [
            EventKind::Access(AccessKind::Read),
            EventKind::Access(AccessKind::Close(AccessMode::Read)),
        ] {
            assert!(!changes_something(&event(read)), "{read:?}");
        }
        for change in [
            EventKind::Create(CreateKind::File),
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            EventKind::Remove(RemoveKind::File),
            // How an editor's finished write shows up where the backend
            // reports it as an access.
            EventKind::Access(AccessKind::Close(AccessMode::Write)),
            // Unclassifiable: hint, and let the scan decide.
            EventKind::Any,
        ] {
            assert!(changes_something(&event(change)), "{change:?}");
        }
    }

    #[tokio::test]
    async fn spaces_added_and_removed_while_running_are_re_registered() {
        // A daemon runs for weeks; `source add` lands whenever an operator says
        // so. A watcher fixed at startup would leave the new root covered only
        // by the hourly rescan (§7.1).
        let (_d, node) = node().await;
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        node.add_filesystem_source("one", first.path()).unwrap();

        let resync = |watcher: &mut SpaceWatcher, node: &Node| {
            watcher.resync_to(&SpaceWatcher::configured_spaces(node).unwrap())
        };
        let configured = SpaceWatcher::configured_spaces(&node).unwrap();
        let mut watcher = SpaceWatcher::start_with(&node, &configured).unwrap();
        assert_eq!(watcher.watching.len(), 1);

        node.add_filesystem_source("two", second.path()).unwrap();
        assert_eq!(resync(&mut watcher, &node), 1);
        assert_eq!(watcher.watching.len(), 2);
        // Re-registering an unchanged set is a no-op.
        assert_eq!(resync(&mut watcher, &node), 0);

        node.finish_source_removal("two").unwrap();
        assert_eq!(resync(&mut watcher, &node), 1);
        assert_eq!(watcher.watching.len(), 1);
        node.shutdown().await.unwrap();
    }
}
