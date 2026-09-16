use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use zork_agent::session::{ports::*, query::FileSessionQuery, tools::*};

struct NetworkFiles {
    started: tokio::sync::Notify,
    dropped: Arc<AtomicUsize>,
}
struct Lease(Arc<AtomicUsize>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
impl NetworkFiles {
    fn pending<T: Send + 'static>(self: Arc<Self>, path: PathBuf) -> FileFuture<T> {
        Box::pin(async move {
            assert!(path.to_string_lossy().starts_with("synch://"));
            let _lease = Lease(self.dropped.clone());
            self.started.notify_one();
            std::future::pending().await
        })
    }
}
impl FileSystem for NetworkFiles {
    fn materialize(self: Arc<Self>, path: PathBuf) -> FileFuture<PathBuf> {
        self.pending(path)
    }
    fn read_page_async(self: Arc<Self>, path: PathBuf, _: u64, _: usize) -> FileFuture<FilePage> {
        self.pending(path)
    }
    fn create_dir_all(&self, _: &Path) -> std::io::Result<()> {
        unreachable!()
    }
    fn create(&self, _: &Path) -> std::io::Result<Box<dyn std::io::Write + Send>> {
        unreachable!()
    }
    fn read_page(&self, _: &Path, _: u64, _: usize) -> std::io::Result<FilePage> {
        panic!("network reads must not block a worker")
    }
    fn read_to_string(&self, _: &Path) -> std::io::Result<String> {
        unreachable!()
    }
    fn write(&self, _: &Path, _: &[u8]) -> std::io::Result<()> {
        unreachable!()
    }
    fn tail(&self, _: &Path, _: usize) -> std::io::Result<Vec<u8>> {
        unreachable!()
    }
}
#[tokio::test]
async fn cancelling_file_tools_drops_pending_network_operations() {
    let root = tempfile::tempdir().unwrap();
    let files = Arc::new(NetworkFiles {
        started: Default::default(),
        dropped: Default::default(),
    });
    let registry = Arc::new(ToolRegistry::default());
    register_builtin_tools(
        &registry,
        BuiltinToolDependencies {
            environment: Default::default(),
            query: Arc::new(FileSessionQuery::open(root.path())),
            clock: Arc::new(SystemClock),
            files: files.clone(),
            processes: Arc::new(SystemProcessSpawner),
        },
    )
    .unwrap();
    for (index, (name, version)) in [
        ("file.read", "builtin-2"),
        ("file.materialize", "builtin-1"),
    ]
    .into_iter()
    .enumerate()
    {
        let ToolResolution::Ready(tool) =
            registry.resolve(name, Some(&ToolVersion::new(version).unwrap()))
        else {
            panic!("missing tool {name}")
        };
        let task = tokio::spawn(async move {
            let context = ToolContext {
                control: None,
                session_id: "session".into(),
                invocation_id: "read".into(),
                workspace: "/unused".into(),
            };
            tool.execute(
                &context,
                &serde_json::json!({"path":"synch://files/remote"}),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(1), files.started.notified())
            .await
            .unwrap();
        task.abort();
        assert!(tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap_err()
            .is_cancelled());
        assert_eq!(files.dropped.load(Ordering::SeqCst), index + 1);
    }
}
