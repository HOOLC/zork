//! Browser authorization owns a read stream and idempotent result receipts.
use super::browser_worker::Worker;
use crate::{api::StationClient, live::LiveEvent};
use anyhow::{ensure, Result};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use zork_browser::Command;

pub struct Grant {
    allowed: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    task: Option<zork_notify::Task<()>>,
    client: Arc<StationClient>,
    path: String,
    registration: Value,
    worker: Worker,
    host: String,
}
impl Drop for Grant {
    fn drop(&mut self) {
        self.allowed.store(false, Ordering::Release);
        self.task.take();
        self.worker.changed(&self.host);
        let (client, path, mut registration) = (
            self.client.clone(),
            self.path.clone(),
            self.registration.clone(),
        );
        registration["disconnect"] = json!(true);
        // Stream IO is already cancelled. This bounded cleanup promptly reports
        // user takeover; operation deadlines also cover a lost acknowledgement.
        self.client.spawn(async move {
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                client.node_request(http::Method::POST, path, Some(registration)),
            )
            .await;
        });
    }
}
impl Grant {
    pub fn start(
        worker: Worker,
        client: Arc<StationClient>,
        _session: String,
        host: String,
    ) -> Self {
        let allowed = Arc::new(AtomicBool::new(true));
        let connected = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let registration = json!({"client_id":client.client_id(), "generation":client.next_browser_generation(), "secret":zork_config::random_token(), "name":"Zork desktop", "replies":[]});
        let path = "/v1/client/browser/receipts".to_owned();
        let (active, ready, failure, owner, scope, channel, credentials) = (
            allowed.clone(),
            connected.clone(),
            error.clone(),
            worker.clone(),
            host.clone(),
            client.clone(),
            registration.clone(),
        );
        let task = client.spawn(async move {
            let outcome = run(&owner, &channel, &scope, &credentials, &active, &ready).await;
            if let Err(error) = outcome {
                *failure.lock().unwrap() = Some(format!("浏览器操作连接失败：{error}"));
            }
            active.store(false, Ordering::Release);
            ready.store(false, Ordering::Release);
            owner.changed(&scope);
        });
        Self {
            allowed,
            connected,
            error,
            task: Some(zork_notify::Task(task)),
            client,
            path,
            registration,
            worker,
            host,
        }
    }
    pub fn active(&self) -> bool {
        self.allowed.load(Ordering::Acquire)
    }
    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }
}
async fn run(
    worker: &Worker,
    client: &Arc<StationClient>,
    host: &str,
    registration: &Value,
    allowed: &Arc<AtomicBool>,
    ready: &AtomicBool,
) -> Result<()> {
    let mut feed = client.browser_events(registration.clone());
    let path = "/v1/client/browser/receipts".to_owned();
    let mut replies: Vec<Value> = vec![];
    // A reconnect uses the same grant identity and retains its execution cache.
    // A new grant has a new identity and cannot inherit/replay old commands.
    let mut receipts: VecDeque<(String, Value, Value)> = VecDeque::new();
    while let Some(event) = feed.next().await {
        match event {
            LiveEvent::Connected => {
                ready.store(true, Ordering::Release);
                worker.changed(host);
            }
            LiveEvent::Disconnected { error, revoked } => {
                ready.store(false, Ordering::Release);
                worker.changed(host);
                ensure!(!revoked, "{error}");
            }
            LiveEvent::Event(event) if event.name == "error" => anyhow::bail!("{}", event.data),
            LiveEvent::Event(event) if event.name == "browser_commands" => {
                let response: Value = serde_json::from_str(&event.data)?;
                let mut work = Vec::new();
                if let Some(commands) = response["commands"].as_array() {
                    for raw in commands {
                        let Ok(command) = serde_json::from_value::<Command>(raw.clone()) else {
                            continue;
                        };
                        let id = command.request_id.clone();
                        if let Some((_, previous, result)) =
                            receipts.iter().find(|(old, _, _)| old == &id)
                        {
                            let result = if previous == raw {
                                result.clone()
                            } else {
                                json!({"error":"browser_request_id_conflict"})
                            };
                            if !replies.iter().any(|r| r["request_id"] == id) {
                                replies.push(json!({"request_id":id,"result":result}));
                            }
                            continue;
                        }
                        let scope = host.to_owned();
                        let permitted = allowed.clone();
                        let received = worker.submit(move |browser| {
                            anyhow::ensure!(permitted.load(Ordering::Acquire), "用户已接管浏览器");
                            if matches!(command.action, zork_browser::Action::List) {
                                return Ok(json!({"tabs":browser.all_tabs()}));
                            }
                            let scope = if let Some(id) = command.action.tab_id() {
                                browser
                                    .all_tabs()
                                    .into_iter()
                                    .find(|tab| tab.id == id)
                                    .ok_or_else(|| anyhow::anyhow!("浏览器标签已关闭"))?
                                    .host
                            } else {
                                scope
                            };
                            browser.execute(&scope, command.action)
                        });
                        let raw = raw.clone();
                        work.push(async move {
                            let result = tokio::task::spawn_blocking(move || match received {
                                Ok(rx) => rx
                                    .recv()
                                    .unwrap_or_else(|_| Err(anyhow::anyhow!("浏览器任务已结束"))),
                                Err(error) => Err(error),
                            })
                            .await
                            .unwrap_or_else(|error| Err(anyhow::anyhow!(error.to_string())));
                            let mut result =
                                result.unwrap_or_else(|error| json!({"error":error.to_string()}));
                            if serde_json::to_vec(&result).map_or(true, |v| v.len() > 192 * 1024) {
                                result = json!({"error":"browser_result_too_large"});
                            }
                            (id, raw, result)
                        });
                    }
                }
                for (id, raw, result) in futures_util::future::join_all(work).await {
                    if allowed.load(Ordering::Acquire)
                        && result.get("error").is_none()
                        && !matches!(
                            raw["action"]["op"].as_str(),
                            Some("list" | "read" | "screenshot" | "close")
                        )
                    {
                        if let Some(tab) = result["tab"]["id"]
                            .as_str()
                            .or_else(|| raw["action"]["tab_id"].as_str())
                        {
                            worker.select(result["tab"]["host"].as_str().unwrap_or(host), tab);
                        }
                    }
                    receipts.push_back((id.clone(), raw, result.clone()));
                    if receipts.len() > 256 {
                        receipts.pop_front();
                    }
                    if !replies.iter().any(|r| r["request_id"] == id) {
                        replies.push(json!({"request_id":id,"result":result}));
                    }
                }
                deliver_receipts(client, &path, registration, &mut replies).await?;
            }
            _ => {}
        }
    }
    anyhow::bail!("浏览器订阅已结束")
}
async fn deliver_receipts(
    client: &StationClient,
    path: &str,
    registration: &Value,
    replies: &mut Vec<Value>,
) -> Result<()> {
    let mut retry = zork_notify::retry::Retry::default();
    while !replies.is_empty() {
        let mut batch = Vec::new();
        let mut bytes = 1024;
        for reply in replies.iter().take(16) {
            let size = serde_json::to_vec(reply)?.len();
            if bytes + size > 224 * 1024 {
                break;
            }
            bytes += size;
            batch.push(reply.clone());
        }
        ensure!(!batch.is_empty(), "browser_receipt_too_large");
        let mut body = registration.clone();
        body["replies"] = json!(batch);
        match client
            .node_request(http::Method::POST, path.into(), Some(body))
            .await
        {
            Ok(value) => {
                let accepted = value["accepted"]
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("browser_receipt_not_accepted"))?;
                let before = replies.len();
                replies.retain(|r| !accepted.contains(&r["request_id"]));
                ensure!(replies.len() < before, "browser_receipt_not_accepted");
                retry.reset();
            }
            Err(error) if error.access_revoked() || matches!(error.status(), Some(400 | 404)) => {
                return Err(error.into())
            }
            Err(_) => retry.wait().await,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn receipt_recovery_preserves_ids_and_obeys_server_batch_limit() {
        use axum::{http::StatusCode, routing::post, Json, Router};
        let observed = Arc::new(Mutex::new(Vec::new()));
        let router = Router::new().route(
            "/receipts",
            post({
                let observed = observed.clone();
                move |Json(body): Json<Value>| {
                    let mut observed = observed.lock().unwrap();
                    let ids: Vec<_> = body["replies"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v["request_id"].clone())
                        .collect();
                    assert!(ids.len() <= 16);
                    observed.push(ids.clone());
                    let failed = observed.len() == 1;
                    async move {
                        if failed {
                            (
                                StatusCode::SERVICE_UNAVAILABLE,
                                Json(json!({"error":"temporary"})),
                            )
                        } else {
                            (StatusCode::OK, Json(json!({"accepted":ids})))
                        }
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = StationClient::new(format!("http://{}", listener.local_addr().unwrap()), None);
        let server = zork_notify::Task(tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        }));
        let mut replies: Vec<_> = (0..17)
            .map(|id| json!({"request_id":format!("r{id}"),"result":{"ok":true}}))
            .collect();
        tokio::time::timeout(
            Duration::from_secs(5),
            deliver_receipts(
                &client,
                "/receipts",
                &json!({"client_id":"fixture"}),
                &mut replies,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(replies.is_empty());
        let observed = observed.lock().unwrap();
        assert_eq!(
            observed.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![16, 16, 1]
        );
        assert_eq!(observed[0], observed[1]);
        assert_eq!(observed[2], vec![json!("r16")]);
        drop(server);
    }
}
