//! A hover owns only a read-only core subscription and its presentation.
use super::*;
#[cfg(feature = "headless-bench")]
use zork_client_core::state::HistoryData;
use zork_client_core::state::{ConversationTopics, SessionOverview};
use zork_ui::components::tooltip::DetailsTooltip;

/// Core subscription adapter; the shared overlay owns presentation and hover.
#[derive(Default)]
pub(super) struct Overlay {
    active: Option<Entity<Preview>>,
    observation: Option<gpui::Subscription>,
    shared: Option<Entity<zork_ui::member_activity::Overlay>>,
}
impl Overlay {
    fn shared(&mut self, cx: &mut Context<Self>) -> Entity<zork_ui::member_activity::Overlay> {
        if let Some(shared) = &self.shared {
            return shared.clone();
        }
        let shared = cx.new(|_| Default::default());
        cx.subscribe(
            &shared,
            |v, _, event: &zork_ui::member_activity::Closed, cx| {
                if v.active
                    .as_ref()
                    .is_some_and(|active| active.read(cx).member.id == event.0)
                {
                    v.active = None;
                    v.observation = None;
                }
            },
        )
        .detach();
        self.shared = Some(shared.clone());
        shared
    }
    pub(super) fn show(
        &mut self,
        id: &str,
        anchor: gpui::Bounds<gpui::Pixels>,
        create: impl FnOnce(&mut gpui::App) -> Entity<Preview>,
        cx: &mut Context<Self>,
    ) {
        if !self
            .active
            .as_ref()
            .is_some_and(|active| active.read(cx).member.id == id)
        {
            let content = create(cx);
            self.observation = Some(cx.observe(&content, |v, _, cx| v.update_shared(cx)));
            self.active = Some(content);
        }
        let shared = self.shared(cx);
        let content = self.active.as_ref().unwrap().read(cx);
        let details = content.details.clone();
        let footer = content.locale.text("presence_history_hint").into();
        shared.update(cx, |view, cx| {
            view.show(id.into(), details, footer, anchor, cx)
        });
        cx.notify();
    }
    fn update_shared(&mut self, cx: &mut Context<Self>) {
        if let Some(content) = &self.active {
            let content = content.read(cx);
            let id = content.member.id.clone();
            let details = content.details.clone();
            let footer = content.locale.text("presence_history_hint").into();
            self.shared(cx)
                .update(cx, |view, cx| view.update(&id, details, footer, cx));
        }
    }
    pub(super) fn anchor(
        &mut self,
        id: &str,
        bounds: gpui::Bounds<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.shared(cx)
            .update(cx, |view, cx| view.anchor(id, bounds, cx));
    }
    pub(super) fn retain(
        &mut self,
        ids: impl Iterator<Item = impl AsRef<str>>,
        cx: &mut Context<Self>,
    ) {
        let ids: Vec<_> = ids.map(|id| id.as_ref().to_owned()).collect();
        self.shared(cx).update(cx, |view, cx| view.retain(&ids, cx));
    }
    pub(super) fn leave(&mut self, id: &str, cx: &mut Context<Self>) {
        self.shared(cx).update(cx, |view, cx| view.leave(id, cx));
    }
}
impl Render for Overlay {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.shared(cx)
    }
}

pub(super) struct Preview {
    member: ParticipantStatus,
    locale: Locale,
    details: DetailsTooltip,
    state: Arc<SessionOverview>,
    _source: Option<Arc<zork_client_core::state::Conversation>>,
    _subscription: Option<Task<()>>,
    _activity: Option<Task<()>>,
}

impl Preview {
    #[cfg(feature = "headless-bench")]
    pub(super) fn fixture(
        member: ParticipantStatus,
        locale: Locale,
        state: Arc<HistoryData>,
    ) -> Self {
        let state = Arc::new(SessionOverview::fixture(&state));
        Self {
            details: details(&member, locale, &state),
            member,
            locale,
            state,
            _source: None,
            _subscription: None,
            _activity: None,
        }
    }
    pub(super) fn new(
        member: ParticipantStatus,
        locale: Locale,
        source: Option<Arc<zork_client_core::state::Conversation>>,
        conversation: Option<Arc<zork_client_core::state::Conversation>>,
        is_selected: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut changes = source
            .as_ref()
            .map(|source| source.subscribe_topics(ConversationTopics::OVERVIEW));
        let state = changes
            .as_mut()
            .map(|changes| changes.snapshot().state.overview.clone())
            .unwrap_or_else(|| {
                Arc::new(SessionOverview {
                    loaded: true,
                    ..Default::default()
                })
            });
        let card = details(&member, locale, &state);
        let subscription = changes.map(|mut changes| {
            cx.spawn(async move |this, cx| {
                while let Some(update) = changes.changed().await {
                    if this
                        .update(cx, |v, cx| {
                            v.details = details(&v.member, v.locale, &update.state.overview);
                            v.state = update.state.overview.clone();
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
        });
        let activity = conversation.map(|conversation| {
            let mut changes = conversation
                .subscribe_topics(ConversationTopics::ACTIVITY | ConversationTopics::PARTICIPANTS);
            cx.spawn(async move |this, cx| {
                while let Some(update) = changes.changed().await {
                    if this
                        .update(cx, |v, cx| {
                            let previous = v.member.clone();
                            if let Some(member) = update
                                .state
                                .participants
                                .iter()
                                .find(|m| m.id == v.member.id)
                            {
                                v.member = member.clone();
                            } else if is_selected {
                                v.member.activity = update.state.activity.clone();
                            }
                            if previous != v.member {
                                v.details = details(&v.member, v.locale, &v.state);
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
        });
        if let Some(source) = &source {
            // The normal session stream supplies snapshot + future updates.
            // A hover never creates a history controller or requests its pages.
            source.start();
        }
        Self {
            member,
            locale,
            details: card,
            state,
            _source: source,
            _subscription: subscription,
            _activity: activity,
        }
    }
}

fn details(member: &ParticipantStatus, locale: Locale, state: &SessionOverview) -> DetailsTooltip {
    let mut rows = Vec::with_capacity(5);
    for (index, entry) in state.highlights().enumerate() {
        let projection = zork_ui::history::activity::Projection::new(std::slice::from_ref(&entry));
        let action = projection
            .activities
            .first()
            .filter(|a| a.kind != zork_ui::history::activity::Kind::UnknownTool)
            .map(|a| {
                locale
                    .text(zork_ui::components::history::kind_label(a.kind))
                    .to_owned()
            })
            .unwrap_or_else(|| zork_ui::history::activity::preview(&entry.action));
        let status = locale.text(match entry.state.as_str() {
            "running" => "history_running",
            "succeeded" => "history_success",
            "cancelled" | "interrupted" => "history_cancelled",
            "failed" | "timed_out" => "history_error",
            _ => "history_notice",
        });
        // Only explicit failure detail is useful here. Successful stdout and
        // model reasoning belong in the full history, not a hover card.
        let error = matches!(entry.state.as_str(), "failed" | "timed_out")
            .then(|| entry.outcome_summary.as_deref().unwrap_or(&entry.summary))
            .filter(|text| !text.trim().is_empty())
            .map(zork_ui::history::activity::preview);
        let time = entry
            .end
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|at| {
                format!(
                    " · {}",
                    at.with_timezone(&chrono::Local).format("%m-%d %H:%M")
                )
            })
            .unwrap_or_default();
        rows.push((
            locale
                .text(if index == 0 {
                    "presence_recent"
                } else {
                    "presence_previous"
                })
                .into(),
            match error {
                Some(error) => format!("{action} · {status}{time} — {error}"),
                None => format!("{action} · {status}{time}"),
            },
        ));
    }
    if rows.is_empty() || state.error.is_some() {
        rows.push((
            locale.text("presence_recent").into(),
            locale
                .text(
                    if state.error.is_some() || (state.loaded && !state.aggregates.complete) {
                        "presence_history_unavailable"
                    } else if state.loaded {
                        "history_no_activity"
                    } else {
                        "presence_history_loading"
                    },
                )
                .into(),
        ));
    }
    if let Some(runtime) = &state.runtime {
        if let Some(model) = &runtime.model {
            rows.push((locale.text("model").into(), model.clone()));
        }
        if let (Some(used), Some(limit)) = (runtime.context_tokens, runtime.context_limit) {
            if limit > 0 {
                rows.push((
                    locale.text("presence_context").into(),
                    format!("{used} / {limit} tokens"),
                ));
            }
        }
    }
    if member.assigned {
        rows.push((
            locale.text("presence_work_relation").into(),
            locale.text("presence_assigned_executor").into(),
        ));
    }
    rows.push((
        locale.text("chat_receiving").into(),
        locale
            .text(if member.subscribed {
                "chat_subscribed"
            } else {
                "chat_unsubscribed"
            })
            .into(),
    ));
    DetailsTooltip {
        key: format!("presence-{}", member.id),
        title: member.name.clone(),
        kind: locale.text("presence_member").into(),
        avatar: Some(member.avatar.clone().unwrap_or_else(|| "cat".into())),
        description: member
            .activity
            .as_ref()
            .filter(|s| should_render_live_activity(s, None))
            .map(|s| agent_status_label(s, locale))
            .unwrap_or_else(|| locale.text("presence_idle").into()),
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpt_keeps_failures_but_omits_success_output_and_internal_model_text() {
        let state = SessionOverview {
            loaded: true,
            aggregates: serde_json::from_value(serde_json::json!({
                "complete":true,"usage":{"input":0,"output":0,"cached":0,"reported_steps":0,"cache_reported_steps":0,"cache_input":0},
                "recent":[
                    {"id":"tool:b","lane":2,"tool":"read_file","action":"Read file","state":"failed","finished_at_ms":2,"error":"File missing"},
                    {"id":"tool:a","lane":2,"tool":"exec","action":"Execute command","state":"succeeded","finished_at_ms":1,"error":null}
                ]
            })).unwrap(),
            ..Default::default()
        };
        let member = ParticipantStatus {
            subscribed: false,
            assigned: false,
            id: "a".into(),
            name: "Atlas".into(),
            avatar: None,
            session_id: "session-a".into(),
            activity: None,
        };
        for locale in Locale::ALL {
            let card = details(&member, locale, &state);
            assert_eq!(card.title, "Atlas");
            assert!(card.rows[0].1.contains("File missing"));
            let text = format!("{:?}", card.rows);
            assert!(!text.contains("PRIVATE OUTPUT"));
            assert!(!text.contains("INTERNAL REASONING"));
        }
    }
}
