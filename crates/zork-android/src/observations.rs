//! Platform handle lifecycle and lightweight readiness callbacks. Waiting is a
//! parked Rust future, independent of commands, encoding and Java IO threads.
#![cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
use super::Host;
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use zork_client_core::subscriptions::{Key, Signals, WireSubscription};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Query {
    Open {
        generation: u64,
        key: Key,
    },
    Prepare {
        handle: u64,
        generation: u64,
    },
    Validate {
        handle: u64,
        generation: u64,
        batch: u64,
    },
    Finish {
        handle: u64,
        generation: u64,
        batch: u64,
        applied: bool,
    },
    Older {
        handle: u64,
        generation: u64,
    },
    Newer {
        handle: u64,
        generation: u64,
    },
    Window {
        handle: u64,
        generation: u64,
        anchor: Option<String>,
    },
    Detail {
        handle: u64,
        generation: u64,
        id: Option<String>,
    },
    Refresh {
        handle: u64,
        generation: u64,
    },
    Close {
        handle: u64,
        generation: u64,
    },
}
struct Entry {
    generation: u64,
    state: Mutex<Option<WireSubscription>>,
    signals: tokio::sync::Mutex<Signals>,
    closed: tokio::sync::watch::Sender<bool>,
    watcher: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl Entry {
    fn close(&self) {
        self.closed.send_replace(true);
        if let Some(task) = self.watcher.lock().unwrap().take() {
            task.abort();
        }
        self.state.lock().unwrap().take();
    }
    async fn ready(&self) -> Option<bool> {
        let mut closed = self.closed.subscribe();
        let mut signals = self.signals.lock().await;
        if *closed.borrow() {
            return None;
        }
        tokio::select! { result = signals.changed() => result.ok(), _ = closed.changed() => None }
    }
}
#[derive(Default)]
pub(super) struct Registry {
    next: AtomicU64,
    entries: Mutex<HashMap<u64, Arc<Entry>>>,
}
impl Drop for Registry {
    fn drop(&mut self) {
        for entry in self.entries.get_mut().unwrap().values() {
            entry.close();
        }
    }
}
impl Registry {
    fn insert(&self, state: WireSubscription, generation: u64) -> u64 {
        let signals = state.signals();
        let handle = self
            .next
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .expect("observation handles exhausted")
            + 1;
        self.entries.lock().unwrap().insert(
            handle,
            Arc::new(Entry {
                generation,
                state: Mutex::new(Some(state)),
                signals: tokio::sync::Mutex::new(signals),
                closed: tokio::sync::watch::channel(false).0,
                watcher: Mutex::new(None),
            }),
        );
        handle
    }
    fn get(&self, handle: u64, generation: u64) -> Result<Arc<Entry>> {
        let entry = self
            .entries
            .lock()
            .unwrap()
            .get(&handle)
            .cloned()
            .context("订阅已关闭")?;
        ensure!(entry.generation == generation, "订阅已切换");
        Ok(entry)
    }
    pub(super) fn watch(
        &self,
        runtime: &tokio::runtime::Runtime,
        handle: u64,
        generation: u64,
        notify: impl Fn(bool, bool) -> bool + Send + 'static,
    ) -> Result<()> {
        let entry = self.get(handle, generation)?;
        let mut watcher = entry.watcher.lock().unwrap();
        ensure!(!*entry.closed.borrow(), "订阅已关闭");
        ensure!(watcher.is_none(), "订阅已注册通知");
        let owner = entry.clone();
        *watcher = Some(runtime.spawn(async move {
            loop {
                let ready = owner.ready().await;
                if *owner.closed.borrow() {
                    return;
                }
                // No source/registry/reader lock is held while calling Java.
                if !notify(ready.unwrap_or(false), ready.is_none()) {
                    owner.close();
                    return;
                }
                if ready.is_none() {
                    return;
                }
            }
        }));
        Ok(())
    }
    pub(super) fn execute(&self, host: &Host, query: Query) -> Result<Value> {
        match query {
            Query::Open { generation, key } => {
                let _runtime = host.executor.enter();
                let state = host.local.observe(key)?;
                let handle = self.insert(state, generation);
                Ok(json!({"handle":handle,"generation":generation}))
            }
            Query::Close { handle, generation } => {
                let mut entries = self.entries.lock().unwrap();
                if let Some(entry) = entries.get(&handle) {
                    ensure!(entry.generation == generation, "订阅已切换");
                }
                let entry = entries.remove(&handle);
                drop(entries);
                if let Some(entry) = entry {
                    entry.close();
                }
                Ok(json!({}))
            }
            query => {
                let (handle, generation) = match &query {
                    Query::Prepare { handle, generation }
                    | Query::Validate {
                        handle, generation, ..
                    }
                    | Query::Finish {
                        handle, generation, ..
                    }
                    | Query::Older { handle, generation }
                    | Query::Newer { handle, generation }
                    | Query::Window {
                        handle, generation, ..
                    }
                    | Query::Detail {
                        handle, generation, ..
                    }
                    | Query::Refresh { handle, generation } => (*handle, *generation),
                    _ => unreachable!(),
                };
                let entry = self.get(handle, generation)?;
                let mut state = entry.state.lock().unwrap();
                let state = state.as_mut().context("订阅已关闭")?;
                let _runtime = host.executor.enter();
                match query {
                    Query::Prepare { .. } => {
                        let mut frame = state
                            .prepare()?
                            .map(|v| v.as_ref().clone())
                            .unwrap_or_else(|| json!({}));
                        frame["handle"] = json!(handle);
                        frame["generation"] = json!(generation);
                        Ok(frame)
                    }
                    Query::Validate { batch, .. } => Ok(json!({"valid":state.valid(batch)})),
                    Query::Finish { batch, applied, .. } => {
                        Ok(json!({"accepted":state.finish(batch, applied)}))
                    }
                    Query::Older { .. } => {
                        state.older()?;
                        Ok(json!({}))
                    }
                    Query::Newer { .. } => {
                        state.newer()?;
                        Ok(json!({}))
                    }
                    Query::Window { anchor, .. } => {
                        state.window_anchor(anchor)?;
                        Ok(json!({}))
                    }
                    Query::Detail { id, .. } => {
                        state.detail(id)?;
                        Ok(json!({}))
                    }
                    Query::Refresh { .. } => {
                        state.refresh()?;
                        Ok(json!({}))
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zork_client_core::{api::GatewayClient, state::Device, store::ClientStore, Client};
    #[test]
    fn callbacks_coalesce_without_command_locks_and_reject_a_b_a_stale_results() {
        let root = tempfile::tempdir().unwrap();
        let client = Client::open(root.path()).unwrap();
        let host = Arc::new(Host {
            root: root.path().into(),
            executor: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap(),
            local: client.local(),
            client: tokio::sync::Mutex::new(client),
            observations: Registry::default(),
        });
        let store = Arc::new(ClientStore::open(root.path()).unwrap());
        let device = Device::open(
            Arc::new(GatewayClient::new("http://127.0.0.1:9", None)),
            None,
            true,
        );
        let observer = |session: &str| {
            WireSubscription::from_device(
                Key::Conversation {
                    peer: "peer".into(),
                    session: Some(session.into()),
                },
                device.clone(),
                store.clone(),
            )
            .unwrap()
        };
        let first = host.observations.insert(observer("a"), 10);
        let other = host.observations.insert(observer("b"), 11);
        let _network = host.client.blocking_lock();
        let (sent, received) = std::sync::mpsc::channel();
        host.observations
            .watch(&host.executor, first, 10, move |urgent, closed| {
                sent.send((urgent, closed)).is_ok()
            })
            .unwrap();
        assert_eq!(
            received
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap(),
            (false, false)
        );
        let frame = host
            .observations
            .execute(
                &host,
                Query::Prepare {
                    handle: first,
                    generation: 10,
                },
            )
            .unwrap();
        let old_batch = frame["batch"].as_u64().unwrap();
        assert_eq!(
            host.observations
                .execute(
                    &host,
                    Query::Finish {
                        handle: first,
                        generation: 10,
                        batch: old_batch,
                        applied: true
                    }
                )
                .unwrap()["accepted"],
            true
        );
        assert!(
            received
                .recv_timeout(std::time::Duration::from_millis(30))
                .is_err(),
            "idle observation woke without a commit"
        );
        let other_frame = host
            .observations
            .execute(
                &host,
                Query::Prepare {
                    handle: other,
                    generation: 11,
                },
            )
            .unwrap();
        assert!(other_frame["state"]["messages"].is_array());
        for i in 0..2000 {
            device.edit_draft("a", format!("edit {i}")).unwrap();
        }
        assert_eq!(
            received
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap(),
            (false, false)
        );
        assert!(
            received
                .recv_timeout(std::time::Duration::from_millis(30))
                .is_err(),
            "burst did not coalesce before preparation"
        );
        let latest = host
            .observations
            .execute(
                &host,
                Query::Prepare {
                    handle: first,
                    generation: 10,
                },
            )
            .unwrap();
        assert_eq!(latest["state"]["draft_document"]["text"], "edit 1999");
        host.observations
            .execute(
                &host,
                Query::Close {
                    handle: first,
                    generation: 10,
                },
            )
            .unwrap();
        assert!(received
            .recv_timeout(std::time::Duration::from_millis(30))
            .is_err());
        let reopened = host.observations.insert(observer("a"), 12);
        assert_ne!(first, reopened);
        assert!(host
            .observations
            .execute(
                &host,
                Query::Prepare {
                    handle: reopened,
                    generation: 10
                }
            )
            .is_err());
        assert!(host
            .observations
            .execute(
                &host,
                Query::Finish {
                    handle: first,
                    generation: 10,
                    batch: old_batch,
                    applied: true
                }
            )
            .is_err());
        assert!(host
            .observations
            .execute(
                &host,
                Query::Prepare {
                    handle: reopened,
                    generation: 12
                }
            )
            .unwrap()["reset"]
            .as_bool()
            .unwrap());
        host.observations
            .execute(
                &host,
                Query::Close {
                    handle: other,
                    generation: 11,
                },
            )
            .unwrap();
        host.observations
            .execute(
                &host,
                Query::Close {
                    handle: reopened,
                    generation: 12,
                },
            )
            .unwrap();
        assert!(host.observations.entries.lock().unwrap().is_empty());
    }
}
