//! A bounded, read-only Session history preview for the active Chat member.
use super::*;
use std::collections::HashSet;
use zork_client_core::state::{Conversation, HistoryData};
use zork_ui::history::{
    activity::{Kind, Projection},
    Entry,
};

#[derive(Default)]
struct PreviewWindow {
    initialized: bool,
    excluded: HashSet<String>,
    last_end: Option<String>,
}

impl PreviewWindow {
    fn latest_end(entries: &[Entry]) -> Option<String> {
        let all = Projection::new(entries);
        all.activities
            .iter()
            .rev()
            .find(|activity| {
                activity.kind == Kind::End && entries[activity.entry].state == "succeeded"
            })
            .map(|activity| entries[activity.entry].id.clone())
    }

    fn restart(&mut self, entries: &[Entry]) {
        let end = Self::latest_end(entries);
        let boundary_changed = end.is_some() && end != self.last_end;
        self.excluded = entries
            .iter()
            .take(if boundary_changed {
                entries
                    .iter()
                    .position(|entry| Some(&entry.id) == end.as_ref())
                    .map_or(entries.len(), |index| index + 1)
            } else {
                entries.len()
            })
            .map(|entry| entry.id.clone())
            .collect();
        self.last_end = end;
        self.initialized = true;
    }

    fn select(&mut self, entries: &[Entry]) -> Vec<Entry> {
        let end = Self::latest_end(entries);
        if !self.initialized {
            self.excluded = entries
                .iter()
                .take(entries.len().saturating_sub(1))
                .map(|entry| entry.id.clone())
                .collect();
            self.last_end = end.clone();
            self.initialized = true;
        } else if end.is_some() && end != self.last_end {
            self.last_end = end.clone();
            if let Some(index) = entries
                .iter()
                .position(|entry| Some(&entry.id) == end.as_ref())
            {
                self.excluded = entries[..=index]
                    .iter()
                    .map(|entry| entry.id.clone())
                    .collect();
            }
        }
        entries
            .iter()
            .filter(|entry| !self.excluded.contains(&entry.id))
            .cloned()
            .collect()
    }
}

pub(super) struct SessionActivityPreview {
    pub session: String,
    pub name: String,
    pub avatar: Option<String>,
    pub rows: Vec<zork_ui::components::activity::SessionRow>,
    pub expanded: bool,
    pub stopped: bool,
    pub leaving: bool,
    window: PreviewWindow,
    _source: Arc<Conversation>,
    updates: Option<zork_client_core::state::HistorySubscription>,
    subscription: Option<Task<()>>,
    dismiss: Option<Task<()>>,
}

impl SessionActivityPreview {
    fn apply(&mut self, data: &HistoryData, locale: Locale) {
        if !data.loaded || data.revoked {
            self.rows.clear();
            return;
        }
        // The core owns the complete ledger. A visible Chat projects only the
        // newest 50 associated entries.
        let entries = data
            .entries
            .iter()
            .rev()
            .take(50)
            .cloned()
            .collect::<Vec<_>>();
        let entries = entries.into_iter().rev().collect::<Vec<_>>();
        let visible = self.window.select(&entries);
        let projection = Projection::new(&visible);
        self.rows = projection
            .rows(&visible, &HashSet::new())
            .into_iter()
            .filter_map(|row| {
                let block = &projection.blocks[row.block];
                let activity_index = row.activity.unwrap_or(block.start);
                let activity = &projection.activities[activity_index];
                if activity.kind == Kind::Input {
                    return None;
                }
                let entry = &visible[activity.entry];
                let group = row.activity.is_none();
                let label = if group {
                    let counts = &block.counts;
                    [
                        (counts.read, "history_group_read"),
                        (counts.written, "history_group_write"),
                        (counts.edited, "history_group_edit"),
                        (counts.shell, "history_group_command"),
                        (counts.queries, "history_group_query"),
                        (counts.thinking, "history_group_thinking"),
                        (counts.other, "history_group_operation"),
                        (counts.failed, "history_group_failed"),
                    ]
                    .into_iter()
                    .filter(|(count, _)| *count > 0)
                    .map(|(count, key)| locale.text(key).replace("{count}", &count.to_string()))
                    .collect::<Vec<_>>()
                    .join(" · ")
                } else {
                    locale
                        .text(zork_ui::components::history::kind_label(activity.kind))
                        .to_owned()
                };
                Some(zork_ui::components::activity::SessionRow {
                    id: entry.id.clone(),
                    icon: if group {
                        "history/generic-tool.svg"
                    } else {
                        zork_ui::components::history::kind_icon(activity.kind)
                    },
                    label,
                    summary: zork_ui::history::activity::preview(if group {
                        &block.summary
                    } else {
                        &activity.summary
                    }),
                    failed: block.counts.failed > 0
                        || matches!(entry.state.as_str(), "failed" | "timed_out"),
                    running: entry.state == "running",
                })
            })
            .collect();
    }
}

impl RootView {
    pub(super) fn sync_session_activity(&mut self, cx: &mut Context<Self>) {
        let working = |participant: &crate::api::ParticipantStatus| {
            !participant.session_id.is_empty()
                && participant.activity.as_ref().is_some_and(|status| {
                    matches!(
                        status,
                        AgentStatus::Live { .. }
                            | AgentStatus::Thinking
                            | AgentStatus::ToolsStarted { .. }
                            | AgentStatus::ToolsWaiting { .. }
                            | AgentStatus::ToolFinished { .. }
                            | AgentStatus::Waiting { .. }
                    )
                })
        };
        let active = self
            .session_activity_preview
            .as_ref()
            .and_then(|preview| {
                self.participants.iter().find(|participant| {
                    participant.session_id == preview.session && working(participant)
                })
            })
            .or_else(|| {
                self.participants
                    .iter()
                    .find(|participant| working(participant))
            });
        if let Some(member) = active {
            if let Some(preview) = self
                .session_activity_preview
                .as_mut()
                .filter(|preview| preview.session == member.session_id)
            {
                preview.name = member.name.clone();
                preview.avatar = member.avatar.clone();
                let restart = preview.stopped;
                preview.stopped = false;
                preview.leaving = false;
                preview.dismiss = None;
                if restart {
                    self.observe_session_activity(true, cx);
                }
                return;
            }
            let session = member.session_id.clone();
            let name = member.name.clone();
            let avatar = member.avatar.clone();
            let source = self.core_device.conversation(&session);
            self.session_activity_preview = Some(SessionActivityPreview {
                session,
                name,
                avatar,
                rows: Vec::new(),
                expanded: false,
                stopped: false,
                leaving: false,
                window: PreviewWindow::default(),
                _source: source,
                updates: None,
                subscription: None,
                dismiss: None,
            });
            self.observe_session_activity(false, cx);
        } else if let Some(preview) = self.session_activity_preview.as_mut() {
            if preview.stopped {
                return;
            }
            preview.stopped = true;
            preview.leaving = false;
            preview.subscription = None;
            preview.updates = None;
            let session = preview.session.clone();
            let reduced = cx.reduce_motion();
            preview.dismiss = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(600))
                    .await;
                if !reduced {
                    if this
                        .update(cx, |view, cx| {
                            if let Some(preview) = view
                                .session_activity_preview
                                .as_mut()
                                .filter(|preview| preview.session == session && preview.stopped)
                            {
                                preview.leaving = true;
                                zork_ui::components::region::invalidate(cx, &["transcript"]);
                            }
                        })
                        .is_err()
                    {
                        return;
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(220))
                        .await;
                }
                let _ = this.update(cx, |view, cx| {
                    if view
                        .session_activity_preview
                        .as_ref()
                        .is_some_and(|preview| preview.session == session && preview.stopped)
                    {
                        view.session_activity_preview = None;
                        view.transcript_list
                            .remeasure_items(view.lines.len()..view.lines.len() + 1);
                        zork_ui::components::region::invalidate(cx, &["transcript"]);
                    }
                });
            }));
        }
    }

    fn observe_session_activity(&mut self, restart: bool, cx: &mut Context<Self>) {
        let Some(preview) = self.session_activity_preview.as_mut() else {
            return;
        };
        let source = preview._source.clone();
        let history = source.history();
        let mut updates = history.subscribe();
        if let Some(update) = updates.prepare() {
            if restart {
                let entries = update
                    .state
                    .entries
                    .iter()
                    .rev()
                    .take(50)
                    .cloned()
                    .collect::<Vec<_>>();
                let entries = entries.into_iter().rev().collect::<Vec<_>>();
                preview.window.restart(&entries);
            }
            preview.apply(&update.state, self.locale);
            updates.acknowledge(update.batch.unwrap());
        }
        let mut readiness = updates.readiness();
        preview.updates = Some(updates);
        preview.subscription = Some(cx.spawn(async move |this, cx| {
            while readiness.changed().await.is_ok() {
                if readiness.take_urgent() {
                    if this
                        .update(cx, |view, cx| view.deliver_core_updates(cx))
                        .is_err()
                    {
                        return;
                    }
                } else if !zork_ui::components::frame_delivery::FrameDelivery::request(
                    &this,
                    cx,
                    |view| &mut view.frame_delivery,
                ) {
                    return;
                }
            }
        }));
        #[cfg(not(feature = "headless-bench"))]
        {
            source.start();
            history.load(false);
        }
        #[cfg(feature = "headless-bench")]
        if !self.benchmark_offline {
            source.start();
            history.load(false);
        }
    }

    pub(super) fn deliver_session_activity_updates(&mut self, cx: &mut Context<Self>) {
        let Some(preview) = self.session_activity_preview.as_mut() else {
            return;
        };
        let Some(mut updates) = preview.updates.take() else {
            return;
        };
        if let Some(update) = updates.prepare() {
            preview.apply(&update.state, self.locale);
            updates.acknowledge(update.batch.unwrap());
            self.transcript_list
                .remeasure_items(self.lines.len()..self.lines.len() + 1);
            zork_ui::components::region::invalidate(cx, &["transcript"]);
        }
        preview.updates = Some(updates);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, action: &str) -> Entry {
        Entry {
            id: id.into(),
            lane: 2,
            action: action.into(),
            summary: id.into(),
            start: None,
            end: None,
            state: "succeeded".into(),
            raw: vec![],
            usage: None,
            model: None,
            outcome_summary: None,
        }
    }

    #[test]
    fn preview_starts_at_current_work_and_resets_after_end() {
        let mut window = PreviewWindow::default();
        let mut entries = vec![
            entry("old", "read"),
            entry("end-1", "end"),
            entry("current", "read"),
        ];
        assert_eq!(
            window
                .select(&entries)
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            ["current"]
        );
        entries.push(entry("next", "write"));
        assert_eq!(
            window
                .select(&entries)
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            ["current", "next"]
        );
        entries.push(entry("end-2", "end"));
        assert!(window.select(&entries).is_empty());
        entries.push(entry("new", "read"));
        assert_eq!(
            window
                .select(&entries)
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            ["new"]
        );
        window.restart(&entries);
        assert!(
            window.select(&entries).is_empty(),
            "old work must not reappear on a new status"
        );
        entries.push(entry("newer", "write"));
        assert_eq!(
            window
                .select(&entries)
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            ["newer"]
        );
    }
}
