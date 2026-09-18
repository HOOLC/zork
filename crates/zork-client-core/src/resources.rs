//! On-demand read-only Mesh inventory. All UI surfaces observe this projection.
use crate::{
    api::StationClient,
    state::{Observable, Subscription},
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
pub use zork_client_types::resources::*;

pub struct Resources {
    clients: Mutex<Vec<(String, u64, Arc<StationClient>)>>,
    next: AtomicU64,
    state: Mutex<ResourcesData>,
    changes: Observable<ResourcesData>,
}
impl Resources {
    /// A read-only transport fixture using the same snapshots as connected clients.
    #[doc(hidden)]
    pub fn fixture(data: ResourcesData) -> Arc<Self> {
        let source = Self::new(vec![]);
        *source.state.lock().unwrap() = data.clone();
        source.changes.publish(data);
        source
    }
    pub fn new(devices: Vec<(String, String, Arc<StationClient>)>) -> Arc<Self> {
        let state = ResourcesData {
            devices: devices
                .iter()
                .map(|(id, name, _)| ResourceDevice {
                    id: id.clone(),
                    name: name.clone(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        Arc::new(Self {
            clients: Mutex::new(
                devices
                    .into_iter()
                    .map(|(id, _, client)| (id, 0, client))
                    .collect(),
            ),
            next: AtomicU64::new(1),
            changes: Observable::new(state.clone()),
            state: Mutex::new(state),
        })
    }
    pub fn snapshot(&self) -> Arc<ResourcesData> {
        self.changes.read()
    }
    pub fn subscribe(&self) -> Subscription<ResourcesData> {
        self.changes.subscribe()
    }
    pub fn replace_devices(&self, devices: Vec<(String, String, Arc<StationClient>)>) {
        let mut clients = self.clients.lock().unwrap();
        let mut state = self.state.lock().unwrap();
        let invalidated = clients.iter().any(|(id, _, old)| {
            !devices
                .iter()
                .any(|(next, _, client)| next == id && old.same_connection(client))
        });
        let mut next_clients = vec![];
        let mut next_devices = vec![];
        for (id, name, client) in devices {
            let same = clients
                .iter()
                .find(|(old, _, source)| old == &id && source.same_connection(&client));
            let generation = same
                .map(|(_, g, _)| *g)
                .unwrap_or_else(|| self.next.fetch_add(1, Ordering::Relaxed));
            let mut device = same
                .and_then(|_| state.devices.iter().find(|d| d.id == id))
                .cloned()
                .unwrap_or_default();
            if same.is_none() {
                state.inspections.retain(|(node, _), _| node != &id);
            }
            device.id = id.clone();
            device.name = name;
            next_clients.push((id, generation, client));
            next_devices.push(device);
        }
        state
            .inspections
            .retain(|(node, _), _| next_devices.iter().any(|d| &d.id == node));
        state.devices = next_devices;
        *clients = next_clients;
        if invalidated {
            self.changes.invalidate(state.clone());
        } else {
            self.changes.publish(state.clone());
        }
    }
    pub async fn refresh(&self) {
        self.refresh_scope(None).await;
    }
    pub async fn refresh_scope(&self, peer: Option<&str>) {
        let clients = self.clients.lock().unwrap().clone();
        let requests = {
            let mut state = self.state.lock().expect("resource inventory");
            let mut requests = vec![];
            for device in &mut state.devices {
                if !device.loading && peer.is_none_or(|peer| peer == device.id) {
                    if let Some((_, generation, client)) =
                        clients.iter().find(|(id, _, _)| id == &device.id)
                    {
                        device.loading = true;
                        requests.push((device.id.clone(), *generation, client.clone()));
                    }
                }
            }
            self.changes.publish(state.clone());
            requests
        };
        futures_util::future::join_all(requests.into_iter().map(
            |(id, generation, client)| async move {
                let result = client
                    .node_request(reqwest::Method::GET, "/v1/node/resources".into(), None)
                    .await;
                let revoked = result
                    .as_ref()
                    .err()
                    .is_some_and(|error| error.access_revoked() || error.status() == Some(403));
                let result = result.map_err(|e| e.to_string()).and_then(|value| {
                    serde_json::from_value::<ResourceCatalog>(value)
                        .map_err(|e| format!("Invalid resource inventory: {e}"))
                });
                let mut clients = self.clients.lock().unwrap();
                if !clients
                    .iter()
                    .any(|(node, g, _)| node == &id && *g == generation)
                {
                    return;
                }
                let mut state = self.state.lock().expect("resource inventory");
                if let Some(device) = state.devices.iter_mut().find(|device| device.id == id) {
                    device.loading = false;
                    let replaced_origin = result.as_ref().ok().is_some_and(|catalog| {
                        device
                            .catalog
                            .as_ref()
                            .is_some_and(|old| old.origin != catalog.origin)
                    });
                    match result {
                        Ok(mut catalog) => {
                            if let Some(old) = &device.catalog {
                                if old.origin == catalog.origin {
                                    for item in &old.items {
                                        if catalog
                                            .issues
                                            .iter()
                                            .any(|issue| issue.kind == item.kind)
                                            && !catalog.items.iter().any(|new| {
                                                new.kind == item.kind && new.id == item.id
                                            })
                                        {
                                            catalog.items.push(item.clone());
                                        }
                                    }
                                }
                            }
                            device.catalog = Some(catalog);
                            device.error = None;
                        }
                        Err(error) => {
                            device.error = Some(error);
                            if revoked {
                                device.catalog = None;
                            }
                        }
                    }
                    if revoked {
                        if let Some((_, generation, _)) =
                            clients.iter_mut().find(|(node, _, _)| node == &id)
                        {
                            *generation = self.next.fetch_add(1, Ordering::Relaxed);
                        }
                        state.inspections.retain(|(node, _), _| node != &id);
                        self.changes.invalidate(state.clone());
                    } else {
                        let catalog = state
                            .devices
                            .iter()
                            .find(|device| device.id == id)
                            .and_then(|device| device.catalog.as_ref());
                        let missing = state
                            .inspections
                            .keys()
                            .filter(|(node, query)| {
                                node == &id
                                    && (replaced_origin
                                        || catalog.is_some_and(|catalog| {
                                            let resource = match query {
                                                Inspection::Mcp(id) => {
                                                    Some((ResourceKind::Mcp, id))
                                                }
                                                Inspection::Service { id, .. } => {
                                                    Some((ResourceKind::Service, id))
                                                }
                                                _ => None,
                                            };
                                            resource.is_some_and(|(kind, id)| {
                                                !catalog
                                                    .items
                                                    .iter()
                                                    .any(|item| item.kind == kind && &item.id == id)
                                            })
                                        }))
                            })
                            .cloned()
                            .collect::<Vec<_>>();
                        if replaced_origin {
                            if let Some((_, generation, _)) =
                                clients.iter_mut().find(|(node, _, _)| node == &id)
                            {
                                *generation = self.next.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        for key in missing {
                            state.inspections.insert(
                                key,
                                InspectionState {
                                    error: Some("Resource is no longer registered".into()),
                                    ..Default::default()
                                },
                            );
                        }
                        if replaced_origin {
                            self.changes.invalidate(state.clone());
                        } else {
                            self.changes.publish(state.clone());
                        }
                    }
                }
            },
        ))
        .await;
    }

    pub(crate) fn revoke(&self, peer: &str) {
        let mut clients = self.clients.lock().unwrap();
        let had_client = clients.iter().any(|(id, _, _)| id == peer);
        clients.retain(|(id, _, _)| id != peer);
        let mut state = self.state.lock().unwrap();
        if !had_client
            && !state.inspections.keys().any(|(id, _)| id == peer)
            && !state
                .devices
                .iter()
                .any(|d| d.id == peer && d.catalog.is_some())
        {
            return;
        }
        state.inspections.retain(|(id, _), _| id != peer);
        if let Some(device) = state.devices.iter_mut().find(|d| d.id == peer) {
            device.catalog = None;
            device.loading = false;
            device.error = Some("设备访问权限已撤销".into());
        }
        self.changes.invalidate(state.clone());
    }

    pub async fn inspect(&self, node: &str, query: Inspection) {
        let source = self
            .clients
            .lock()
            .unwrap()
            .iter()
            .find(|(id, _, _)| id == node)
            .cloned();
        let Some((_, generation, client)) = source else {
            return;
        };
        let key = (node.to_owned(), query.clone());
        let pinned_skill = if let Inspection::Skill { agent, skill, .. } = &query {
            let state = self.state.lock().unwrap();
            state
                .inspection(node, &Inspection::AgentSkills(agent.clone()))
                .and_then(|state| state.content.as_deref())
                .and_then(|content| match content {
                    InspectionContent::Skills(catalog) => {
                        catalog.skills.iter().find(|entry| &entry.id == skill)
                    }
                    _ => None,
                })
                .map(|entry| entry.path.clone())
        } else {
            None
        };
        {
            let mut state = self.state.lock().unwrap();
            if !state.inspections.contains_key(&key) && state.inspections.len() >= 64 {
                let stale = state
                    .inspections
                    .iter()
                    .find(|(_, value)| !value.loading)
                    .map(|(key, _)| key.clone());
                if let Some(stale) = stale {
                    state.inspections.remove(&stale);
                } else {
                    return;
                }
            }
            let entry = state.inspections.entry(key.clone()).or_default();
            if entry.loading {
                return;
            }
            entry.loading = true;
            entry.error = None;
            self.changes.publish(state.clone());
        }
        let result = async {
            let mut path = inspection_path(&query)?;
            if let Some(reference) = pinned_skill.filter(|path| path.starts_with("synch://")) {
                let mut url = reqwest::Url::parse(&format!("http://node.invalid{path}"))?;
                url.query_pairs_mut().append_pair("reference", &reference);
                path = format!("{}?{}", url.path(), url.query().unwrap());
            }
            let value = client
                .node_request(reqwest::Method::GET, path, None)
                .await?;
            Ok::<_, anyhow::Error>(match query {
                Inspection::AgentSkills(_) => {
                    InspectionContent::Skills(serde_json::from_value(value)?)
                }
                _ => InspectionContent::Details(serde_json::from_value(value)?),
            })
        }
        .await;
        let mut clients = self.clients.lock().unwrap();
        if !clients
            .iter()
            .any(|(id, g, _)| id == node && *g == generation)
        {
            return;
        }
        let revoked = result
            .as_ref()
            .err()
            .and_then(|error| error.downcast_ref::<crate::api::ApiError>())
            .is_some_and(|error| error.access_revoked() || error.status() == Some(403));
        let mut state = self.state.lock().unwrap();
        let entry = state.inspections.entry(key).or_default();
        entry.loading = false;
        match result {
            Ok(value) => {
                entry.content = Some(Arc::new(value));
                entry.error = None;
            }
            Err(error) => {
                entry.error = Some(error.to_string());
                if error
                    .downcast_ref::<crate::api::ApiError>()
                    .is_some_and(|e| {
                        e.status().is_some_and(|status| {
                            (400..500).contains(&status) && !matches!(status, 408 | 429)
                        })
                    })
                {
                    entry.content = None;
                }
            }
        }
        if revoked {
            if let Some((_, generation, _)) = clients.iter_mut().find(|(id, _, _)| id == node) {
                *generation = self.next.fetch_add(1, Ordering::Relaxed);
            }
            state.inspections.retain(|(owner, _), _| owner != node);
            if let Some(device) = state.devices.iter_mut().find(|device| device.id == node) {
                device.catalog = None;
                device.loading = false;
                device.error = Some("Access revoked".into());
            }
            self.changes.invalidate(state.clone());
        } else {
            self.changes.publish(state.clone());
        }
    }
}

/// A detail destination comes from the tool's structured invocation arguments.
/// Error text and human summaries never determine a resource identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectionTarget {
    pub origin: Option<String>,
    pub query: Inspection,
}
pub fn history_target(entry: &zork_client_types::history::Entry) -> Option<InspectionTarget> {
    if !matches!(entry.state.as_str(), "failed" | "timed_out" | "interrupted") {
        return None;
    }
    let call = entry.raw.iter().find(|raw| {
        raw["tool"].as_str() == Some(entry.action.as_str()) && raw["arguments"].is_object()
    })?;
    let args = &call["arguments"];
    let query = match entry.action.as_str() {
        "mcp.call" | "mcp.inspect" | "mcp.enable" | "mcp.disable" | "mcp.update" | "mcp.share" => {
            Inspection::Mcp(args["server_id"].as_str()?.into())
        }
        "service.inspect" | "service.restart" | "service.stop" | "service.share"
        | "service.unshare" => Inspection::Service {
            id: args["id"].as_str()?.into(),
            log: None,
        },
        _ => return None,
    };
    inspection_path(&query).ok()?;
    Some(InspectionTarget {
        origin: args["target"].as_str().map(str::to_owned),
        query,
    })
}

fn inspection_path(query: &Inspection) -> anyhow::Result<String> {
    let valid = |id: &str| crate::model_edit::valid_id(id);
    let (path, parameter) = match query {
        Inspection::AgentSkills(agent) => {
            valid(agent)?;
            (format!("/v1/node/agents/{agent}/skills/catalog"), None)
        }
        Inspection::Skill { agent, skill, file } => {
            valid(agent)?;
            valid(skill)?;
            (
                format!("/v1/node/agents/{agent}/skills/{skill}"),
                file.as_ref().map(|f| ("file", f)),
            )
        }
        Inspection::Mcp(id) => {
            valid(id)?;
            (format!("/v1/node/resources/mcp/{id}"), None)
        }
        Inspection::Service { id, log } => {
            valid(id)?;
            (
                format!("/v1/node/resources/service/{id}"),
                log.as_ref().map(|f| ("log", f)),
            )
        }
    };
    if let Some((key, value)) = parameter {
        let mut url = reqwest::Url::parse("http://node.invalid")?;
        url.query_pairs_mut().append_pair(key, value);
        Ok(format!("{path}?{}", url.query().unwrap_or_default()))
    } else {
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    #[tokio::test]
    async fn inspection_result_from_an_old_connection_cannot_restore_removed_data() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (ready, started) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 8192];
            let n = socket.read(&mut request).unwrap();
            assert!(
                String::from_utf8_lossy(&request[..n])
                    .starts_with("GET /v1/node/resources/mcp/server ")
            );
            ready.send(()).unwrap();
            wait.recv().unwrap();
            let body = serde_json::to_string(&ResourceDetails {
                title: "Old private connection".into(),
                ..Default::default()
            })
            .unwrap();
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        });
        let core = Resources::new(vec![(
            "node".into(),
            "Device".into(),
            Arc::new(StationClient::new(format!("http://{address}"), None)),
        )]);
        let request = core.clone();
        let task = tokio::spawn(async move {
            request
                .inspect("node", Inspection::Mcp("server".into()))
                .await
        });
        started.await.unwrap();
        assert!(
            core.snapshot()
                .inspection("node", &Inspection::Mcp("server".into()))
                .unwrap()
                .loading
        );
        core.replace_devices(vec![]);
        release.send(()).unwrap();
        task.await.unwrap();
        server.join().unwrap();
        assert!(core.snapshot().devices.is_empty());
        assert!(core.snapshot().inspections.is_empty());
    }
    #[test]
    fn failure_details_use_structured_identity_not_error_text() {
        let mut entry = zork_client_types::history::Entry {
            id: "call".into(),
            lane: 2,
            action: "mcp.call".into(),
            summary: "server_id: wrong".into(),
            start: None,
            end: None,
            state: "failed".into(),
            raw: vec![
                serde_json::json!({"tool":"mcp.call","arguments":{"target":"key:remote","server_id":"server-one"}}),
            ],
            usage: None,
            model: None,
            outcome_summary: None,
        };
        assert_eq!(
            history_target(&entry),
            Some(InspectionTarget {
                origin: Some("key:remote".into()),
                query: Inspection::Mcp("server-one".into())
            })
        );
        entry.raw.clear();
        assert!(history_target(&entry).is_none());
        entry
            .raw
            .push(serde_json::json!({"tool":"mcp.call","arguments":{"server_id":"../private"}}));
        assert!(history_target(&entry).is_none());
    }
    #[tokio::test]
    async fn offline_refresh_keeps_stale_data_but_revocation_clears_it() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for (status, body) in [
                (200, r#"{"origin":"key:node","items":[],"issues":[]}"#),
                (503, r#"{"error":"offline"}"#),
                (403, r#"{"error":"revoked"}"#),
            ] {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = [0u8; 8192];
                let n = socket.read(&mut request).unwrap();
                assert!(
                    String::from_utf8_lossy(&request[..n]).starts_with("GET /v1/node/resources ")
                );
                write!(socket,"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        let core = Resources::new(vec![(
            "node".into(),
            "Device".into(),
            Arc::new(StationClient::new(format!("http://{address}"), None)),
        )]);
        core.refresh().await;
        assert!(core.snapshot().devices[0].catalog.is_some());
        core.refresh().await;
        assert!(core.snapshot().devices[0].catalog.is_some());
        assert!(core.snapshot().devices[0].error.is_some());
        core.state.lock().unwrap().inspections.insert(
            ("node".into(), Inspection::Mcp("server".into())),
            InspectionState {
                content: Some(Arc::new(InspectionContent::Details(
                    ResourceDetails::default(),
                ))),
                ..Default::default()
            },
        );
        core.refresh().await;
        assert!(core.snapshot().inspections.is_empty());
        assert!(core.snapshot().devices[0].catalog.is_none());
        assert!(!core.snapshot().devices[0].loading);
        server.join().unwrap();
    }
}
