//! App-owned startup, independent of window construction and rendering.
use super::directory::Directory;
use crate::store::SavedNode;
use anyhow::{Context, Result};
use futures_util::{
    future::{BoxFuture, Shared},
    FutureExt,
};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll},
};
use tokio::sync::oneshot;

type Restored = std::result::Result<Option<SavedNode>, Arc<str>>;

#[derive(Clone)]
pub struct Completion(Shared<BoxFuture<'static, Restored>>);

fn restored(result: Restored) -> Result<Option<SavedNode>> {
    result.map_err(|error| anyhow::anyhow!(error.to_string()))
}

impl Completion {
    pub fn try_ready(&self) -> Option<Result<Option<SavedNode>>> {
        self.0.clone().now_or_never().map(restored)
    }
}

impl Future for Completion {
    type Output = Result<Option<SavedNode>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx).map(restored)
    }
}

/// Keep this handle for the app lifetime, including failure before a window exists.
pub struct Startup {
    pub directory: Arc<Directory>,
    completion: Completion,
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
        let source = directory.clone();
        let (sender, receiver) = oneshot::channel::<Restored>();
        std::thread::Builder::new()
            .name("zork-startup".into())
            .spawn(move || {
                let result = zork_config::service::clear_signal_mask()
                    .map_err(anyhow::Error::from)
                    .and_then(|_| source.restore())
                    .and_then(|node| {
                        if let Some(node) = &node {
                            // HTTP/runtime setup belongs to startup, before the
                            // view asks for this same retained connection.
                            source.connection(&node.id)?;
                        }
                        super::trace_startup("client.startup_connection_ready");
                        Ok(node)
                    });
                let _ = sender.send(result.map_err(|error| Arc::from(error.to_string())));
            })
            .context("启动客户端运行时")?;
        Ok(Self {
            directory,
            completion: Completion(
                async move {
                    receiver
                        .await
                        .unwrap_or_else(|_| Err(Arc::from("客户端启动任务中断")))
                }
                .boxed()
                .shared(),
            ),
        })
    }

    pub fn completion(&self) -> Completion {
        self.completion.clone()
    }

    pub fn shutdown(&self) -> impl Future<Output = ()> + Send + 'static {
        self.directory.cancel_account();
        let local = self.directory.local.shutdown();
        let transport = self.directory.transport.shutdown();
        async move {
            let _ = local.await;
            let _ = transport.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // No window exists to drive cancellation; the detached preparation
        // must relinquish ownership when its directory IO finishes.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
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

    #[tokio::test]
    async fn startup_completes_without_a_window_and_drop_fences_new_work() {
        let root = tempfile::tempdir().unwrap();
        let directory = Directory::open(&root.path().join("client")).unwrap();
        let node = SavedNode {
            id: "saved".into(),
            name: "Saved device".into(),
            url: "http://127.0.0.1:9".into(),
            token: None,
            local: false,
            group: None,
            mesh: None,
        };
        directory.store.save_node(&node).unwrap();
        // Reopen so the published directory includes the persisted selection.
        drop(directory);
        let directory = Directory::open(&root.path().join("client")).unwrap();
        directory.select(&node.id).unwrap();
        let startup = Startup::from_directory(directory.clone()).unwrap();
        assert_eq!(startup.completion().await.unwrap().unwrap().id, node.id);
        assert_eq!(
            startup
                .completion()
                .try_ready()
                .unwrap()
                .unwrap()
                .unwrap()
                .id,
            node.id
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
}

impl Drop for Startup {
    fn drop(&mut self) {
        drop(self.shutdown());
    }
}
