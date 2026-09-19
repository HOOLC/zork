//! App-owned recovery. Cached workspaces, local readiness and Mesh recovery are
//! independent: a peer cannot be a prerequisite for starting our own Station.
use super::directory::Directory;
use crate::{
    state::{Observable, Subscription},
    store::SavedNode,
};
use anyhow::{Context, Result};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    NotRequired,
    Preparing,
    Stopping,
    Ready,
    Failed(String),
    StopFailed(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// A saved workspace can be opened before its runtime or network is ready.
    pub selected: Option<String>,
    /// Preserve a new activation request even if intermediate updates coalesce.
    pub selection_generation: u64,
    pub local: Phase,
    pub mesh: Phase,
}

impl State {
    /// Only the selected device's dependency belongs in its content area.
    pub fn dependency(&self, node: &SavedNode) -> &Phase {
        if node.local {
            &self.local
        } else if node.mesh.is_some() {
            &self.mesh
        } else {
            &Phase::NotRequired
        }
    }
}

#[derive(Clone, Copy)]
enum Step {
    Local,
    Mesh,
}

impl Step {
    fn phase(self, state: &mut State) -> &mut Phase {
        match self {
            Self::Local => &mut state.local,
            Self::Mesh => &mut state.mesh,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Local => "zork-local-startup",
            Self::Mesh => "zork-mesh-startup",
        }
    }
}

struct Recovery {
    directory: Arc<Directory>,
    state: Mutex<State>,
    updates: Observable<State>,
    stopping: AtomicBool,
}

impl Recovery {
    fn new(directory: Arc<Directory>, state: State) -> Arc<Self> {
        Arc::new(Self {
            directory,
            updates: Observable::new(state.clone()),
            state: Mutex::new(state),
            stopping: AtomicBool::new(false),
        })
    }

    fn launch(self: &Arc<Self>, step: Step) {
        let source = self.directory.clone();
        self.launch_with(step, move || match step {
            Step::Local => source.start_local().map(Some),
            Step::Mesh => source.pair(None),
        });
    }

    fn launch_with(
        self: &Arc<Self>,
        step: Step,
        work: impl FnOnce() -> Result<Option<SavedNode>> + Send + 'static,
    ) {
        let mut state = self.state.lock().expect("startup state");
        if self.stopping.load(Ordering::Acquire)
            || matches!(
                step.phase(&mut state),
                Phase::Preparing | Phase::Stopping | Phase::Ready
            )
        {
            return;
        }
        *step.phase(&mut state) = Phase::Preparing;
        self.updates.publish(state.clone());
        drop(state);
        let owner = self.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(step.name().into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    zork_config::service::clear_signal_mask()
                        .map_err(anyhow::Error::from)
                        .and_then(|_| work())
                        .and_then(|node| {
                            if let Some(node) = &node {
                                owner.directory.connection(&node.id)?;
                            }
                            Ok(node)
                        })
                }))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("设备准备任务意外结束，请重试")));
                owner.finish(step, result);
            })
        {
            self.finish(step, Err(error.into()));
        }
    }

    fn finish(&self, step: Step, result: Result<Option<SavedNode>>) {
        let mut state = self.state.lock().expect("startup state");
        if self.stopping.load(Ordering::Acquire) {
            return;
        }
        match result {
            Ok(node) => {
                if let Some(node) = node.filter(|_| state.selected.is_none()) {
                    state.selected = Some(node.id);
                    state.selection_generation = state.selection_generation.wrapping_add(1);
                }
                *step.phase(&mut state) = Phase::Ready;
            }
            Err(error) => *step.phase(&mut state) = Phase::Failed(format!("{error:#}")),
        }
        self.updates.publish(state.clone());
        super::trace_startup(match step {
            Step::Local => "client.local_restore_complete",
            Step::Mesh => "client.mesh_restore_complete",
        });
    }
}

/// Keep this handle for the app lifetime, including failure before a window exists.
pub struct Startup {
    pub directory: Arc<Directory>,
    recovery: Arc<Recovery>,
}

/// Directory IO can overlap native application setup. Dropping an unclaimed
/// preparation also drops its eventual Startup, cancelling any restored work.
pub struct Preparation(std::thread::JoinHandle<Result<Startup>>);

impl Preparation {
    fn start(open: impl FnOnce() -> Result<Startup> + Send + 'static) -> Result<Self> {
        Ok(Self(
            std::thread::Builder::new()
                .name("zork-directory".into())
                .spawn(move || {
                    zork_config::service::clear_signal_mask()?;
                    open()
                })
                .context("准备客户端运行时")?,
        ))
    }

    pub fn finish(self) -> Result<Startup> {
        self.0
            .join()
            .map_err(|_| anyhow::anyhow!("客户端目录准备任务中断"))?
    }
}

impl Startup {
    pub fn prepare() -> Result<Preparation> {
        Preparation::start(Self::open)
    }

    pub fn open() -> Result<Self> {
        Self::from_directory(Directory::open(&super::client_root())?)
    }

    fn from_directory(directory: Arc<Directory>) -> Result<Self> {
        let snapshot = directory.snapshot();
        let available = |node: &&SavedNode| !node.local || snapshot.local_enabled;
        let selected = directory
            .selected()
            .and_then(|id| {
                snapshot
                    .nodes
                    .iter()
                    .filter(available)
                    .find(|node| node.id == id)
            })
            .or_else(|| snapshot.nodes.iter().find(available));
        if let Some(node) = selected {
            directory.connection(&node.id)?;
        }
        let needs_mesh = snapshot.nodes.iter().any(|node| {
            node.mesh.is_some()
                || (node.local
                    && directory
                        .store
                        .get::<String>(&node.id, "mesh-origin")
                        .ok()
                        .flatten()
                        .is_some())
        });
        let recovery = Recovery::new(
            directory.clone(),
            State {
                selected: selected.map(|node| node.id.clone()),
                selection_generation: 1,
                ..State::default()
            },
        );
        // Launch independently. Mesh readoption can contact this same Station,
        // or wait for an unavailable remote peer, without delaying local use.
        if snapshot.local_enabled {
            recovery.launch(Step::Local);
        }
        if needs_mesh {
            recovery.launch(Step::Mesh);
        }
        super::trace_startup("client.cached_workspace_ready");
        Ok(Self {
            directory,
            recovery,
        })
    }

    pub fn subscribe(&self) -> Subscription<State> {
        self.recovery.updates.subscribe()
    }

    pub fn start_local(&self) {
        let mut state = self.recovery.state.lock().expect("startup state");
        if matches!(state.local, Phase::Preparing | Phase::Stopping)
            || self.recovery.stopping.load(Ordering::Acquire)
        {
            return;
        }
        state.local = Phase::NotRequired;
        state.selected = self
            .directory
            .snapshot()
            .nodes
            .iter()
            .find(|node| node.local)
            .map(|node| node.id.clone());
        state.selection_generation = state.selection_generation.wrapping_add(1);
        drop(state);
        self.recovery.launch(Step::Local);
    }

    pub fn stop_local(&self) -> bool {
        let mut state = self.recovery.state.lock().expect("startup state");
        if matches!(state.local, Phase::Preparing | Phase::Stopping)
            || self.recovery.stopping.load(Ordering::Acquire)
        {
            return false;
        }
        state.local = Phase::Stopping;
        state.selected = None;
        state.selection_generation = state.selection_generation.wrapping_add(1);
        self.recovery.updates.publish(state.clone());
        drop(state);
        let owner = self.recovery.clone();
        let finish = |owner: &Recovery, result: Result<()>| {
            let mut state = owner.state.lock().expect("startup state");
            if owner.stopping.load(Ordering::Acquire) {
                return;
            }
            state.local = match result {
                Ok(()) => Phase::NotRequired,
                Err(error) => Phase::StopFailed(format!("{error:#}")),
            };
            owner.updates.publish(state.clone());
        };
        if let Err(error) = std::thread::Builder::new()
            .name("zork-local-stop".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    zork_config::service::clear_signal_mask()
                        .map_err(anyhow::Error::from)
                        .and_then(|_| owner.directory.stop_local())
                }))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("设备停止任务意外结束，请重试")));
                finish(&owner, result);
            })
        {
            finish(&self.recovery, Err(error.into()));
        }
        true
    }

    pub fn retry(&self) {
        let state = self.recovery.updates.read();
        if matches!(state.local, Phase::Failed(_)) {
            self.recovery.launch(Step::Local);
        }
        if matches!(state.local, Phase::StopFailed(_)) {
            self.stop_local();
        }
        if matches!(state.mesh, Phase::Failed(_)) {
            self.recovery.launch(Step::Mesh);
        }
    }

    pub fn shutdown(&self) -> impl Future<Output = ()> + Send + 'static {
        self.recovery.stopping.store(true, Ordering::Release);
        let _ = self.directory.cancel_account();
        let local = self.directory.local.shutdown();
        let transport = self.directory.transport.shutdown();
        async move {
            let _ = local.await;
            let _ = transport.await;
        }
    }
}

impl Drop for Startup {
    fn drop(&mut self) {
        drop(self.shutdown());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn saved() -> SavedNode {
        SavedNode {
            id: "saved".into(),
            name: "Saved device".into(),
            url: "http://127.0.0.1:9".into(),
            token: None,
            local: false,
            group: None,
            mesh: None,
        }
    }

    #[test]
    fn abandoned_preparation_cancels_the_runtime_even_when_open_is_pending() {
        let root = tempfile::tempdir().unwrap();
        let directory = Directory::open(&root.path().join("client")).unwrap();
        let source = directory.clone();
        let (release, pending) = std::sync::mpsc::channel();
        let preparation = Preparation::start(move || {
            pending.recv().unwrap();
            Startup::from_directory(source)
        })
        .unwrap();
        drop(preparation);
        release.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while Arc::strong_count(&directory) != 1 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(directory
            .local
            .start()
            .unwrap_err()
            .to_string()
            .contains("客户端正在退出"));
    }

    #[test]
    fn cached_selection_is_available_without_a_window_and_drop_fences_new_work() {
        let root = tempfile::tempdir().unwrap();
        let directory = Directory::open(&root.path().join("client")).unwrap();
        let node = saved();
        directory.store.save_node(&node).unwrap();
        drop(directory);
        let directory = Directory::open(&root.path().join("client")).unwrap();
        directory.select(&node.id).unwrap();
        let startup = Startup::from_directory(directory.clone()).unwrap();
        assert_eq!(
            startup.subscribe().snapshot().selected.as_deref(),
            Some(node.id.as_str())
        );
        drop(startup);
        assert!(directory
            .local
            .start()
            .unwrap_err()
            .to_string()
            .contains("客户端正在退出"));
        assert!(directory
            .transport
            .start(&[])
            .unwrap_err()
            .to_string()
            .contains("客户端正在退出"));
    }

    #[tokio::test]
    async fn pending_mesh_does_not_block_local_recovery_and_failures_are_retryable() {
        let root = tempfile::tempdir().unwrap();
        let directory = Directory::open(&root.path().join("client")).unwrap();
        let recovery = Recovery::new(
            directory,
            State {
                selected: Some("saved".into()),
                ..State::default()
            },
        );
        let mut updates = recovery.updates.subscribe();
        let (release, pending) = std::sync::mpsc::channel();
        recovery.launch_with(Step::Mesh, move || {
            pending.recv()?;
            anyhow::bail!("peer recovery failed")
        });
        recovery.launch_with(Step::Local, || Ok(None));
        tokio::time::timeout(Duration::from_secs(2), async {
            while updates.snapshot().local != Phase::Ready {
                updates.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert_eq!(updates.snapshot().mesh, Phase::Preparing);
        assert_eq!(updates.snapshot().selected.as_deref(), Some("saved"));
        recovery.launch_with(Step::Mesh, || panic!("duplicate startup"));
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !matches!(updates.snapshot().mesh, Phase::Failed(_)) {
                updates.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        recovery.launch_with(Step::Mesh, || Ok(None));
        tokio::time::timeout(Duration::from_secs(2), async {
            while updates.snapshot().mesh != Phase::Ready {
                updates.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert_eq!(updates.snapshot().selected.as_deref(), Some("saved"));
        recovery.stopping.store(true, Ordering::Release);
        recovery.finish(Step::Mesh, Err(anyhow::anyhow!("late failure")));
        assert_eq!(updates.snapshot().mesh, Phase::Ready);
    }
}
