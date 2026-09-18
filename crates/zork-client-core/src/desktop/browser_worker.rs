use std::{
    collections::HashMap,
    sync::{mpsc, Arc, Mutex},
};
use zork_browser::Browser;
type Job = Box<dyn FnOnce(&Browser) + Send>;
#[derive(Clone)]
pub struct Worker {
    pub browser: Arc<Browser>,
    tx: mpsc::SyncSender<Job>,
    selected: Arc<Mutex<HashMap<String, String>>>,
    services: Arc<Mutex<Services>>,
    changes: zork_notify::Hub<String>,
    pages: Arc<Mutex<HashMap<(String, String), String>>>,
    page_gates: Arc<Mutex<HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>>,
}
#[derive(Default)]
struct Services {
    pending: std::collections::HashSet<String>,
    tabs: HashMap<String, (String, String, zork_mesh::services::LocalService)>,
}
impl Services {
    fn reconcile(&mut self, browser: &Browser) {
        self.tabs
            .retain(|id, (host, _, _)| browser.tabs(host).iter().any(|tab| tab.id == *id));
    }
}
impl Worker {
    pub fn new() -> Self {
        let browser = Arc::new(Browser::new(
            crate::desktop::client_root().join("browser/cef"),
        ));
        let (tx, rx) = mpsc::sync_channel::<Job>(64);
        let process = browser.clone();
        std::thread::Builder::new()
            .name("zork-browser-commands".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    job(&process);
                }
            })
            .expect("browser worker");
        Self {
            browser,
            tx,
            selected: Arc::new(Mutex::new(HashMap::new())),
            services: Arc::new(Mutex::new(Services::default())),
            changes: Default::default(),
            pages: Default::default(),
            page_gates: Default::default(),
        }
    }
    pub fn subscribe(&self, host: &str) -> zork_notify::Changes {
        self.browser
            .subscribe(host)
            .merge(self.changes.subscribe([host.to_owned()]))
    }
    pub(crate) fn changed(&self, host: &str) {
        self.changes.publish([host.to_owned()]);
    }
    pub fn select(&self, host: &str, tab: &str) {
        let previous = self
            .selected
            .lock()
            .unwrap()
            .insert(host.into(), tab.into());
        if previous.as_deref() != Some(tab) {
            self.changed(host);
        }
    }
    pub fn selected(&self, host: &str) -> Option<String> {
        self.selected.lock().unwrap().get(host).cloned()
    }
    pub fn clear_selection(&self, host: &str) {
        if self.selected.lock().unwrap().remove(host).is_some() {
            self.changed(host);
        }
    }
    pub fn submit<T: Send + 'static>(
        &self,
        job: impl FnOnce(&Browser) -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<mpsc::Receiver<anyhow::Result<T>>> {
        let (tx, rx) = mpsc::channel();
        let browser = self.browser.clone();
        let services = self.services.clone();
        std::thread::Builder::new()
            .name("zork-browser-operation".into())
            .spawn(move || {
                let result = job(&browser);
                services.lock().unwrap().reconcile(&browser);
                let _ = tx.send(result);
            })?;
        Ok(rx)
    }
    /// Preserve physical keyboard/pointer ordering without blocking independent
    /// browser operations or a Stop command behind a pending navigation.
    pub fn submit_ordered<T: Send + 'static>(
        &self,
        job: impl FnOnce(&Browser) -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<mpsc::Receiver<anyhow::Result<T>>> {
        let (tx, rx) = mpsc::channel();
        let services = self.services.clone();
        self.tx
            .try_send(Box::new(move |browser| {
                let result = job(browser);
                services.lock().unwrap().reconcile(browser);
                let _ = tx.send(result);
            }))
            .map_err(|_| anyhow::anyhow!("浏览器操作队列繁忙，请稍后重试"))?;
        Ok(rx)
    }
    pub async fn open_service(
        &self,
        client: Arc<crate::api::StationClient>,
        host: String,
        url: String,
    ) -> anyhow::Result<serde_json::Value> {
        let parsed = reqwest::Url::parse(&url)?;
        anyhow::ensure!(
            matches!(parsed.scheme(), "zork" | "http" | "https")
                && parsed.username().is_empty()
                && parsed.password().is_none(),
            "Unsupported page URL"
        );
        let url = parsed.to_string();
        let key = format!("{host}\0{url}");
        let gate = {
            let mut gates = self.page_gates.lock().unwrap();
            gates.retain(|_, gate| gate.strong_count() > 0);
            if let Some(gate) = gates.get(&key).and_then(std::sync::Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(tokio::sync::Mutex::new(()));
                gates.insert(key, Arc::downgrade(&gate));
                gate
            }
        };
        let worker = self.clone();
        let station = client.clone();
        client
            .spawn(async move {
                let _guard = gate.lock().await;
                let tabs = worker.browser.tabs(&host);
                let known = {
                    let mut pages = worker.pages.lock().unwrap();
                    pages.retain(|(owner, _), id| {
                        owner != &host || tabs.iter().any(|tab| &tab.id == id)
                    });
                    pages.get(&(host.clone(), url.clone())).cloned()
                };
                let existing = {
                    let mut services = worker.services.lock().unwrap();
                    services.reconcile(&worker.browser);
                    services
                        .tabs
                        .iter()
                        .find(|(_, (owner, source, _))| owner == &host && source == &url)
                        .map(|(id, _)| id.clone())
                }
                .or(known)
                .and_then(|id| tabs.iter().find(|tab| tab.id == id));
                if let Some(tab) = existing.or_else(|| tabs.iter().find(|tab| tab.url == url)) {
                    worker.select(&host, &tab.id);
                    return Ok(serde_json::json!({"tab":tab}));
                }
                if parsed.scheme() == "zork" {
                    return worker.open_service_new(station, host, url).await;
                }
                let page_key = (host.clone(), url.clone());
                let browser_host = host.clone();
                let receiver = worker.submit(move |browser| {
                    browser.execute(&browser_host, zork_browser::Action::Open { url })
                })?;
                let value = tokio::task::spawn_blocking(move || {
                    receiver
                        .recv()
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("browser task stopped")))
                })
                .await??;
                if let Some(id) = value["tab"]["id"].as_str() {
                    worker.pages.lock().unwrap().insert(page_key, id.into());
                    worker.select(&host, id);
                }
                Ok(value)
            })
            .await?
    }
    async fn open_service_new(
        &self,
        client: Arc<crate::api::StationClient>,
        host: String,
        url: String,
    ) -> anyhow::Result<serde_json::Value> {
        let id = ulid::Ulid::new().to_string();
        {
            let mut services = self.services.lock().unwrap();
            anyhow::ensure!(
                services.pending.len() + services.tabs.len() < 16,
                "同时打开的服务页面不能超过 16 个"
            );
            services.pending.insert(id.clone());
        }
        let worker = self.clone();
        let station = client.clone();
        client
            .spawn(async move {
                let result = async {
                    let service = station.open_shared_service(&url).await?;
                    let local_url = service.url.clone();
                    let browser_host = host.clone();
                    let receiver = worker.submit(move |browser| {
                        browser
                            .execute(&browser_host, zork_browser::Action::Open { url: local_url })
                    })?;
                    let value = tokio::task::spawn_blocking(move || {
                        receiver
                            .recv()
                            .unwrap_or_else(|_| Err(anyhow::anyhow!("browser task stopped")))
                    })
                    .await??;
                    let tab = value["tab"]["id"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("browser did not return a tab"))?;
                    let mut services = worker.services.lock().unwrap();
                    anyhow::ensure!(services.pending.contains(&id), "service view closed");
                    services.tabs.insert(tab.into(), (host, url, service));
                    Ok(value)
                }
                .await;
                worker.services.lock().unwrap().pending.remove(&id);
                result
            })
            .await?
    }
    pub fn shutdown(&self) {
        let mut services = self.services.lock().unwrap();
        services.pending.clear();
        services.tabs.clear();
        drop(services);
        self.browser.shutdown();
    }
}
