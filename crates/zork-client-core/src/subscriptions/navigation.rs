//! Every known device's Chats in one list, merged by real message time. Each
//! device keeps its own `NavigationData` projection; this only merges them.
use super::snapshot::SnapshotWire;
use crate::{
    state::{NavigationChat, NavigationData},
    store::ClientStore,
};
use anyhow::Result;
use chrono::{DateTime, FixedOffset};
use serde_json::{json, Value};
use std::{sync::Arc, task::Poll};
use zork_observe::{Readiness, ValueSource};

/// Visible and archived rows are bounded after ordering, so unread Chats and
/// the newest messages always survive the cut.
const CHAT_LIMIT: usize = 500;
const ARCHIVED_LIMIT: usize = 200;

pub(super) struct NavigationWire {
    pub wire: SnapshotWire,
    task: tokio::task::JoinHandle<()>,
    store: Arc<ClientStore>,
    peers: Vec<String>,
}
impl Drop for NavigationWire {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl NavigationWire {
    pub fn new(directory: Arc<ValueSource<Value>>, store: Arc<ClientStore>) -> Self {
        let mut nodes = directory.subscribe();
        let (inputs, mut sources) = collect(&store, &nodes.snapshot());
        let source = Arc::new(ValueSource::new(merge(&inputs, now())));
        let wire = SnapshotWire::new(&source);
        let db = store.clone();
        let task = tokio::spawn(async move {
            loop {
                let mut signals: Vec<Readiness> = sources.iter().map(|s| s.readiness()).collect();
                signals.push(nodes.readiness());
                tokio::select! {
                    _ = changed(&mut signals) => {}
                    // Date sections move at local midnight without a source change.
                    _ = tokio::time::sleep(until_midnight(now())) => {}
                }
                let (current, next) = collect(&db, &nodes.snapshot());
                sources = next;
                source.publish(merge(&current, now()));
            }
        });
        Self {
            wire,
            task,
            store,
            peers: vec![],
        }
    }
    pub fn valid(&self) -> bool {
        self.wire.valid()
            && self
                .peers
                .iter()
                .all(|peer| !self.store.replica_revoked(peer).unwrap_or(true))
    }
    pub fn prepare(&mut self) -> Result<Option<(Value, bool, bool)>> {
        let batch = self.wire.prepare()?;
        if let Some((value, _, _)) = &batch {
            let snapshot = &value["snapshot"];
            let mut peers: Vec<String> = ["chats", "archived"]
                .iter()
                .flat_map(|list| snapshot[*list].as_array().into_iter().flatten())
                .filter_map(|chat| chat["peer"].as_str().map(str::to_owned))
                .collect();
            peers.sort();
            peers.dedup();
            self.peers = peers;
        }
        Ok(batch)
    }
}

/// Wakes when any source changes or closes; a closed source means its device
/// controller went away, which the next collection reflects.
async fn changed(signals: &mut Vec<Readiness>) {
    futures_util::future::poll_fn(|cx| {
        let mut changed = false;
        signals.retain_mut(|signal| match signal.poll_changed(cx) {
            Poll::Ready(Ok(())) => {
                changed = true;
                true
            }
            Poll::Ready(Err(_)) => {
                changed = true;
                false
            }
            Poll::Pending => true,
        });
        if changed {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await
}

pub(crate) struct Input {
    pub peer: String,
    pub name: String,
    pub navigation: Arc<NavigationData>,
}

/// Directory order and names; each live device controller's navigation. The
/// subscriptions are returned so the caller can wait on exactly these sources.
fn collect(
    store: &ClientStore,
    directory: &Value,
) -> (Vec<Input>, Vec<crate::state::Subscription<NavigationData>>) {
    let mut inputs = Vec::new();
    let mut sources = Vec::new();
    for node in directory["nodes"].as_array().into_iter().flatten() {
        let Some(peer) = node["id"].as_str() else {
            continue;
        };
        let device = store
            .1
            .lock()
            .unwrap()
            .get(peer)
            .and_then(std::sync::Weak::upgrade);
        let Some(device) = device else {
            continue;
        };
        let mut source = device.navigation();
        inputs.push(Input {
            peer: peer.into(),
            name: node["name"].as_str().unwrap_or(peer).into(),
            navigation: source.snapshot(),
        });
        sources.push(source);
    }
    (inputs, sources)
}

fn now() -> DateTime<FixedOffset> {
    chrono::Local::now().fixed_offset()
}

fn until_midnight(now: DateTime<FixedOffset>) -> std::time::Duration {
    let next = now
        .date_naive()
        .succ_opt()
        .and_then(|day| day.and_hms_opt(0, 0, 1))
        .map(|next| (next - now.naive_local()).num_milliseconds().max(1_000))
        .unwrap_or(3_600_000);
    std::time::Duration::from_millis(next as u64)
}

fn millis(chat: &NavigationChat) -> Option<i64> {
    DateTime::parse_from_rfc3339(&chat.updated_at)
        .ok()
        .map(|at| at.timestamp_millis())
}

/// Stable key for the platform's section header. Unread Chats lead the list,
/// so they form their own section instead of breaking the date order.
fn section(chat: &NavigationChat, at: Option<i64>, now: DateTime<FixedOffset>) -> &'static str {
    if chat.unread && !chat.archived {
        return "unread";
    }
    let Some(at) = at.and_then(DateTime::<chrono::Utc>::from_timestamp_millis) else {
        return "earlier";
    };
    let days = (now.date_naive() - at.with_timezone(now.offset()).date_naive()).num_days();
    match days {
        ..=0 => "today",
        1 => "yesterday",
        2..=6 => "week",
        _ => "earlier",
    }
}

/// Same order as `NavigationData::project` (archived, unread first, message
/// time, id), extended across devices by comparing parsed instants.
pub(crate) fn merge(inputs: &[Input], now: DateTime<FixedOffset>) -> Value {
    let mut rows: Vec<(&Input, &NavigationChat, Option<i64>)> = inputs
        .iter()
        .flat_map(|input| {
            input
                .navigation
                .chats
                .iter()
                .map(move |chat| (input, chat, millis(chat)))
        })
        .collect();
    rows.sort_by(|(ap, a, at), (bp, b, bt)| {
        a.archived
            .cmp(&b.archived)
            .then_with(|| b.unread.cmp(&a.unread))
            .then_with(|| bt.cmp(at))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| ap.peer.cmp(&bp.peer))
            .then_with(|| a.chat_id.cmp(&b.chat_id))
    });
    let row = |(input, chat, at): &(&Input, &NavigationChat, Option<i64>)| {
        let mut value = json!(chat);
        value["peer"] = json!(input.peer);
        value["peer_name"] = json!(input.name);
        value["updated_at_ms"] = json!(at);
        value["section"] = json!(section(chat, *at, now));
        value
    };
    let split = rows.partition_point(|(_, chat, _)| !chat.archived);
    let (visible, archived) = rows.split_at(split);
    json!({
        "chats": visible.iter().take(CHAT_LIMIT).map(&row).collect::<Vec<_>>(),
        "chat_total": visible.len(),
        "archived": archived.iter().take(ARCHIVED_LIMIT).map(&row).collect::<Vec<_>>(),
        "archived_total": archived.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(id: &str, updated_at: &str, unread: bool, archived: bool) -> NavigationChat {
        NavigationChat {
            chat_id: id.into(),
            title: id.into(),
            updated_at: updated_at.into(),
            unread,
            archived,
            ..Default::default()
        }
    }
    fn input(peer: &str, chats: Vec<NavigationChat>) -> Input {
        Input {
            peer: peer.into(),
            name: format!("name-{peer}"),
            navigation: Arc::new(NavigationData {
                chats: Arc::new(chats),
                ..Default::default()
            }),
        }
    }
    fn at(value: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(value).unwrap()
    }
    fn ids(list: &Value) -> Vec<String> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|c| {
                format!(
                    "{}/{}",
                    c["peer"].as_str().unwrap(),
                    c["chat_id"].as_str().unwrap()
                )
            })
            .collect()
    }

    #[test]
    fn devices_mix_by_instant_with_unread_first_and_archived_apart() {
        let merged = merge(
            &[
                input(
                    "a",
                    vec![
                        chat("old", "2026-09-01T08:00:00Z", false, false),
                        // Offsets differ between devices; instants decide.
                        chat("late", "2026-09-24T12:00:00+02:00", false, false),
                        chat("gone", "2026-09-24T11:00:00Z", false, true),
                    ],
                ),
                input(
                    "b",
                    vec![
                        chat("mid", "2026-09-24T10:30:00Z", false, false),
                        chat("ping", "2026-08-01T00:00:00Z", true, false),
                        chat("stored", "2026-09-20T00:00:00Z", false, true),
                    ],
                ),
            ],
            at("2026-09-24T18:00:00Z"),
        );
        assert_eq!(
            ids(&merged["chats"]),
            ["b/ping", "b/mid", "a/late", "a/old"]
        );
        assert_eq!(ids(&merged["archived"]), ["a/gone", "b/stored"]);
        assert_eq!(merged["chat_total"], 4);
        assert_eq!(merged["archived_total"], 2);
        let first = &merged["chats"][0];
        assert_eq!(first["peer_name"], "name-b");
        assert_eq!(first["unread"], true);
        assert_eq!(first["section"], "unread");
    }

    #[test]
    fn sections_follow_the_local_calendar_day() {
        let merged = merge(
            &[input(
                "a",
                vec![
                    chat("today", "2026-09-24T00:30:00+08:00", false, false),
                    chat("yesterday", "2026-09-23T23:59:00+08:00", false, false),
                    chat("week", "2026-09-19T12:00:00+08:00", false, false),
                    chat("earlier", "2026-09-17T12:00:00+08:00", false, false),
                    chat("unknown", "", false, false),
                ],
            )],
            // 08:00 local on the 24th; the first chat is 16:30 UTC the day before.
            at("2026-09-24T08:00:00+08:00"),
        );
        let sections: Vec<_> = merged["chats"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                (
                    c["chat_id"].as_str().unwrap(),
                    c["section"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            sections,
            [
                ("today", "today"),
                ("yesterday", "yesterday"),
                ("week", "week"),
                ("earlier", "earlier"),
                ("unknown", "earlier"),
            ]
        );
        assert!(merged["chats"][4]["updated_at_ms"].is_null());
    }

    #[test]
    fn bounds_keep_unread_and_report_totals() {
        let mut chats: Vec<_> = (0..CHAT_LIMIT + 20)
            .map(|i| chat(&format!("c{i:04}"), "2026-09-24T00:00:00Z", false, false))
            .collect();
        chats.push(chat("unread", "2020-01-01T00:00:00Z", true, false));
        let merged = merge(&[input("a", chats)], at("2026-09-24T12:00:00Z"));
        assert_eq!(merged["chats"].as_array().unwrap().len(), CHAT_LIMIT);
        assert_eq!(merged["chat_total"], CHAT_LIMIT + 21);
        assert_eq!(merged["chats"][0]["chat_id"], "unread");
    }

    #[test]
    fn platform_key_is_client_owned() {
        let key: crate::subscriptions::Key =
            serde_json::from_value(json!({"projection":"navigation"})).unwrap();
        assert_eq!(key.peer(), "");
    }

    #[test]
    fn midnight_wake_is_bounded() {
        let wait = until_midnight(at("2026-09-24T23:59:59+08:00"));
        assert!(wait >= std::time::Duration::from_secs(1));
        assert!(
            until_midnight(at("2026-09-24T00:00:02+08:00"))
                <= std::time::Duration::from_secs(86_400)
        );
    }
}
