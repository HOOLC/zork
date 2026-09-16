//! Public, offline Gateway adapter for portable core contracts and UI fixtures.
#[path = "profile_types.rs"]
mod profile_types;
pub use profile_types::{ProfileInfo, ProfileModel, ProfileQuota};
use serde_json::Value;
use std::{
    cell::RefCell,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

pub trait Executor {
    fn wait(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + 'static>>;
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + 'static>>);
}
type Fixture = (Value, Value, Arc<dyn Executor>);
thread_local! {static FIXTURE: RefCell<Option<Fixture>> = const {RefCell::new(None)};}
pub fn configure_fixture(fixture: Value, providers: Value, executor: Arc<dyn Executor>) {
    FIXTURE.with(|slot| *slot.borrow_mut() = Some((fixture, providers, executor)));
}
pub(crate) struct ClientTask {
    abort: futures_util::future::AbortHandle,
    finished: Arc<AtomicBool>,
}
impl ClientTask {
    pub fn abort(&self) {
        self.abort.abort();
    }
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}
#[path = "offline.rs"]
mod offline;
pub struct GatewayClient {
    fixture: offline::Fixture,
    executor: Arc<dyn Executor>,
}
impl GatewayClient {
    pub fn new(_: &str, _: Option<String>) -> Self {
        let (fixture, providers, executor) = FIXTURE
            .with(|fixture| fixture.borrow().clone())
            .expect("configure the core fixture adapter before opening views");
        Self {
            fixture: offline::Fixture::new(fixture, providers),
            executor,
        }
    }
    pub(crate) async fn wait(&self, duration: Duration) {
        self.executor.wait(duration).await;
    }
    pub(crate) fn spawn<F: Future<Output = ()> + 'static>(&self, future: F) -> ClientTask {
        let (abort, registration) = futures_util::future::AbortHandle::new_pair();
        let finished = Arc::new(AtomicBool::new(false));
        let done = finished.clone();
        self.executor.spawn(Box::pin(async move {
            let _ = futures_util::future::Abortable::new(future, registration).await;
            done.store(true, Ordering::Release);
        }));
        ClientTask { abort, finished }
    }

    pub async fn list_profiles(&self) -> anyhow::Result<Vec<ProfileInfo>> {
        self.fixture.list_profiles()
    }
    pub async fn node_request(
        &self,
        method: http::Method,
        path: String,
        body: Option<Value>,
    ) -> anyhow::Result<Value> {
        self.fixture.node_request(method, path, body)
    }
}
