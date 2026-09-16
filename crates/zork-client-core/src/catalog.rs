//! Durable catalog projection shared by observable state and serialized clients.
use crate::{
    api::{Artifact, ConversationReadMarker, ProductTask, ProfileInfo, SessionSummary, TaskState},
    store::ClientStore,
};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};
use zork_client_types::sync::{Cursor, Kind};

#[derive(Clone, Debug, serde::Deserialize)]
pub(crate) struct ResourceState {
    pub ready: bool,
    pub error: Option<String>,
}
impl Default for ResourceState {
    fn default() -> Self {
        Self {
            ready: true,
            error: None,
        }
    }
}
pub(crate) struct Catalog {
    pub cursor: Cursor,
    pub info: Value,
    pub profile_state: ResourceState,
    pub agents: Arc<Vec<Value>>,
    pub profiles: Arc<Vec<ProfileInfo>>,
    pub raw_profiles: Arc<Vec<Value>>,
    pub providers: Arc<Vec<Value>>,
    pub sessions: Arc<Vec<SessionSummary>>,
    pub chats: Option<Arc<Vec<zork_client_types::chat::Channel>>>,
    pub by_leader: Arc<HashMap<String, Vec<ProductTask>>>,
    pub all_tasks: Arc<Vec<ProductTask>>,
    pub inbox: Arc<Vec<ProductTask>>,
    pub artifacts: Arc<Vec<Artifact>>,
    pub pages: Arc<crate::pages::PageCatalog>,
    pub markers: Arc<Vec<ConversationReadMarker>>,
}
impl Catalog {
    pub fn read(store: &ClientStore, peer: &str) -> Result<Option<Self>> {
        let Some((cursor, records)) = store.replica_catalog(peer)? else {
            return Ok(None);
        };
        let mut agents = vec![];
        let mut profiles = vec![];
        let mut raw_profiles = vec![];
        let mut info = Value::Null;
        let mut profile_state = ResourceState::default();
        let mut providers = vec![];
        let mut sessions = vec![];
        let mut chats: Vec<zork_client_types::chat::Channel> = vec![];
        let mut chat_catalog = false;
        let mut tasks: Vec<(Option<String>, ProductTask)> = vec![];
        let mut artifacts = vec![];
        let mut pages = crate::pages::PageCatalog::default();
        let mut markers = vec![];
        for record in records {
            let mut value = record.value.context("catalog contains deletion")?;
            value["sync_revision"] = serde_json::json!(record.revision);
            match record.kind {
                Kind::Agent => agents.push(value),
                Kind::Profile => {
                    raw_profiles.push(value.clone());
                    profiles.push(serde_json::from_value::<ProfileInfo>(value)?);
                }
                Kind::Device => info = value,
                Kind::Resource if record.id == "profiles" => {
                    profile_state = serde_json::from_value(value)?
                }
                Kind::Resource => match value["resource_type"].as_str() {
                    Some("chat_catalog") => {
                        anyhow::ensure!(value["schema_version"] == 1, "unsupported_chat_catalog");
                        chat_catalog = true;
                    }
                    Some("chat_summary") => {
                        anyhow::ensure!(value["schema_version"] == 1, "unsupported_chat_catalog");
                        chats.push(serde_json::from_value(value)?);
                    }
                    Some("conversation_page") => {
                        pages.references.push(serde_json::from_value(value)?)
                    }
                    Some("application") => pages.applications.push(serde_json::from_value(value)?),
                    Some("conversation_file") => pages.files.push(serde_json::from_value(value)?),
                    _ => {}
                },
                Kind::Provider => providers.push(value),
                Kind::Session => {
                    let mut value = value;
                    value["status"] = Value::String("wait".into());
                    sessions.push(serde_json::from_value::<SessionSummary>(value)?);
                }
                Kind::Task => {
                    let leader = value["leader_id"].as_str().map(str::to_owned);
                    tasks.push((leader, serde_json::from_value::<ProductTask>(value)?));
                }
                Kind::Artifact => artifacts.push(serde_json::from_value::<Artifact>(value)?),
                Kind::ReadMarker => {
                    markers.push(serde_json::from_value::<ConversationReadMarker>(value)?)
                }
                _ => {}
            }
        }
        crate::pages::merge_message_links(&mut pages, store.message_links(peer)?);
        tasks.sort_by(|a, b| {
            b.1.updated_at
                .cmp(&a.1.updated_at)
                .then_with(|| b.1.task_id.cmp(&a.1.task_id))
        });
        let mut by_leader: HashMap<String, Vec<ProductTask>> = HashMap::new();
        let by_session: HashMap<_, _> = tasks
            .iter()
            .filter_map(|(_, t)| t.session_id.as_ref().map(|id| (id, t)))
            .collect();
        let chat_titles: HashMap<_, _> = chats
            .iter()
            .map(|chat| (&chat.chat_id, &chat.title))
            .collect();
        for session in &mut sessions {
            if let Some(title) = chat_titles.get(&session.session_id) {
                session.title = Some((*title).clone());
            }
            session.task = by_session.get(&session.session_id).map(|t| (*t).clone());
        }
        for (leader, task) in &tasks {
            if let Some(leader) = leader {
                by_leader
                    .entry(leader.clone())
                    .or_default()
                    .push(task.clone());
            }
        }
        let task_titles: HashMap<_, _> = tasks
            .iter()
            .map(|(_, t)| (t.task_id.clone(), t.title.clone()))
            .collect();
        let agent_names: HashMap<_, _> = agents
            .iter()
            .filter_map(|a| {
                Some((
                    a["session_id"].as_str()?.to_owned(),
                    a["name"].as_str()?.to_owned(),
                ))
            })
            .collect();
        let all_tasks = tasks.into_iter().map(|(_, task)| task).collect::<Vec<_>>();
        let inbox = all_tasks
            .iter()
            .filter(|t| {
                !t.state.is_closed()
                    && t.last_run_status.as_deref() != Some("running")
                    && (t.state == TaskState::Review
                        || t.mesh
                            .as_ref()
                            .is_some_and(|m| m.state == "needs_attention")
                        || t.session_id.is_none()
                        || matches!(t.last_run_status.as_deref(), Some("failed" | "cancelled")))
            })
            .cloned()
            .collect();
        for artifact in &mut artifacts {
            artifact.task_title = artifact
                .task_id
                .as_ref()
                .and_then(|id| task_titles.get(id))
                .or_else(|| {
                    artifact
                        .session_id
                        .as_ref()
                        .and_then(|id| agent_names.get(id))
                })
                .cloned()
                .unwrap_or_else(|| "Conversation".into());
        }
        Ok(Some(Self {
            cursor,
            info,
            profile_state,
            agents: Arc::new(agents),
            profiles: Arc::new(profiles),
            raw_profiles: Arc::new(raw_profiles),
            providers: Arc::new(providers),
            sessions: Arc::new(sessions),
            chats: chat_catalog.then(|| Arc::new(chats)),
            by_leader: Arc::new(by_leader),
            all_tasks: Arc::new(all_tasks),
            inbox: Arc::new(inbox),
            artifacts: Arc::new(artifacts),
            pages: Arc::new(pages),
            markers: Arc::new(markers),
        }))
    }
    pub fn supports(path: &str) -> bool {
        let raw = path.split('?').next().unwrap_or(path);
        matches!(
            raw,
            "/v1/node/info"
                | "/v1/node/agents"
                | "/v1/node/chats"
                | "/v1/im/profiles"
                | "/v1/node/providers"
                | "/v1/im/sessions"
                | "/v1/tasks"
                | "/v1/inbox"
                | "/v1/artifacts"
                | "/v1/node/pages"
                | "/v1/node/conversations/read-markers"
        ) || raw
            .strip_prefix("/v1/node/profiles/")
            .is_some_and(|id| !id.is_empty() && !id.contains('/'))
            || raw
                .strip_prefix("/v1/node/agents/")
                .and_then(|p| p.strip_suffix("/tasks"))
                .is_some_and(|id| !id.is_empty() && !id.contains('/'))
    }
    pub fn response(&self, path: &str) -> Result<Option<Value>> {
        let raw = path.split('?').next().unwrap_or(path);
        let value = match raw {
            "/v1/node/info" => self.info.clone(),
            "/v1/node/agents" => json!({"items":self.agents}),
            "/v1/node/chats" => match &self.chats {
                Some(chats) => json!({"schema_version":1,"items":chats}),
                None => return Ok(None),
            },
            "/v1/im/profiles" => json!({"items":self.raw_profiles}),
            "/v1/node/providers" => json!({"providers":self.providers}),
            "/v1/im/sessions" => json!({"items":self.sessions}),
            "/v1/tasks" => json!({"items":self.all_tasks}),
            "/v1/inbox" => json!({"items":self.inbox}),
            "/v1/node/pages" => json!(self.pages),
            "/v1/node/conversations/read-markers" => json!({"items":self.markers}),
            "/v1/artifacts" => {
                let url = reqwest::Url::parse(&format!("http://localhost{path}"))?;
                let task = url
                    .query_pairs()
                    .find(|(k, _)| k == "task_id")
                    .map(|(_, v)| v.into_owned());
                json!({"items":self.artifacts.iter().filter(|a|task.as_ref().is_none_or(|id|a.task_id.as_ref()==Some(id))).collect::<Vec<_>>()})
            }
            _ => {
                if let Some(id) = raw
                    .strip_prefix("/v1/node/profiles/")
                    .filter(|id| !id.contains('/'))
                {
                    self.raw_profiles
                        .iter()
                        .find(|p| p["profile_id"] == id)
                        .cloned()
                        .unwrap_or(Value::Null)
                } else if let Some(id) = raw
                    .strip_prefix("/v1/node/agents/")
                    .and_then(|p| p.strip_suffix("/tasks"))
                    .filter(|id| !id.contains('/'))
                {
                    json!({"items":self.by_leader.get(id).cloned().unwrap_or_default()})
                } else {
                    return Ok(None);
                }
            }
        };
        Ok(Some(value))
    }
}

pub(crate) fn read_response(store: &ClientStore, peer: &str, path: &str) -> Result<Option<Value>> {
    if !Catalog::supports(path) {
        return Ok(None);
    }
    let device = store
        .1
        .lock()
        .unwrap()
        .get(peer)
        .and_then(std::sync::Weak::upgrade);
    if let Some(device) = device {
        if let Some(value) = device.replica_response(path)? {
            return Ok(Some(value));
        }
    }
    Catalog::read(store, peer)?
        .map(|catalog| catalog.response(path))
        .transpose()
        .map(Option::flatten)
}
