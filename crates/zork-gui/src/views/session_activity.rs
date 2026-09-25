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
            let first_visible = end
                .as_ref()
                .and_then(|id| entries.iter().position(|entry| &entry.id == id))
                .map_or(entries.len().saturating_sub(1), |index| index + 1);
            self.excluded = entries
                .iter()
                .take(first_visible)
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
    pub rows: Vec<zork_ui::components::activity::SessionRow>,
    pub expanded: bool,
    /// The round ended; the preview holds briefly, then leaves.
    pub stopped: bool,
    /// Fading out; removed once the exit has played.
    pub leaving: bool,
    window: PreviewWindow,
    _source: Arc<Conversation>,
    updates: Option<zork_client_core::state::HistorySubscription>,
    subscription: Option<Task<()>>,
    dismiss: Option<Task<()>>,
}

/// A finished round stays readable this long before it fades away.
const FINISH_HOLD: Duration = Duration::from_millis(600);

fn step_duration(ms: i64, locale: Locale) -> String {
    let seconds = (ms + 500) / 1000;
    match (locale == Locale::ZhCn, seconds < 60) {
        (true, true) => format!("{seconds} 秒"),
        (true, false) => format!("{} 分 {} 秒", seconds / 60, seconds % 60),
        (false, true) => format!("{seconds}s"),
        (false, false) => format!("{}m {}s", seconds / 60, seconds % 60),
    }
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
        // Tool steps only: input, replies, thinking and round boundaries are
        // not "what the member is doing" and never become filler text.
        self.rows = projection
            .activities
            .iter()
            .filter(|activity| {
                !matches!(
                    activity.kind,
                    Kind::Input | Kind::Output | Kind::Thinking | Kind::End
                )
            })
            .map(|activity| {
                let entry = &visible[activity.entry];
                zork_ui::components::activity::SessionRow {
                    id: entry.id.clone(),
                    icon: zork_ui::components::history::kind_icon(activity.kind),
                    label: locale
                        .text(zork_ui::components::history::kind_label(activity.kind))
                        .to_owned(),
                    summary: zork_ui::history::activity::preview(&activity.summary),
                    failed: matches!(entry.state.as_str(), "failed" | "timed_out"),
                    running: entry.state == "running",
                    duration: entry
                        .end
                        .and_then(|_| entry.duration(0))
                        .filter(|ms| *ms >= 1000)
                        .map(|ms| step_duration(ms, locale)),
                }
            })
            .collect();
    }
}

/// Builds the activity row of the transcript. The message list calls it for
/// its trailing item, so it is rebuilt on every frame the row is laid out.
pub(super) type ActivityRow = Rc<dyn Fn(&mut Window, &mut gpui::App) -> gpui::AnyElement>;

impl RootView {
    /// The activity is the transcript's trailing item: a change of its
    /// content or height remeasures that item, so scrolling to the end,
    /// following the tail and the scrollbar see its current height.
    pub(super) fn invalidate_session_activity(&mut self, cx: &mut Context<Self>) {
        let tail = self.lines.len();
        self.transcript_list.remeasure_items(tail..tail + 1);
        zork_ui::components::region::invalidate(cx, &["transcript"]);
    }

    pub(super) fn sync_session_activity(&mut self, cx: &mut Context<Self>) {
        self.sync_session_activity_preview(cx);
        self.invalidate_session_activity(cx);
    }

    fn sync_session_activity_preview(&mut self, cx: &mut Context<Self>) {
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
            let source = self.core_device.conversation(&session);
            self.session_activity_preview = Some(SessionActivityPreview {
                session,
                name,
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
        } else if !self.agent_online {
            // Disconnected is not the end of the round: keep the last
            // reliable state until the device answers again.
        } else if let Some(preview) = self.session_activity_preview.as_mut() {
            if preview.stopped {
                return;
            }
            preview.stopped = true;
            preview.leaving = false;
            preview.subscription = None;
            preview.updates = None;
            let session = preview.session.clone();
            preview.dismiss = Some(cx.spawn(async move |this, cx| {
                // Hold the finished state briefly, then fade on the exit
                // curve and drop it once the fade has played.
                cx.background_executor().timer(FINISH_HOLD).await;
                let still = |view: &RootView| {
                    view.session_activity_preview
                        .as_ref()
                        .is_some_and(|preview| preview.session == session && preview.stopped)
                };
                if this
                    .update(cx, |view, cx| {
                        if still(view) {
                            view.session_activity_preview.as_mut().unwrap().leaving = true;
                            view.invalidate_session_activity(cx);
                        }
                    })
                    .is_err()
                {
                    return;
                }
                cx.background_executor()
                    .timer(
                        zork_ui::motion::duration(zork_ui::motion::exit(zork_ui::motion::BASE))
                            + Duration::from_millis(32),
                    )
                    .await;
                let _ = this.update(cx, |view, cx| {
                    if still(view) {
                        view.session_activity_preview = None;
                        view.invalidate_session_activity(cx);
                    }
                });
            }));
        }
    }

    /// The live activity row, the last item of the message list at the
    /// message column's width. It shows the member's Session preview, or the
    /// live status while its records have not arrived. The presence is
    /// stepped here, in the transcript's render, so the enter and exit keep
    /// their progress while the row is scrolled out of the virtualized list.
    pub(super) fn render_session_activity(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<ActivityRow> {
        use zork_ui::components::activity::{Actions, Labels, Phase, SessionActivity};
        let locale = self.locale;
        let offline = !self.agent_online;
        let labels = Labels {
            expand: locale.text("session_activity_more").into(),
            collapse: locale.text("session_activity_less").into(),
            stop: locale.text("session_activity_stop").into(),
            history: locale.text("session_activity_history").into(),
            now: locale.text("session_activity_now").into(),
        };
        let presentations = self.activity_presentations();
        let failed = presentations.iter().any(|item| item.failed);
        let data = match self
            .session_activity_preview
            .as_ref()
            .filter(|preview| !preview.rows.is_empty() && !failed)
        {
            Some(preview) => {
                let phase = if offline {
                    Phase::Disconnected
                } else if preview.stopped {
                    Phase::Finished
                } else {
                    Phase::Running
                };
                Some((
                    preview.session.clone(),
                    SessionActivity {
                        name: preview.name.clone(),
                        phase,
                        expanded: preview.expanded,
                        rows: preview.rows.clone(),
                        live: None,
                        status: locale
                            .text(match phase {
                                Phase::Running => "history_running",
                                Phase::Finished => "session_activity_done",
                                Phase::Disconnected => "session_activity_offline",
                            })
                            .into(),
                        labels,
                    },
                    !preview.leaving,
                ))
            }
            None => presentations.first().map(|item| {
                let session = self
                    .participants
                    .iter()
                    .find(|participant| participant.id == item.id)
                    .map(|participant| participant.session_id.clone())
                    .unwrap_or_default();
                (
                    session,
                    SessionActivity {
                        name: item.name.clone(),
                        phase: if offline {
                            Phase::Disconnected
                        } else if item.running {
                            Phase::Running
                        } else {
                            Phase::Finished
                        },
                        expanded: false,
                        rows: Vec::new(),
                        live: Some((item.label.clone(), item.failed)),
                        status: String::new(),
                        labels,
                    },
                    true,
                )
            }),
        };
        let open = data.as_ref().is_some_and(|(_, _, open)| *open);
        let frame = zork_ui::motion::presence(
            "session-activity-presence",
            open,
            zork_ui::motion::BASE,
            window,
            cx,
        );
        let (session, data, _) = match (frame, data) {
            (Some(_), Some(data)) => data,
            _ => return None,
        };
        let frame = frame.unwrap();
        let root = cx.entity().downgrade();
        let open_root = root.clone();
        let stop_root = root.clone();
        let running = data.phase == Phase::Running;
        let expand: Rc<dyn Fn(&mut gpui::App)> = Rc::new(move |cx| {
            let _ = root.update(cx, |view, cx| {
                if let Some(preview) = view.session_activity_preview.as_mut() {
                    preview.expanded = !preview.expanded;
                    view.invalidate_session_activity(cx);
                }
            });
        });
        let open: Rc<dyn Fn(Option<String>, &mut gpui::App)> = Rc::new(move |entry, cx| {
            let session = session.clone();
            let _ = open_root.update(cx, |view, cx| match entry {
                Some(entry) => view.open_history_entry(&session, entry, cx),
                None => view.open_history(&session, cx),
            });
        });
        let stop = (running && !self.canceling).then(|| {
            Rc::new(move |cx: &mut gpui::App| {
                let _ = stop_root.update(cx, |view, cx| view.cancel_session(cx));
            }) as Rc<dyn Fn(&mut gpui::App)>
        });
        let width = self.composer_surface_width;
        let data = Rc::new(data);
        Some(Rc::new(move |window: &mut Window, cx: &mut gpui::App| {
            let (element, moving) = zork_ui::components::activity::render_session(
                &data,
                Actions {
                    expand: expand.clone(),
                    open: open.clone(),
                    stop: stop.clone(),
                },
                window,
                cx,
            );
            // Expanding, collapsing and step changes resize the row: keep
            // laying the list out until the transition settles.
            if moving {
                window.request_animation_frame();
            }
            // Same row geometry as a message: the message column, centered.
            div()
                .id("session-activity-row")
                .w_full()
                .px_6()
                .pt(px(12.))
                .pb(px(7.))
                .flex()
                .justify_center()
                .child(
                    div()
                        .w(px(width))
                        .max_w_full()
                        .relative()
                        .top(px(zork_ui::motion::ROW_OFFSET * frame.travel))
                        .opacity(frame.opacity)
                        .child(element),
                )
                .into_any_element()
        }))
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
            preview.updates = Some(updates);
            self.invalidate_session_activity(cx);
            return;
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

    #[test]
    fn preview_does_not_replay_last_completed_turn() {
        let mut window = PreviewWindow::default();
        let mut entries = vec![entry("old", "read"), entry("end", "end")];
        assert!(window.select(&entries).is_empty());
        entries.push(entry("current", "read"));
        assert_eq!(
            window
                .select(&entries)
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            ["current"]
        );
    }
}
