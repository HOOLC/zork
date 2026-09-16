//! Running several futures to completion together.

use std::future::Future;

/// Runs several futures to completion together, collecting their outputs.
///
/// A hand-rolled join rather than a `futures` dependency: what the engine needs
/// is the simplest possible shape — no cancellation, no early return, every
/// branch polled to the end — and that is a dozen lines. The outputs come back
/// in completion order, which is all any caller here asks of them.
pub(crate) async fn futures_join<F: Future>(
    futures: impl IntoIterator<Item = F>,
) -> Vec<F::Output> {
    let mut pending: Vec<std::pin::Pin<Box<F>>> = futures.into_iter().map(Box::pin).collect();
    let mut out = Vec::with_capacity(pending.len());
    std::future::poll_fn(move |cx| {
        let mut index = 0;
        while index < pending.len() {
            match pending[index].as_mut().poll(cx) {
                std::task::Poll::Ready(value) => {
                    out.push(value);
                    drop(pending.remove(index));
                }
                std::task::Poll::Pending => index += 1,
            }
        }
        if pending.is_empty() {
            std::task::Poll::Ready(std::mem::take(&mut out))
        } else {
            std::task::Poll::Pending
        }
    })
    .await
}

/// Runs a finite queue of futures with at most `limit` in flight.
///
/// Unlike chunking the input into fixed waves, every completion immediately
/// admits the next future. One slow or broken operation therefore occupies one
/// slot without holding all later work behind its timeout.
pub(crate) async fn futures_buffered<F: Future>(futures: Vec<F>, limit: usize) -> Vec<F::Output> {
    let mut queued = futures.into_iter();
    let limit = limit.max(1);
    let mut pending: Vec<std::pin::Pin<Box<F>>> = Vec::with_capacity(limit);
    let mut out = Vec::new();
    let mut exhausted = false;
    std::future::poll_fn(move |cx| loop {
        while pending.len() < limit && !exhausted {
            match queued.next() {
                Some(future) => pending.push(Box::pin(future)),
                None => exhausted = true,
            }
        }
        if exhausted && pending.is_empty() {
            return std::task::Poll::Ready(std::mem::take(&mut out));
        }

        let mut completed = false;
        let mut index = 0;
        while index < pending.len() {
            match pending[index].as_mut().poll(cx) {
                std::task::Poll::Ready(value) => {
                    out.push(value);
                    pending.remove(index);
                    completed = true;
                }
                std::task::Poll::Pending => index += 1,
            }
        }
        if !completed {
            return std::task::Poll::Pending;
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_slow_future_does_not_hold_the_next_queue_slot() {
        use std::future::Future;
        use std::pin::Pin;

        let (release_slow, slow) = tokio::sync::oneshot::channel();
        let (third_started, third) = tokio::sync::oneshot::channel();
        let futures: Vec<Pin<Box<dyn Future<Output = usize> + Send>>> = vec![
            Box::pin(async move {
                let _ = slow.await;
                0
            }),
            Box::pin(async { 1 }),
            Box::pin(async move {
                let _ = third_started.send(());
                2
            }),
        ];

        let running = tokio::spawn(futures_buffered(futures, 2));
        tokio::time::timeout(std::time::Duration::from_secs(1), third)
            .await
            .expect("the third future should start while the first is still pending")
            .expect("the third future should report that it started");
        release_slow.send(()).unwrap();
        assert_eq!(running.await.unwrap(), vec![1, 2, 0]);
    }

    #[tokio::test]
    async fn the_buffer_never_starts_more_than_the_limit() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };

        let started = Arc::new(AtomicUsize::new(0));
        let two_started = Arc::new(tokio::sync::Notify::new());
        let (release, _) = tokio::sync::watch::channel(false);
        let futures: Vec<_> = (0..6)
            .map(|value| {
                let started = started.clone();
                let two_started = two_started.clone();
                let mut release = release.subscribe();
                async move {
                    if started.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
                        two_started.notify_one();
                    }
                    while !*release.borrow() {
                        release.changed().await.unwrap();
                    }
                    value
                }
            })
            .collect();

        let running = tokio::spawn(futures_buffered(futures, 2));
        tokio::time::timeout(std::time::Duration::from_secs(1), two_started.notified())
            .await
            .expect("the first two futures should start");
        tokio::task::yield_now().await;
        assert_eq!(
            started.load(Ordering::SeqCst),
            2,
            "queued futures must not start before an in-flight slot opens"
        );
        release.send(true).unwrap();
        assert_eq!(running.await.unwrap().len(), 6);
    }
}
