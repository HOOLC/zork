//! Device-local notification policy. OS delivery attempts are not read receipts.
pub mod mobile;
use crate::{
    api::{Role, TaskState},
    state::DeviceData,
    store::ClientStore,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const KEY: &str = "notification-ledger-v1";
pub(crate) const PREFERENCES: &str = "notification-preferences-v1";
const MAX_AGE_MS: u64 = 10 * 60 * 1000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub enabled: bool,
    pub preview: bool,
    pub sound: bool,
    /// Scoped by node and conversation, never by a display name.
    pub muted: BTreeSet<(String, String)>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            enabled: true,
            preview: false,
            sound: true,
            muted: BTreeSet::new(),
        }
    }
}
pub fn preferences(store: &ClientStore) -> Result<Preferences> {
    Ok(store.get("device", PREFERENCES)?.unwrap_or_default())
}
pub fn save_preferences(store: &ClientStore, value: &Preferences) -> Result<()> {
    if preferences(store)? == *value {
        return Ok(());
    }
    store.put("device", PREFERENCES, value)?;
    let devices: Vec<_> = store
        .1
        .lock()
        .unwrap()
        .values()
        .filter_map(std::sync::Weak::upgrade)
        .collect();
    for device in devices {
        device.refresh_notifications();
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Reply,
    Review,
    Attention,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Notice {
    pub id: String,
    pub node: String,
    pub session: Option<String>,
    pub task: Option<String>,
    pub leader: Option<String>,
    pub title: String,
    pub kind: Kind,
    pub created_at_ms: u64,
}
impl Notice {
    pub fn tag(&self) -> String {
        let target = if let Some(session) = &self.session {
            ("session", session.as_str())
        } else {
            ("task", self.task.as_deref().unwrap_or_default())
        };
        serde_json::to_string(&("zork-notification-v1", &self.node, target)).unwrap()
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Ledger {
    initialized: bool,
    preview: bool,
    messages: BTreeMap<String, String>,
    tasks: BTreeMap<String, String>,
    pub pending: BTreeMap<String, Notice>,
    /// Persist routing for OS-accepted requests across an application restart.
    pub presented: BTreeMap<String, Notice>,
}
impl Ledger {
    pub fn reconcile(
        &mut self,
        node: &str,
        data: &DeviceData,
        reading: Option<&str>,
        prefs: &Preferences,
        now: u64,
    ) {
        if data.revoked {
            *self = Self::default();
            return;
        }
        if data.online != Some(true) || !data.sessions_loaded || !data.inbox_loaded {
            return;
        }
        let mut candidates = Vec::new();
        let mut all_tasks = BTreeMap::new();
        for (leader, tasks) in data.tasks.iter() {
            for task in tasks {
                all_tasks.insert(task.task_id.as_str(), (task, Some(leader.as_str())));
            }
        }
        for task in data
            .sessions
            .iter()
            .filter_map(|session| session.task.as_ref())
        {
            all_tasks
                .entry(task.task_id.as_str())
                .or_insert((task, None));
        }
        for task in data.inbox.iter() {
            all_tasks
                .entry(task.task_id.as_str())
                .or_insert((task, None));
        }
        let by_session: BTreeMap<_, _> = all_tasks
            .values()
            .filter_map(|(task, leader)| {
                task.session_id
                    .as_deref()
                    .map(|session| (session, (*task, *leader)))
            })
            .collect();
        let agents: BTreeMap<_, _> = data
            .agents
            .iter()
            .filter_map(|agent| agent["session_id"].as_str().map(|session| (session, agent)))
            .collect();
        let message_ids: BTreeSet<_> = data
            .read_markers
            .iter()
            .map(|marker| marker.session_id.as_str())
            .collect();
        for marker in data.read_markers.iter() {
            let previous = self
                .messages
                .insert(marker.session_id.clone(), marker.last_message_id.clone());
            if self.initialized
                && previous.as_ref() != Some(&marker.last_message_id)
                && marker.role == Role::Assistant
                // The legacy unread projection maps worker assignment/rework inputs
                // to Assistant. Those reserved IDs are not delivered assistant replies.
                && !marker.last_message_id.starts_with("assignment-")
                && !marker.last_message_id.starts_with("rework-")
            {
                let fresh = chrono::DateTime::parse_from_rfc3339(&marker.created_at)
                    .ok()
                    .and_then(|t| u64::try_from(t.timestamp_millis()).ok())
                    .is_some_and(|at| {
                        at <= now.saturating_add(60_000) && now.saturating_sub(at) <= MAX_AGE_MS
                    });
                if !fresh {
                    continue;
                }
                let task = by_session
                    .get(marker.session_id.as_str())
                    .map(|(task, _)| *task);
                // The owner presents remote work; executor mirrors must not alert twice.
                if task.is_some_and(|task| {
                    task.mesh
                        .as_ref()
                        .is_some_and(|mesh| mesh.role == "executor")
                }) {
                    continue;
                }
                let leader = agents.get(marker.session_id.as_str());
                candidates.push(Notice {
                    id: serde_json::to_string(&(
                        "message",
                        &marker.session_id,
                        &marker.last_message_id,
                    ))
                    .unwrap(),
                    node: node.into(),
                    session: Some(marker.session_id.clone()),
                    task: task.map(|t| t.task_id.clone()),
                    leader: by_session
                        .get(marker.session_id.as_str())
                        .and_then(|(_, leader)| *leader)
                        .or_else(|| leader.and_then(|a| a["id"].as_str()))
                        .map(str::to_owned),
                    title: task
                        .map(|t| t.title.clone())
                        .or_else(|| leader.and_then(|a| a["name"].as_str()).map(str::to_owned))
                        .unwrap_or_default(),
                    kind: Kind::Reply,
                    created_at_ms: now,
                });
            }
        }
        let task_ids: BTreeSet<_> = all_tasks.keys().copied().collect();
        for (task, leader) in all_tasks.values() {
            let kind = if task.state.is_closed()
                || task.last_run_status.as_deref() == Some("running")
                || task
                    .mesh
                    .as_ref()
                    .is_some_and(|mesh| mesh.role == "executor")
            {
                None
            } else if task.state == TaskState::Review {
                Some(Kind::Review)
            } else if task.last_run_status.as_deref() == Some("failed")
                || task
                    .mesh
                    .as_ref()
                    .is_some_and(|m| m.state == "needs_attention")
            {
                Some(Kind::Attention)
            } else {
                None
            };
            let fingerprint =
                serde_json::to_string(&(kind, task.run_count, &task.result_message_id)).unwrap();
            let previous = self.tasks.insert(task.task_id.clone(), fingerprint.clone());
            if let Some(kind) = kind {
                if self.initialized && previous.as_ref() != Some(&fingerprint) {
                    candidates.push(Notice {
                        id: serde_json::to_string(&("task", &task.task_id, &fingerprint)).unwrap(),
                        node: node.into(),
                        session: task.session_id.clone(),
                        task: Some(task.task_id.clone()),
                        leader: leader.map(str::to_owned),
                        title: task.title.clone(),
                        kind,
                        created_at_ms: now,
                    });
                }
            } else {
                self.pending
                    .retain(|_, n| n.task.as_ref() != Some(&task.task_id) || n.kind == Kind::Reply);
                self.presented
                    .retain(|_, n| n.task.as_ref() != Some(&task.task_id) || n.kind == Kind::Reply);
            }
        }
        self.messages
            .retain(|id, _| message_ids.contains(id.as_str()));
        self.tasks.retain(|id, _| task_ids.contains(id.as_str()));
        let exists = |n: &Notice| {
            n.task.as_ref().map_or_else(
                || {
                    n.session
                        .as_ref()
                        .is_some_and(|s| self.messages.contains_key(s))
                },
                |id| task_ids.contains(id.as_str()),
            )
        };
        self.pending.retain(|_, n| exists(n));
        self.presented.retain(|_, n| exists(n));
        self.initialized = true;
        for mut notice in candidates {
            notice.title = notice
                .title
                .chars()
                .filter(|c| !c.is_control())
                .take(120)
                .collect();
            let tag = notice.tag();
            if notice.kind == Kind::Reply {
                if let Some(existing) = self.pending.get(&tag).filter(|n| n.kind != Kind::Reply) {
                    notice.kind = existing.kind;
                }
            }
            self.pending.insert(tag, notice);
        }
        self.filter(node, reading, prefs, now);
        // Bound OS routing history independently of how many messages a session has.
        while self.pending.len() > 32 {
            let key = self
                .pending
                .iter()
                .min_by_key(|(_, n)| n.created_at_ms)
                .unwrap()
                .0
                .clone();
            self.pending.remove(&key);
        }
        self.bound_presented();
    }
    fn bound_presented(&mut self) {
        while self.presented.len() > 128 {
            let key = self
                .presented
                .iter()
                .min_by_key(|(_, n)| n.created_at_ms)
                .unwrap()
                .0
                .clone();
            self.presented.remove(&key);
        }
    }
    pub fn filter(&mut self, node: &str, reading: Option<&str>, prefs: &Preferences, now: u64) {
        // Hide already accepted previews immediately when the user changes privacy.
        if self.preview && !prefs.preview {
            self.presented.clear();
        }
        self.preview = prefs.preview;
        let keep = |n: &Notice| {
            prefs.enabled
                && !n.session.as_ref().is_some_and(|s| {
                    prefs.muted.contains(&(node.to_owned(), s.clone()))
                        || reading == Some(s.as_str())
                })
        };
        self.pending
            .retain(|_, n| keep(n) && now.saturating_sub(n.created_at_ms) <= MAX_AGE_MS);
        self.presented.retain(|_, n| keep(n));
    }
    pub fn acknowledge(&mut self, id: &str) {
        if let Some(tag) = self
            .pending
            .iter()
            .find(|(_, n)| n.id == id)
            .map(|(tag, _)| tag.clone())
        {
            if let Some(notice) = self.pending.remove(&tag) {
                self.presented.insert(tag, notice);
                self.bound_presented();
            }
        }
    }
}

/// Only persisted notifications for currently configured, authorized nodes can route.
pub fn resolve(store: &ClientStore, tag: &str) -> Result<Option<Notice>> {
    for node in store.nodes()? {
        if store.replica_revoked(&node.id)? {
            continue;
        }
        if let Some(ledger) = store.get::<Ledger>(&node.id, KEY)? {
            if let Some(notice) = ledger
                .presented
                .get(tag)
                .or_else(|| ledger.pending.get(tag))
            {
                return Ok(Some(notice.clone()));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ConversationReadMarker;
    use std::sync::Arc;
    fn data(now: u64, id: &str) -> DeviceData {
        let mut data = DeviceData::default();
        data.online = Some(true);
        data.sessions_loaded = true;
        data.inbox_loaded = true;
        data.read_markers = Arc::new(vec![ConversationReadMarker {
            session_id: "session".into(),
            last_message_id: id.into(),
            created_at: chrono::DateTime::from_timestamp_millis(now as i64)
                .unwrap()
                .to_rfc3339(),
            role: Role::Assistant,
        }]);
        data
    }
    #[test]
    fn baseline_restart_duplicate_and_coalescing() {
        let dir = tempfile::tempdir().unwrap();
        let store = ClientStore::open(dir.path()).unwrap();
        let now = crate::store::delivery_now_ms();
        let mut ledger = Ledger::default();
        ledger.reconcile(
            "node",
            &data(now, "old"),
            None,
            &Preferences::default(),
            now,
        );
        assert!(ledger.pending.is_empty());
        ledger.reconcile(
            "node",
            &data(now, "new"),
            None,
            &Preferences::default(),
            now,
        );
        assert_eq!(ledger.pending.len(), 1);
        ledger.reconcile(
            "node",
            &data(now, "newer"),
            None,
            &Preferences::default(),
            now,
        );
        assert_eq!(ledger.pending.len(), 1);
        let notice = ledger.pending.values().next().unwrap().clone();
        ledger.acknowledge(&notice.id);
        store.put("node", KEY, &ledger).unwrap();
        drop(store);
        let store = ClientStore::open(dir.path()).unwrap();
        let mut ledger: Ledger = store.get("node", KEY).unwrap().unwrap();
        ledger.reconcile(
            "node",
            &data(now, "newer"),
            None,
            &Preferences::default(),
            now,
        );
        assert!(ledger.pending.is_empty());
        assert_eq!(ledger.presented.get(&notice.tag()), Some(&notice));
    }
    #[test]
    fn visible_muted_disabled_old_and_revoked_do_not_notify() {
        let now = crate::store::delivery_now_ms();
        let mut ledger = Ledger::default();
        let mut prefs = Preferences::default();
        ledger.reconcile("node", &data(now, "a"), None, &prefs, now);
        ledger.reconcile("node", &data(now, "b"), Some("session"), &prefs, now);
        assert!(ledger.pending.is_empty());
        prefs.muted.insert(("node".into(), "session".into()));
        ledger.reconcile("node", &data(now, "c"), None, &prefs, now);
        assert!(ledger.pending.is_empty());
        prefs.muted.clear();
        prefs.enabled = false;
        ledger.reconcile("node", &data(now, "d"), None, &prefs, now);
        prefs.enabled = true;
        ledger.reconcile("node", &data(now, "d"), None, &prefs, now);
        assert!(ledger.pending.is_empty());
        ledger.reconcile("node", &data(now - MAX_AGE_MS - 1, "e"), None, &prefs, now);
        assert!(ledger.pending.is_empty());
        ledger.reconcile("node", &data(now, "f"), None, &prefs, now);
        assert_eq!(ledger.pending.len(), 1);
        let mut revoked = data(now, "f");
        revoked.revoked = true;
        ledger.reconcile("node", &revoked, None, &prefs, now);
        assert!(ledger.pending.is_empty());
        assert!(ledger.presented.is_empty());
    }
    #[test]
    fn user_messages_never_notify_and_other_nodes_are_not_muted() {
        let now = crate::store::delivery_now_ms();
        let mut ledger = Ledger::default();
        let mut prefs = Preferences::default();
        prefs.muted.insert(("other".into(), "session".into()));
        ledger.reconcile("node", &data(now, "a"), None, &prefs, now);
        let mut user = data(now, "b");
        Arc::make_mut(&mut user.read_markers)[0].role = Role::User;
        ledger.reconcile("node", &user, None, &prefs, now);
        assert!(ledger.pending.is_empty());
        ledger.reconcile("node", &data(now, "c"), None, &prefs, now);
        assert_eq!(ledger.pending.len(), 1);
    }
    #[test]
    fn preferences_survive_reopening_and_default_to_private_content() {
        let dir = tempfile::tempdir().unwrap();
        let store = ClientStore::open(dir.path()).unwrap();
        assert!(!preferences(&store).unwrap().preview);
        let prefs = Preferences {
            enabled: false,
            ..Default::default()
        };
        save_preferences(&store, &prefs).unwrap();
        drop(store);
        assert_eq!(
            preferences(&ClientStore::open(dir.path()).unwrap()).unwrap(),
            prefs
        );
    }
    fn task(now: u64) -> crate::api::ProductTask {
        serde_json::from_value(serde_json::json!({
            "task_id":"task", "session_id":"session", "conversation_id":"conversation", "title":"Result", "workspace":"",
            "state":"open", "revision":1, "run_count":1, "last_run_status":"running", "result_message_id":null, "result_text":null,
            "created_at":now.to_string(), "updated_at":now.to_string()
        })).unwrap()
    }
    #[test]
    fn task_transitions_merge_with_replies_and_resolve_without_repeating_metadata_edits() {
        let now = crate::store::delivery_now_ms();
        let mut data = data(now, "old");
        let mut task = task(now);
        data.tasks = Arc::new(std::collections::HashMap::from([(
            "leader".into(),
            vec![task.clone()],
        )]));
        let mut ledger = Ledger::default();
        let prefs = Preferences::default();
        ledger.reconcile("node", &data, None, &prefs, now);
        task.state = TaskState::Review;
        task.last_run_status = Some("completed".into());
        task.result_message_id = Some("result".into());
        Arc::make_mut(&mut data.tasks).insert("leader".into(), vec![task.clone()]);
        Arc::make_mut(&mut data.read_markers)[0].last_message_id = "reply".into();
        ledger.reconcile("node", &data, None, &prefs, now);
        assert_eq!(ledger.pending.len(), 1);
        let notice = ledger.pending.values().next().unwrap().clone();
        assert_eq!(notice.kind, Kind::Review);
        assert_eq!(notice.leader.as_deref(), Some("leader"));
        Arc::make_mut(&mut data.read_markers)[0].last_message_id = "reply-later-in-burst".into();
        ledger.reconcile("node", &data, None, &prefs, now);
        let notice = ledger.pending.values().next().unwrap().clone();
        assert_eq!(
            notice.kind,
            Kind::Review,
            "a later reply must preserve the pending review reason"
        );
        ledger.acknowledge(&notice.id);
        task.title = "Renamed".into();
        task.revision += 1;
        Arc::make_mut(&mut data.tasks).insert("leader".into(), vec![task.clone()]);
        ledger.reconcile("node", &data, None, &prefs, now);
        assert!(ledger.pending.is_empty());
        task.state = TaskState::Completed;
        Arc::make_mut(&mut data.tasks).insert("leader".into(), vec![task.clone()]);
        ledger.reconcile("node", &data, None, &prefs, now);
        assert!(ledger.presented.is_empty());
        task.state = TaskState::Open;
        task.last_run_status = Some("failed".into());
        task.run_count += 1;
        Arc::make_mut(&mut data.tasks).insert("leader".into(), vec![task.clone()]);
        ledger.reconcile("node", &data, None, &prefs, now);
        assert_eq!(
            ledger.pending.values().next().unwrap().kind,
            Kind::Attention
        );
        task.last_run_status = Some("running".into());
        Arc::make_mut(&mut data.tasks).insert("leader".into(), vec![task]);
        ledger.reconcile("node", &data, None, &prefs, now);
        assert!(ledger.pending.is_empty());
    }
    #[test]
    fn deleting_conversation_or_hiding_previews_withdraws_accepted_notifications() {
        let now = crate::store::delivery_now_ms();
        let mut ledger = Ledger::default();
        let mut prefs = Preferences {
            preview: true,
            ..Default::default()
        };
        ledger.reconcile("node", &data(now, "a"), None, &prefs, now);
        ledger.reconcile("node", &data(now, "b"), None, &prefs, now);
        let id = ledger.pending.values().next().unwrap().id.clone();
        ledger.acknowledge(&id);
        prefs.preview = false;
        ledger.filter("node", None, &prefs, now);
        assert!(ledger.presented.is_empty());
        ledger.reconcile("node", &data(now, "c"), None, &prefs, now);
        let mut empty = data(now, "c");
        empty.read_markers = Default::default();
        ledger.reconcile("node", &empty, None, &prefs, now);
        assert!(ledger.pending.is_empty());
    }
    #[test]
    fn worker_inputs_with_legacy_assistant_markers_are_not_replies() {
        let now = crate::store::delivery_now_ms();
        let mut ledger = Ledger::default();
        let prefs = Preferences::default();
        ledger.reconcile("node", &data(now, "old"), None, &prefs, now);
        for id in ["assignment-123", "rework-123"] {
            ledger.reconcile("node", &data(now, id), None, &prefs, now);
            assert!(ledger.pending.is_empty());
        }
    }
    #[test]
    fn routing_history_is_bounded_even_without_a_following_catalog_commit() {
        let mut ledger = Ledger::default();
        for index in 0..256 {
            let notice = Notice {
                id: index.to_string(),
                node: "node".into(),
                session: Some(format!("session-{index}")),
                task: None,
                leader: None,
                title: String::new(),
                kind: Kind::Reply,
                created_at_ms: index,
            };
            ledger.pending.insert(notice.tag(), notice.clone());
            ledger.acknowledge(&notice.id);
            assert!(ledger.presented.len() <= 128);
        }
        assert_eq!(ledger.presented.len(), 128);
        assert!(ledger.presented.values().all(|n| n.created_at_ms >= 128));
    }
    #[test]
    fn executor_mirrors_do_not_duplicate_the_owners_notification() {
        let now = crate::store::delivery_now_ms();
        let mut data = data(now, "a");
        let mut task = task(now);
        task.mesh = Some(crate::api::MeshTask {
            role: "executor".into(),
            assignment_id: "assignment".into(),
            state: "running".into(),
            error: None,
            owner_origin: "owner".into(),
            executor_origin: "executor".into(),
        });
        data.tasks = Arc::new(std::collections::HashMap::from([(
            "leader".into(),
            vec![task.clone()],
        )]));
        let mut ledger = Ledger::default();
        ledger.reconcile("executor", &data, None, &Preferences::default(), now);
        task.state = TaskState::Review;
        task.last_run_status = Some("completed".into());
        Arc::make_mut(&mut data.tasks).insert("leader".into(), vec![task]);
        Arc::make_mut(&mut data.read_markers)[0].last_message_id = "b".into();
        ledger.reconcile("executor", &data, None, &Preferences::default(), now);
        assert!(ledger.pending.is_empty());
    }
}
