//! Bounded, restart-safe background compression for sealed event segments.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::store::SessionStore;

#[derive(Clone)]
pub struct SegmentCompressor {
    inner: Arc<Inner>,
}

struct Inner {
    wake: zork_notify::Notifier,
    stop: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl SegmentCompressor {
    pub fn start(store: Arc<dyn SessionStore>, retry_interval: Duration) -> Self {
        let wake = zork_notify::Notifier::default();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(run(
            store,
            wake.subscribe(),
            stopped,
            retry_interval.max(Duration::from_millis(1)),
        ));
        Self {
            inner: Arc::new(Inner {
                wake,
                stop: Mutex::new(Some(stop)),
                task: Mutex::new(Some(task)),
            }),
        }
    }

    pub fn wake(&self) {
        self.inner.wake.notify();
    }

    pub async fn shutdown(&self) {
        if let Some(stop) = self
            .inner
            .stop
            .lock()
            .expect("segment compressor stop mutex poisoned")
            .take()
        {
            let _ = stop.send(());
        }
        let task = self
            .inner
            .task
            .lock()
            .expect("segment compressor task mutex poisoned")
            .take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}

async fn run(
    store: Arc<dyn SessionStore>,
    mut wake: zork_notify::Changes,
    mut stop: tokio::sync::oneshot::Receiver<()>,
    retry_interval: Duration,
) {
    let mut retry =
        zork_notify::retry::Retry::new(retry_interval, retry_interval.saturating_mul(16));
    loop {
        wake.checkpoint();
        let mut failed = false;
        let scan_store = store.clone();
        match tokio::task::spawn_blocking(move || scan_store.compression_candidates()).await {
            Ok(Ok(candidates)) => {
                for candidate in candidates {
                    if stop.try_recv().is_ok() {
                        return;
                    }
                    let compress_store = store.clone();
                    let session_id = candidate.session_id;
                    let first_event_id = candidate.first_event_id;
                    match tokio::task::spawn_blocking(move || {
                        compress_store.compress_segment(&session_id, &first_event_id)
                    })
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            failed = true;
                            tracing::warn!(%error, "segment compression failed");
                        }
                        Err(error) => {
                            failed = true;
                            tracing::warn!(%error, "segment compression task failed");
                        }
                    }
                }
            }
            Ok(Err(error)) => {
                failed = true;
                tracing::warn!(%error, "segment compression scan failed");
            }
            Err(error) => {
                failed = true;
                tracing::warn!(%error, "segment compression scan task failed");
            }
        }
        if !failed {
            retry.reset();
        }
        tokio::select! {
            _ = &mut stop => return,
            changed = wake.changed() => if changed.is_err() { return; },
            _ = retry.wait(), if failed => {},
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use serde_json::Value;

    use super::*;
    use crate::session::events::SessionEvent;
    use crate::session::store::{
        CompressionCandidate, DetachedSession, EventEnvelope, SnapshotAppend, StoreError,
    };

    struct RetryStore {
        pending: Mutex<BTreeSet<CompressionCandidate>>,
        fail_once: AtomicBool,
        active: AtomicUsize,
        peak: AtomicUsize,
        scans: zork_notify::Notifier,
    }

    impl RetryStore {
        fn new(count: usize) -> Self {
            Self {
                pending: Mutex::new(
                    (0..count)
                        .map(|index| CompressionCandidate {
                            session_id: "session".into(),
                            first_event_id: format!("segment-{index}"),
                        })
                        .collect(),
                ),
                fail_once: AtomicBool::new(true),
                active: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                scans: Default::default(),
            }
        }
    }

    impl SessionStore for RetryStore {
        fn append_batch(
            &self,
            _: &str,
            _: &[SessionEvent],
        ) -> Result<Vec<EventEnvelope>, StoreError> {
            unreachable!()
        }

        fn append_snapshot(&self, _: &str, _: u32, _: Value) -> Result<SnapshotAppend, StoreError> {
            unreachable!()
        }

        fn compress_segment(
            &self,
            session_id: &str,
            first_event_id: &str,
        ) -> Result<(), StoreError> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(5));
            self.active.fetch_sub(1, Ordering::SeqCst);
            if self.fail_once.swap(false, Ordering::SeqCst) {
                return Err(StoreError::Io(std::io::Error::other("retry me")));
            }
            self.pending.lock().unwrap().remove(&CompressionCandidate {
                session_id: session_id.into(),
                first_event_id: first_event_id.into(),
            });
            Ok(())
        }

        fn compression_candidates(&self) -> Result<Vec<CompressionCandidate>, StoreError> {
            self.scans.notify();
            Ok(self.pending.lock().unwrap().iter().cloned().collect())
        }

        fn detach_session(&self, _: &str) -> Result<DetachedSession, StoreError> {
            unreachable!()
        }
    }

    #[tokio::test]
    // Contract: docs/design/agent-runtime.md [SEGMENT-02]
    async fn worker_is_single_flight_and_retries_failed_segments() {
        let store = Arc::new(RetryStore::new(3));
        let compressor = SegmentCompressor::start(store.clone(), Duration::from_millis(5));
        tokio::time::timeout(Duration::from_secs(1), async {
            while !store.pending.lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        compressor.shutdown().await;
        assert_eq!(store.peak.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn idle_compressor_waits_for_a_sealed_segment_instead_of_scanning() {
        let store = Arc::new(RetryStore::new(0));
        let mut scans = store.scans.subscribe();
        let compressor = SegmentCompressor::start(store.clone(), Duration::from_millis(5));
        scans.changed().await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(80), scans.changed())
                .await
                .is_err()
        );
        compressor.wake();
        tokio::time::timeout(Duration::from_secs(1), scans.changed())
            .await
            .unwrap()
            .unwrap();
        compressor.shutdown().await;
    }
}
