//! The batching publisher (§7.1).
//!
//! Staged changes are accumulated and turned into a *single* new trie root:
//! bump `seq`, sign, store, `HeadPush`. A burst of editor saves therefore costs
//! one head rather than one head per save, and a 100k-file initial index costs
//! a handful.
//!
//! A batch flushes when any trigger fires, whichever comes first:
//!
//! - `publish_quiesce` (default 2 s) passes with nothing new staged, or
//! - twice that interval passes since the batch's first change, or
//! - the buffer reaches `publish_batch_max` (default 1000) entries.
//!
//! Callers that must publish before they return — `synch source scan`, `synch adopt path` —
//! flush explicitly instead of waiting for either.

use std::{sync::Mutex, time::Duration};

use synch_core::SignedHead;
use tokio::sync::Notify;

use crate::{
    error::Result,
    node::{Node, StagedChange},
};

/// How long staging must go quiet before a batch is published (§7.1).
pub(crate) const DEFAULT_PUBLISH_QUIESCE: Duration = Duration::from_secs(2);

/// How many staged entries force a batch out without waiting (§7.1).
pub(crate) const DEFAULT_PUBLISH_BATCH_MAX: usize = 1000;

/// The buffer between staging and one signed root.
///
/// Held by the [`Node`]; the timer that drains it is [`Node::run_publisher`],
/// and any caller can drain it by hand with [`Node::flush_staged`].
#[derive(Debug)]
pub struct Publisher {
    quiesce: Duration,
    batch_max: usize,
    staged: Mutex<Batch>,
    wake: Notify,
}

#[derive(Debug, Default)]
struct Batch {
    changes: Vec<StagedChange>,
    since: Option<tokio::time::Instant>,
}

impl Publisher {
    /// Builds a publisher with the given batch triggers.
    pub fn new(quiesce: Duration, batch_max: usize) -> Publisher {
        Publisher {
            quiesce,
            batch_max,
            staged: Mutex::new(Batch::default()),
            wake: Notify::new(),
        }
    }

    /// How long staging must go quiet before the batch is published.
    pub fn quiesce(&self) -> Duration {
        self.quiesce
    }

    /// Adds changes to the current batch.
    ///
    /// Cheap and non-blocking: it appends and wakes the timer, and never
    /// touches the database.
    pub fn stage(&self, changes: impl IntoIterator<Item = StagedChange>) {
        let added = {
            let mut staged = self.buffer();
            let before = staged.changes.len();
            staged.changes.extend(changes);
            let added = staged.changes.len() - before;
            if before == 0 && added > 0 {
                staged.since = Some(tokio::time::Instant::now());
            }
            added
        };
        if added > 0 {
            // A stored permit means a waiter that has not started waiting yet
            // still sees this, so a change staged in the gap is never lost.
            self.wake.notify_one();
        }
    }

    /// The keys currently waiting to be published.
    ///
    /// So a producer of *removals* can avoid contradicting a live entry already
    /// in the batch: `publish` folds in order and the last value for a key wins,
    /// which for a removal against a fresh entry means the path is erased.
    pub(crate) fn staged_keys(&self) -> std::collections::HashSet<Vec<u8>> {
        self.buffer()
            .changes
            .iter()
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// How many changes are waiting to be published.
    pub fn pending(&self) -> usize {
        self.buffer().changes.len()
    }

    fn deadline(&self) -> Option<tokio::time::Instant> {
        self.buffer().since.map(|since| {
            since
                .checked_add(self.quiesce.saturating_mul(2))
                .unwrap_or(since)
        })
    }

    /// Whether the batch has reached `publish_batch_max`.
    pub fn is_full(&self) -> bool {
        self.pending() >= self.batch_max
    }

    /// Resolves once something has been staged since the last wake.
    pub(crate) async fn woken(&self) {
        self.wake.notified().await
    }

    /// Takes the whole batch, leaving the buffer empty.
    pub(crate) fn take(&self) -> Vec<StagedChange> {
        let mut staged = self.buffer();
        staged.since = None;
        std::mem::take(&mut staged.changes)
    }

    /// Puts a batch back at the front after a publish failed, so the next
    /// flush retries it rather than dropping it on the floor.
    pub(crate) fn restage(&self, changes: Vec<StagedChange>) {
        {
            let mut staged = self.buffer();
            let queued = std::mem::replace(&mut staged.changes, changes);
            staged.changes.extend(queued);
            if !staged.changes.is_empty() && staged.since.is_none() {
                staged.since = Some(tokio::time::Instant::now());
            }
        }
        // And wake the loop, or "it stays staged" means "until something else
        // happens to be staged". The permit that drove the failed flush was
        // consumed by it, so the loop parks on `woken()` with a full buffer —
        // and the scan that produced the batch has already written its
        // `local_files` rows, so every later scan calls those paths unchanged.
        // The node's own tree then lags until an unrelated change arrives or the
        // daemon stops.
        self.wake.notify_one();
    }

    fn buffer(&self) -> std::sync::MutexGuard<'_, Batch> {
        self.staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Node {
    /// Adds changes to the batch the publisher is accumulating (§7.1).
    ///
    /// Returns immediately: what turns a batch into a root is either the
    /// quiesce timer in [`Node::run_publisher`] or an explicit
    /// [`Node::flush_staged`].
    pub fn stage(&self, changes: impl IntoIterator<Item = StagedChange>) {
        self.publisher().stage(changes);
    }

    /// Publishes everything staged so far as one new signed root and pushes it
    /// to reachable peers (§7.1).
    ///
    /// This is the whole batch, not one caller's share of it: a `synch source scan`
    /// that lands while a watcher-triggered rescan is still buffered publishes
    /// both, which is the point of batching.
    ///
    /// A failed publish puts the batch back rather than dropping it, so the
    /// next flush retries it. A failed *push* does not fail the flush: the head
    /// is published, and peers pick it up at the next anti-entropy round.
    pub async fn flush_staged(&self) -> Result<Option<SignedHead>> {
        let head = self.publish_staged().await?;
        if let Some(head) = &head {
            if let Err(e) = self.push_head(head).await {
                tracing::debug!(error = %e, "could not push the new head");
            }
            // This node's own origin's tree just moved, and both checkouts
            // and the replicas follow the unified tree — which
            // includes it.
            self.checkout_wake().notify_one();
            self.replica_wake().notify_one();
        }
        Ok(head)
    }

    /// Publishes everything staged so far as one new signed root, without
    /// telling anybody about it.
    ///
    /// The half of a flush that is this node's own business. Peers learn the
    /// head from the push in [`Node::flush_staged`], or from the next
    /// anti-entropy round if nobody pushes it.
    async fn publish_staged(&self) -> Result<Option<SignedHead>> {
        let batch = self.publisher().take();
        if batch.is_empty() {
            return Ok(None);
        }
        // On the blocking pool: a publish inserts every staged change into the
        // trie, signs a head, re-materializes the changed leaves and fsyncs the
        // lot as one SQLite transaction — up to `publish_batch_max` entries of
        // it (§7.1, §10).
        //
        // The restage happens *inside* the closure, not around the await. A
        // blocking task cannot be cancelled, so once it starts it always either
        // publishes the batch or puts it back — while anything written around
        // the await would be skipped entirely if this future were dropped
        // first. It can be: control connections are spawned detached
        // (`control::Server::run`), so a `daemon stop` landing mid-flush drops
        // one wherever it happens to be parked, and this is the one await point
        // where that would otherwise mean a batch taken out of the buffer and
        // never put back.
        let node = self.clone();
        let head = crate::blocking::offload(move || match node.publish(&batch) {
            Ok(head) => Ok(head),
            Err(e) => {
                node.publisher().restage(batch);
                Err(e)
            }
        })
        .await?;
        Ok(head)
    }

    /// Runs the batching publisher until `shutdown` resolves (§7.1).
    ///
    /// Flush on quiescence, maximum batch age or a full batch,
    /// and one last flush on the way out so a clean stop never strands a
    /// buffered batch.
    pub async fn run_publisher(&self, shutdown: impl std::future::Future<Output = ()>) {
        let mut shutdown = std::pin::pin!(shutdown);
        loop {
            tokio::select! {
                _ = &mut shutdown => return self.flush_on_stop().await,
                _ = self.publisher().woken() => {}
            }
            // New changes restart quiescence, but never the batch's age.
            // Continuous low-volume writes must also become visible.
            while !self.publisher().is_full() {
                let Some(deadline) = self.publisher().deadline() else {
                    break;
                };
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
                tokio::select! {
                    _ = &mut shutdown => return self.flush_on_stop().await,
                    _ = tokio::time::sleep_until(deadline) => break,
                    _ = self.publisher().woken() => continue,
                    _ = tokio::time::sleep(self.publisher().quiesce()) => break,
                }
            }
            if let Err(e) = self.flush_staged().await {
                tracing::warn!(error = %e, "publishing a batch failed; it stays staged");
            }
        }
    }

    /// Publishes whatever is still buffered, on the way out (§7.1).
    ///
    /// Published but not pushed. The batch must not be stranded — that is what
    /// this is for — but telling peers means dialing them, and a peer that
    /// accepts and then says nothing would hold the whole daemon open for its
    /// request deadline while an operator waits on `synch daemon stop`. The
    /// head is durable either way, and peers pick it up at the next
    /// anti-entropy round.
    async fn flush_on_stop(&self) {
        if self.publisher().pending() == 0 {
            return;
        }
        if let Err(e) = self.publish_staged().await {
            tracing::warn!(error = %e, "the last batch could not be published");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(key: &str) -> StagedChange {
        (key.as_bytes().to_vec(), Some(b"v".to_vec()))
    }

    #[test]
    fn staging_accumulates_until_taken() {
        let publisher = Publisher::new(Duration::from_secs(2), 3);
        assert_eq!(publisher.pending(), 0);
        publisher.stage([change("a")]);
        publisher.stage([change("b"), change("c")]);
        assert_eq!(publisher.pending(), 3);
        assert!(publisher.is_full());

        let batch = publisher.take();
        assert_eq!(batch.len(), 3);
        assert_eq!(publisher.pending(), 0);
        assert!(!publisher.is_full());

        // A failed batch goes back ahead of anything staged meanwhile.
        publisher.stage([change("d")]);
        publisher.restage(batch);
        let retried = publisher.take();
        assert_eq!(retried.len(), 4);
        assert_eq!(retried[0].0, b"a".to_vec());
        assert_eq!(retried[3].0, b"d".to_vec());
    }

    use crate::testkit::{eventually, node_with};

    /// A node whose batch triggers are set for a test rather than for a desk.
    async fn node(quiesce: Duration, batch_max: usize) -> (tempfile::TempDir, Node) {
        let (dir, node) = node_with(move |config| {
            config.publish_quiesce = quiesce;
            config.publish_batch_max = batch_max;
        })
        .await;
        node.store()
            .put_source("s", synch_store::SourceKind::Api, None)
            .unwrap();
        (dir, node)
    }

    fn entry(node: &Node, path: &str) -> StagedChange {
        let root = node.store().ingest_bytes(path.as_bytes(), 0).unwrap();
        let entry = synch_core::FileEntry::file(path.len() as u64, 0, root, 1);
        (
            node.key_for("s", path).unwrap(),
            Some(postcard::to_stdvec(&entry).unwrap()),
        )
    }

    /// Runs the publisher loop until the returned sender fires.
    fn run(
        node: &Node,
    ) -> (
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let runner = node.clone();
        let handle = tokio::spawn(async move {
            runner
                .run_publisher(async {
                    let _ = rx.await;
                })
                .await
        });
        (tx, handle)
    }

    async fn stop(tx: tokio::sync::oneshot::Sender<()>, handle: tokio::task::JoinHandle<()>) {
        let _ = tx.send(());
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("the loop must stop promptly")
            .unwrap();
    }

    /// The property the whole design rests on: many staged changes, one head.
    #[tokio::test]
    async fn a_batch_becomes_exactly_one_head() {
        let (_d, node) = node(Duration::from_secs(60), 1000).await;
        node.stage([entry(&node, "a.txt")]);
        node.stage([entry(&node, "b.txt"), entry(&node, "c.txt")]);
        assert_eq!(node.publisher().pending(), 3);

        let head = node.flush_staged().await.unwrap().unwrap();
        assert_eq!(head.seq, 1, "one batch is one seq, not three");
        assert_eq!(node.publisher().pending(), 0);
        assert_eq!(
            node.store()
                .list_entries(Some(node.origin()), "s", "", None, None)
                .unwrap()
                .len(),
            3
        );
        // And a flush with nothing staged mints nothing.
        assert!(node.flush_staged().await.unwrap().is_none());
        node.shutdown().await.unwrap();
    }

    /// The loop publishes on its own, with no explicit flush: a quiet batch
    /// goes once the quiesce window passes, and a full batch does not wait
    /// for it at all.
    #[tokio::test]
    async fn staging_flushes_on_quiesce_or_a_full_batch() {
        // Quiesce trigger.
        let (_d, n1) = node(Duration::from_millis(50), 1000).await;
        let (tx, handle) = run(&n1);
        n1.stage([entry(&n1, "a.txt")]);
        let published = eventually(|| n1.own_head().unwrap().is_some()).await;
        assert!(published, "a quiet batch must publish itself");
        assert_eq!(n1.own_head().unwrap().unwrap().seq, 1);
        stop(tx, handle).await;
        n1.shutdown().await.unwrap();

        // Batch trigger: a quiesce far longer than the test could tolerate,
        // so the size trigger is what published.
        let (_d, node) = node(Duration::from_secs(600), 2).await;
        let (tx, handle) = run(&node);
        node.stage([entry(&node, "a.txt"), entry(&node, "b.txt")]);
        let published = eventually(|| node.own_head().unwrap().is_some()).await;
        assert!(published, "a full batch must not wait out the quiesce");
        stop(tx, handle).await;
        node.shutdown().await.unwrap();
    }

    /// Stopping the loop publishes what is still buffered rather than
    /// stranding it.
    #[tokio::test]
    async fn a_clean_stop_flushes_the_last_batch() {
        let (_d, node) = node(Duration::from_secs(600), 1000).await;
        let (tx, handle) = run(&node);
        node.stage([entry(&node, "a.txt")]);
        stop(tx, handle).await;

        assert_eq!(node.publisher().pending(), 0);
        assert_eq!(node.own_head().unwrap().unwrap().seq, 1);
        node.shutdown().await.unwrap();
    }

    /// A publish that fails keeps its batch: the changes are retried, not lost.
    #[tokio::test]
    async fn a_refused_publish_keeps_the_batch_staged() {
        let (_d, node) = node(Duration::from_secs(600), 1000).await;
        // A peer advertising a head for our own origin puts the node in
        // key-loss recovery, where publishing is refused (§3.4).
        node.store()
            .record_observed_head(
                node.origin(),
                42,
                &synch_core::Hash([7u8; 32]),
                true,
                None,
                synch_core::now_ns(),
            )
            .unwrap();

        node.stage([entry(&node, "a.txt")]);
        assert!(node.flush_staged().await.is_err());
        assert_eq!(
            node.publisher().pending(),
            1,
            "the refused batch stays staged for the next attempt"
        );
        node.shutdown().await.unwrap();
    }
}
