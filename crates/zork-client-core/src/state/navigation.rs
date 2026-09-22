//! Flat device → Chat navigation, including historical Agent homes.
use super::DeviceData;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use zork_client_types::chat::{Author, AuthorKind, Channel};

pub use zork_client_types::navigation::NavigationChat;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct NavigationData {
    pub online: Option<bool>,
    #[serde(skip)]
    pub route: crate::api::ConnectionRoute,
    pub chats: Arc<Vec<NavigationChat>>,
    pub unread: Arc<HashSet<String>>,
}

fn string(value: &Value, field: &str) -> String {
    value[field].as_str().unwrap_or_default().to_owned()
}

impl NavigationData {
    /// Shared projection, also used by disconnected platform fixtures.
    pub fn project(data: &DeviceData, unread: HashSet<String>) -> Self {
        if data.revoked {
            return Self::default();
        }
        let definitions: HashMap<_, _> = data
            .agents
            .iter()
            .filter_map(|agent| Some((agent["id"].as_str()?, agent)))
            .collect();
        let sessions: HashMap<_, _> = data
            .sessions
            .iter()
            .map(|session| (session.session_id.as_str(), session))
            .collect();
        let legacy: HashMap<_, _> = data
            .tasks
            .iter()
            .flat_map(|(creator, tasks)| {
                tasks
                    .iter()
                    .filter_map(move |task| Some((task.session_id.as_deref()?, (creator, task))))
            })
            .collect();
        let fallback;
        let channels = if let Some(chats) = &data.chats {
            chats.as_slice()
        } else {
            // Older nodes retain their public session aliases. This is the
            // only compatibility mapping; platforms never rebuild this list.
            let mut by_id = HashMap::new();
            for session in data.sessions.iter().filter(|s| s.kind != "agent_control") {
                let task = legacy.get(session.session_id.as_str());
                let title = session
                    .title
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .or_else(|| task.map(|(_, task)| task.title.as_str()))
                    .unwrap_or_default();
                by_id.insert(
                    session.session_id.clone(),
                    Channel {
                        chat_id: session.session_id.clone(),
                        title: title.into(),
                        created_at: task.map(|(_, t)| t.created_at.clone()).unwrap_or_default(),
                        last_message_at: task.map(|(_, t)| t.updated_at.clone()),
                        message_count: 0,
                        creator: task.map(|(id, _)| Author {
                            id: (*id).clone(),
                            kind: AuthorKind::Agent,
                            name: definitions.get(id.as_str()).map(|a| string(a, "name")),
                        }),
                    },
                );
            }
            for (id, (creator, task)) in &legacy {
                by_id.entry((*id).to_owned()).or_insert_with(|| Channel {
                    chat_id: (*id).into(),
                    title: task.title.clone(),
                    created_at: task.created_at.clone(),
                    last_message_at: Some(task.updated_at.clone()),
                    message_count: 0,
                    creator: Some(Author {
                        id: (*creator).clone(),
                        kind: AuthorKind::Agent,
                        name: definitions.get(creator.as_str()).map(|a| string(a, "name")),
                    }),
                });
            }
            fallback = by_id.into_values().collect::<Vec<_>>();
            &fallback
        };
        let mut others = Vec::new();
        // Keep ordering deterministic when the source catalog arrives in another order.
        let mut ordered = channels.iter().collect::<Vec<_>>();
        ordered.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
        for channel in ordered {
            if sessions
                .get(channel.chat_id.as_str())
                .and_then(|s| s.task.as_ref())
                .and_then(|task| task.mesh.as_ref())
                .is_some_and(|mesh| mesh.role == "executor")
            {
                continue;
            }
            let old = legacy.get(channel.chat_id.as_str()).map(|(_, task)| *task);
            let executor = old.and_then(|task| task.mesh.as_ref()).and_then(|mesh| {
                data.mesh
                    .peers
                    .iter()
                    .find(|peer| peer.origin == mesh.executor_origin)
                    .map(|peer| peer.name.clone())
            });
            let chat = NavigationChat {
                chat_id: channel.chat_id.clone(),
                title: channel.title.clone(),
                description: old.map(|t| t.goal.clone()).unwrap_or_default(),
                workspace: old
                    .map(|t| t.workspace.clone())
                    .or_else(|| {
                        sessions
                            .get(channel.chat_id.as_str())
                            .map(|s| s.workspace.clone())
                    })
                    .unwrap_or_default(),
                updated_at: channel
                    .last_message_at
                    .as_ref()
                    .unwrap_or(&channel.created_at)
                    .clone(),
                unread: unread.contains(&channel.chat_id),
                can_send: sessions
                    .get(channel.chat_id.as_str())
                    .is_some_and(|s| crate::conversation::can_send(s)),
                can_stop: sessions
                    .get(channel.chat_id.as_str())
                    .is_some_and(|s| crate::composer::can_stop(s)),
                executor,
                model: sessions
                    .get(channel.chat_id.as_str())
                    .map(|session| session.model.clone())
                    .unwrap_or_default(),
                in_preview: false,
            };
            others.push(chat);
        }
        let sort = |items: &mut Vec<NavigationChat>| {
            items.sort_by(|a, b| {
                b.unread
                    .cmp(&a.unread)
                    .then_with(|| b.updated_at.cmp(&a.updated_at))
                    .then_with(|| a.chat_id.cmp(&b.chat_id))
            });
            for (index, chat) in items.iter_mut().enumerate() {
                chat.in_preview = index < 10 || chat.unread;
            }
        };
        sort(&mut others);
        Self {
            online: data.online,
            route: data.route,
            chats: Arc::new(others),
            unread: Arc::new(unread),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chat(id: &str, creator: Option<&str>) -> Channel {
        Channel {
            chat_id: id.into(),
            title: id.into(),
            created_at: id.into(),
            last_message_at: None,
            message_count: 0,
            creator: creator.map(|id| Author {
                id: id.into(),
                kind: AuthorKind::Agent,
                name: Some(format!("name-{id}")),
            }),
        }
    }
    fn data(chats: Vec<Channel>) -> DeviceData {
        let mut state = DeviceData::default();
        state.agents = Arc::new(vec![
            json!({"id":"a","name":"A","role":"leader","session_id":"home-a"}),
            json!({"id":"worker","name":"Worker","role":"worker","session_id":"empty-home"}),
            json!({"id":"b","name":"B","role":"leader"}),
        ]);
        state.chats = Some(Arc::new(chats));
        state
    }
    #[test]
    fn existing_homes_and_all_creators_share_one_chat_list() {
        let state = data(vec![
            chat("home-a", None),
            chat("empty-home", None),
            chat("task", Some("a")),
            chat("independent", None),
        ]);
        let nav = NavigationData::project(&state, HashSet::new());
        assert_eq!(
            nav.chats
                .iter()
                .map(|c| c.chat_id.as_str())
                .collect::<Vec<_>>(),
            ["task", "independent", "home-a", "empty-home"]
        );
        let json = serde_json::to_value(&nav).unwrap();
        assert!(json.get("agents").is_none());
        assert!(json.get("tasks").is_none());
    }
    #[test]
    fn removed_and_remote_creators_do_not_hide_their_chats() {
        let nav = NavigationData::project(
            &data(vec![
                chat("task", Some("worker")),
                chat("other", Some("peer/removed")),
            ]),
            HashSet::new(),
        );
        assert_eq!(
            nav.chats
                .iter()
                .map(|c| c.chat_id.as_str())
                .collect::<Vec<_>>(),
            ["task", "other"]
        );
    }
    #[test]
    fn unread_promotes_the_chat_independently_of_creator() {
        let nav = NavigationData::project(
            &data(vec![chat("old", Some("b")), chat("new", Some("a"))]),
            HashSet::from(["old".into()]),
        );
        assert_eq!(nav.chats[0].chat_id, "old");
        assert!(nav.chats[0].unread);
        assert_eq!(nav.chats[1].chat_id, "new");
        assert_eq!(
            serde_json::to_value(&nav).unwrap()["chats"][0]["chat_id"],
            "old"
        );
    }
    #[test]
    fn legacy_catalog_keeps_closed_tasks_and_preview_never_hides_unread() {
        let mut state = data(vec![]);
        state.chats = None;
        let legacy: crate::api::ProductTask = serde_json::from_value(json!({
            "task_id":"old-task","session_id":"legacy-chat","conversation_id":"old-alias",
            "title":"Archived work","workspace":"workspace","state":"completed","revision":4,
            "result_message_id":null,"result_text":null,"last_run_status":"finished",
            "run_count":1,"goal":"Original work","created_at":"old","updated_at":"old"
        }))
        .unwrap();
        state.tasks = Arc::new(HashMap::from([("a".into(), vec![legacy])]));
        let nav = NavigationData::project(&state, HashSet::new());
        assert_eq!(nav.chats[0].chat_id, "legacy-chat");
        assert_eq!(nav.chats[0].title, "Archived work");
        let modern = data(
            (0..1000)
                .map(|index| chat(&format!("chat-{index:04}"), Some("a")))
                .collect(),
        );
        let nav = NavigationData::project(&modern, HashSet::from(["chat-0000".into()]));
        assert_eq!(nav.chats.len(), 1000);
        assert_eq!(nav.chats[0].chat_id, "chat-0000");
        assert_eq!(nav.chats.iter().filter(|chat| chat.in_preview).count(), 10);
        assert!(nav.chats[0].in_preview);
    }
}
