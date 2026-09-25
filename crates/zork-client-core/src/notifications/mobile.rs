//! Mobile notification intents and public OS-delivery projection. The Android
//! host supplies visibility/service lifetime; classification and storage stay here.
use super::{preferences, save_preferences, Ledger, KEY};
use crate::store::ClientStore;
use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};

pub(crate) const BACKGROUND: &str = "notification-background-v1";
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Enabled {
        value: bool,
    },
    Preview {
        value: bool,
    },
    Sound {
        value: bool,
    },
    Background {
        value: bool,
    },
    Mute {
        peer: String,
        session: String,
        value: bool,
    },
}
pub fn settings(store: &ClientStore) -> Result<Value> {
    let prefs = preferences(store)?;
    let background = store.get::<bool>("device", BACKGROUND)?.unwrap_or(false);
    Ok(
        json!({"enabled":prefs.enabled,"preview":prefs.preview,"sound":prefs.sound,
        "muted":prefs.muted,"background":background,
        "service_requested":background && prefs.enabled && !store.nodes()?.is_empty()}),
    )
}
pub fn apply(store: &ClientStore, action: Option<Action>) -> Result<Value> {
    if let Some(action) = action {
        let mut prefs = preferences(store)?;
        match action {
            Action::Enabled { value } => prefs.enabled = value,
            Action::Preview { value } => prefs.preview = value,
            Action::Sound { value } => prefs.sound = value,
            Action::Background { value } => store.put("device", BACKGROUND, &value)?,
            Action::Mute {
                peer,
                session,
                value,
            } => {
                crate::valid_session(&session)?;
                anyhow::ensure!(
                    store.nodes()?.iter().any(|node| node.id == peer),
                    "设备尚未添加"
                );
                if value {
                    prefs.muted.insert((peer, session));
                } else {
                    prefs.muted.remove(&(peer, session));
                }
            }
        }
        save_preferences(store, &prefs)?;
    }
    settings(store)
}
pub fn snapshot(store: &ClientStore) -> Result<Value> {
    let prefs = preferences(store)?;
    let mut pending = vec![];
    let mut retained = vec![];
    for node in store.nodes()? {
        if store.replica_revoked(&node.id)? {
            continue;
        }
        let mut ledger = store.get::<Ledger>(&node.id, KEY)?.unwrap_or_default();
        // Also apply privacy and preferences when a device controller is offline.
        ledger.filter(&node.id, None, &prefs, crate::store::delivery_now_ms());
        for (tag, notice) in &ledger.pending {
            retained.push(tag.clone());
            pending.push(json!({"tag":tag,"id":notice.id,"peer":node.id,"kind":notice.kind,
                "title":if prefs.preview && !notice.title.is_empty() {notice.title.as_str()} else {"Zork"},"sound":prefs.sound}));
        }
        retained.extend(ledger.presented.keys().cloned());
    }
    Ok(json!({"settings":settings(store)?,"pending":pending,"retained":retained}))
}
pub fn resolve(store: &ClientStore, tag: &str) -> Result<Value> {
    if tag == "zork-notification-test" {
        return Ok(json!({"test":true}));
    }
    let notice = super::resolve(store, tag)?
        .ok_or_else(|| anyhow::anyhow!("通知对应的设备或会话已不可用"))?;
    let session = notice
        .session
        .ok_or_else(|| anyhow::anyhow!("任务暂时没有可打开的会话"))?;
    crate::valid_session(&session)?;
    Ok(json!({"peer":notice.node,"session":session,"title":notice.title,"leader":notice.leader}))
}
pub fn test(store: &ClientStore) -> Result<Value> {
    let prefs = preferences(store)?;
    anyhow::ensure!(prefs.enabled, "请先开启接收通知");
    Ok(
        json!({"tag":"zork-notification-test","id":ulid::Ulid::new().to_string(),
        "title":"Zork","kind":"test","sound":prefs.sound}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_is_opt_in_and_requires_notifications_and_a_saved_device() {
        let root = tempfile::tempdir().unwrap();
        let store = ClientStore::open(root.path()).unwrap();
        assert_eq!(settings(&store).unwrap()["service_requested"], false);
        apply(&store, Some(Action::Background { value: true })).unwrap();
        assert_eq!(settings(&store).unwrap()["service_requested"], false);
        store
            .save_node(&crate::store::SavedNode {
                machine_name: None,
                id: "node".into(),
                name: "node".into(),
                url: String::new(),
                token: None,
                local: false,
                mesh: None,
                group: None,
            })
            .unwrap();
        assert_eq!(settings(&store).unwrap()["service_requested"], true);
        apply(&store, Some(Action::Enabled { value: false })).unwrap();
        assert_eq!(settings(&store).unwrap()["service_requested"], false);
    }
}
