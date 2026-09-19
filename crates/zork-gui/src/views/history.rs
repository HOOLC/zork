//! Native history backed by the core's durable execution projection.
use super::*;
#[cfg(feature = "headless-bench")]
use crate::session_history as model;
use crate::session_history::{Entry, Record};
use gpui::ListOffset;
#[cfg(feature = "headless-bench")]
use gpui::{ListAlignment, ListState};
use zork_ui::history::activity::{self, Activity, Kind, Projection, Subject};
use zork_ui::history_page::Host as HistoryHost;

mod live;
mod presentation;
mod statistics;

pub(super) struct HistoryState {
    ui: zork_ui::history_page::State,
    pub open: bool,
    session: Option<String>,
    records: zork_client_core::observe::List<Record>,
    runtime: Option<zork_client_core::state::HistoryRuntime>,
    pub(super) detail: Option<String>,
    pub(super) agent_detail: Option<String>,
    source: Option<Arc<zork_client_core::state::Conversation>>,
    sources: HashMap<String, (String, Option<String>)>,
    wanted_sources: std::collections::HashSet<String>,
    pub(super) subscription: Option<Task<()>>,
    updates: Option<zork_client_core::state::HistorySubscription>,
    overview_subscription: Option<Task<()>>,
    overview_updates: Option<zork_client_core::state::ConversationSubscription>,
    older: Option<String>,
    latest: Option<String>,
    busy: bool,
    loading_older: bool,
    loaded: bool,
    error: Option<String>,
}
impl Default for HistoryState {
    fn default() -> Self {
        Self {
            ui: Default::default(),
            open: false,
            session: None,
            records: Default::default(),
            runtime: None,
            detail: None,
            agent_detail: None,
            source: None,
            sources: HashMap::new(),
            wanted_sources: Default::default(),
            subscription: None,
            updates: None,
            overview_subscription: None,
            overview_updates: None,
            older: None,
            latest: None,
            busy: false,
            loading_older: false,
            loaded: false,
            error: None,
        }
    }
}
impl std::ops::Deref for HistoryState {
    type Target = zork_ui::history_page::State;
    fn deref(&self) -> &Self::Target {
        &self.ui
    }
}
impl std::ops::DerefMut for HistoryState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.ui
    }
}
impl HistoryState {
    #[cfg(feature = "headless-bench")]
    pub(super) fn benchmark_offset(&self) -> ListOffset {
        self.scroll.logical_scroll_top()
    }

    #[cfg(feature = "headless-bench")]
    pub(super) fn story(records: Vec<Record>, now: i64) -> Self {
        let entries = model::entries(&records);
        let projection = Projection::new(&entries);
        let rows = projection.rows(&entries, &Default::default());
        let scroll = ListState::new(rows.len() + 1, ListAlignment::Top, px(200.))
            .with_uniform_item_height(px(26.));
        Self {
            open: true,
            session: Some("render-fixture".into()),
            records: records.into(),
            ui: zork_ui::history_page::State {
                entries: entries.into(),
                projection,
                rows,
                scroll,
                fixed_now: Some(now),
                ..Default::default()
            }
            .with_metrics(),
            loaded: true,
            ..Self::default()
        }
    }
    #[cfg(feature = "headless-bench")]
    pub(super) fn benchmark(records: Vec<Record>, now: i64) -> Self {
        let entries = model::entries(&records);
        let projection = Projection::new(&entries);
        let rows = projection.rows(&entries, &Default::default());
        let scroll = ListState::new(rows.len() + 1, ListAlignment::Top, px(200.))
            .with_uniform_item_height(px(26.));
        scroll.scroll_to(ListOffset {
            item_ix: 100,
            offset_in_item: px(0.),
        });
        Self {
            open: true,
            session: Some("render-fixture".into()),
            records: records.into(),
            ui: zork_ui::history_page::State {
                entries: entries.into(),
                projection,
                rows,
                scroll,
                fixed_now: Some(now),
                positioned: true,
                following_latest: false,
                ..Default::default()
            }
            .with_metrics(),
            loaded: true,
            ..Self::default()
        }
    }
}
impl RootView {
    pub(super) fn save_chat_history(&mut self) {
        // Inactive chats retain their presentation, not a live subscription or clock.
        self.history.subscription = None;
        self.history.updates = None;
        self.history.overview_subscription = None;
        self.history.overview_updates = None;
        self.history.source = None;
        self.chat_histories
            .insert(self.browser_host(), std::mem::take(&mut self.history));
    }
    pub(super) fn restore_chat_history(&mut self, cx: &mut Context<Self>) {
        let host = self.browser_host();
        self.history = self.chat_histories.remove(&host).unwrap_or_default();
        self.browser
            .update(cx, |panel, cx| panel.set_host(host, cx));
        if self.history.open {
            self.load_history(false, cx);
            if self.history.following_latest {
                self.history.scroll.scroll_to_end();
            }
        }
    }
    pub(super) fn reset_history(&mut self) {
        self.history = HistoryState::default();
    }
    pub(super) fn close_history(&mut self) {
        self.history.open = false;
        self.history.subscription = None;
        self.history.updates = None;
        self.history.overview_subscription = None;
        self.history.overview_updates = None;
        self.history.source = None;
        self.history.detail = None;
        self.history.agent_detail = None;
    }
    pub(super) fn toggle_history(&mut self, session: &str, cx: &mut Context<Self>) {
        if session.is_empty() {
            return;
        }
        if self.history.open && self.history.session.as_deref() == Some(session) {
            self.close_history();
            self.browser
                .update(cx, |panel, cx| panel.close_native_page("history", cx));
            zork_ui::components::region::invalidate_all(cx);
        } else {
            self.open_history(session, cx);
        }
    }
    pub(super) fn open_history(&mut self, session: &str, cx: &mut Context<Self>) {
        if self.history.session.as_deref() != Some(session) {
            self.reset_history();
            self.history.session = Some(session.into());
        }
        self.history.open = true;
        self.open_history_tab(cx);
        self.load_history(false, cx);
        if self.history.following_latest {
            self.history.scroll.scroll_to_end();
        }
        zork_ui::components::region::invalidate(cx, &["history", "header"]);
    }
    pub(super) fn open_history_tab(&mut self, cx: &mut Context<Self>) {
        let page = crate::browser::NativePage {
            id: "history".into(),
            title: self.history_name().into(),
            icon: "icons/history.svg",
        };
        self.browser
            .update(cx, |panel, cx| panel.open_native_page(page, cx));
        zork_ui::components::region::invalidate_all(cx);
    }
    fn load_history(&mut self, older: bool, cx: &mut Context<Self>) {
        let Some(core) = self.observe_history(cx) else {
            return;
        };
        let source = self.history.source.as_ref().unwrap();
        #[cfg(not(feature = "headless-bench"))]
        source.start();
        #[cfg(feature = "headless-bench")]
        if !self.benchmark_offline {
            source.start();
        }
        core.load(older);
    }

    fn observe_history(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<Arc<zork_client_core::state::History>> {
        let sid = self.history.session.clone()?;
        let source = self.core_device.conversation(&sid);
        let core = source.history();
        if self.history.overview_subscription.is_none() {
            let mut changes =
                source.subscribe_topics(zork_client_core::state::ConversationTopics::OVERVIEW);
            if let Some(update) = changes.prepare() {
                self.apply_history_overview(&update.state.overview, cx);
                changes.acknowledge(update.batch.unwrap());
            }
            let mut readiness = changes.readiness();
            self.history.overview_updates = Some(changes);
            self.history.overview_subscription = Some(cx.spawn(async move |this, cx| {
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
                        Self::deliver_core_updates,
                    ) {
                        return;
                    }
                }
            }));
        }
        if self.history.subscription.is_none() {
            let mut changes = core.subscribe();
            if let Some(update) = changes.prepare() {
                let batch = update.batch.unwrap();
                self.apply_history_update(update, cx);
                changes.acknowledge(batch);
            }
            let mut readiness = changes.readiness();
            self.history.updates = Some(changes);
            self.history.subscription = Some(cx.spawn(async move |this, cx| {
                while readiness.changed().await.is_ok() {
                    if readiness.take_urgent() {
                        if this
                            .update(cx, |view, cx| view.deliver_core_updates(cx))
                            .is_err()
                        {
                            return;
                        }
                        continue;
                    }
                    if !zork_ui::components::frame_delivery::FrameDelivery::request(
                        &this,
                        cx,
                        |view| &mut view.frame_delivery,
                        Self::deliver_core_updates,
                    ) {
                        return;
                    }
                }
            }));
        }
        self.history.source = Some(source);
        Some(core)
    }
    pub(super) fn deliver_history_updates(&mut self, cx: &mut Context<Self>) {
        if let Some(mut updates) = self.history.overview_updates.take() {
            if let Some(update) = updates.prepare() {
                self.apply_history_overview(&update.state.overview, cx);
                updates.acknowledge(update.batch.unwrap());
            }
            self.history.overview_updates = Some(updates);
        }
        if let Some(mut updates) = self.history.updates.take() {
            if let Some(update) = updates.prepare() {
                let batch = update.batch.unwrap();
                self.apply_history_update(update, cx);
                updates.acknowledge(batch);
            }
            self.history.updates = Some(updates);
        }
    }
    fn apply_history_update(
        &mut self,
        update: zork_client_core::state::HistoryUpdate,
        cx: &mut Context<Self>,
    ) {
        let h = &mut self.history;
        let changed = live::from_update(&update, h.clock_offset != update.state.clock_offset_ms);
        let follow = !h.loaded || h.following_latest;
        let anchor_offset = h.scroll.logical_scroll_top();
        // The paging control occupies row zero. Anchor the first real record
        // while it is visible, retaining the space above it as a negative offset.
        let anchor_row = Some(anchor_offset.item_ix.max(1) - 1);
        let offset_in_item = if anchor_offset.item_ix == 0 {
            anchor_offset.offset_in_item
                - h.scroll
                    .bounds_for_item(0)
                    .map(|b| b.size.height)
                    .unwrap_or_default()
        } else {
            anchor_offset.offset_in_item
        };
        let reading_entry = anchor_row
            .and_then(|index| h.rows.get(index))
            .is_some_and(|row| row.activity.is_some());
        let anchor = anchor_row
            .and_then(|index| h.row_entry(index))
            .map(|e| e.id.clone());
        let previous_count = h.rows.len();
        h.records = update.state.records.clone();
        h.entries = update.state.entries.clone();
        h.older = update.state.older.clone();
        h.latest = update.state.latest.clone();
        h.busy = update.state.loading;
        h.loading_older = update.state.loading_older;
        h.loaded = update.state.loaded;
        h.error = update.state.error.clone();
        h.clock_offset = update.state.clock_offset_ms;
        if update.reset || update.entries.is_some() {
            h.update_metrics();
            h.projection = Projection::new(h.entries.iter());
            h.expanded = h.projection.restore_expansion(
                h.entries.iter(),
                &h.expanded,
                if reading_entry && (update.prepended || !follow) {
                    anchor.as_deref()
                } else {
                    None
                },
            );
            h.rows = h.projection.rows(h.entries.iter(), &h.expanded);
            h.scroll.splice(1..previous_count + 1, h.rows.len());
            h.scroll.clone().with_uniform_item_height(px(26.));
            if update.prepended || !follow {
                if let Some(index) = anchor.and_then(|id| h.row_for_id(&id)) {
                    h.scroll.scroll_to(ListOffset {
                        item_ix: index + 1,
                        offset_in_item,
                    });
                }
            } else {
                h.scroll.scroll_to_end();
            }
        }
        if update.reset || update.entries.is_some() {
            self.refresh_history_sources();
        }
        cx.emit(changed);
        zork_ui::components::region::invalidate(cx, &["history", "header"]);
    }
    fn apply_history_overview(
        &mut self,
        overview: &zork_client_core::state::SessionOverview,
        cx: &mut Context<Self>,
    ) {
        self.history.runtime = overview.runtime.clone();
        zork_ui::components::region::invalidate(cx, &["history"]);
    }
    fn history_name(&self) -> String {
        if let Some(p) = self
            .participants
            .iter()
            .find(|p| Some(&p.session_id) == self.history.session.as_ref())
        {
            return p.name.clone();
        }
        self.node_agents
            .iter()
            .find(|a| a["session_id"].as_str() == self.history.session.as_deref())
            .and_then(|a| a["name"].as_str())
            .unwrap_or("小伙伴")
            .to_owned()
    }
}

#[cfg(test)]
mod canvas_tests {
    use super::*;
    #[gpui::test]
    fn partner_history_tracks_clicked_session_across_switches_and_chat_restore(
        cx: &mut gpui::TestAppContext,
    ) {
        let view = cx.new(|cx| {
            RootView::new(
                Arc::new(StationClient::new("http://127.0.0.1:9", None)),
                None,
                cx,
            )
        });
        view.update(cx, |view, cx| {
            view.selected_session = Some("chat".into());
            view.node_agents = vec![
                serde_json::json!({"id":"a", "name":"Alice", "session_id":"session-a"}),
                serde_json::json!({"id":"b", "name":"Bob", "session_id":"session-b"}),
            ]
            .into();
            view.restore_chat_history(cx);
            view.toggle_history("session-a", cx);
            assert_eq!(view.history.session.as_deref(), Some("session-a"));
            assert_eq!(view.history_name(), "Alice");
            assert!(view.history.subscription.is_some());
            view.history.detail = Some("alice-event".into());

            // Clicking another avatar switches the open page instead of closing it.
            view.toggle_history("session-b", cx);
            assert!(view.history.open);
            assert_eq!(view.history.session.as_deref(), Some("session-b"));
            assert_eq!(view.history_name(), "Bob");
            assert!(view.history.detail.is_none());
            assert_eq!(view.selected_session.as_deref(), Some("chat"));

            // Empty identities never fall back to an unrelated conversation.
            view.toggle_history("", cx);
            assert_eq!(view.history.session.as_deref(), Some("session-b"));
            view.save_chat_history();
            view.selected_session = Some("other-chat".into());
            view.restore_chat_history(cx);
            assert!(!view.history.open);
            view.save_chat_history();
            view.selected_session = Some("chat".into());
            view.restore_chat_history(cx);
            assert_eq!(view.history.session.as_deref(), Some("session-b"));
            assert_eq!(view.history_name(), "Bob");
            assert!(view.history.open);

            view.toggle_history("session-b", cx);
            assert!(!view.history.open);
            assert!(view.history.subscription.is_none());
            view.toggle_history("session-b", cx);
            assert!(view.history.open);
            assert_eq!(view.history.session.as_deref(), Some("session-b"));
        });
    }
}

#[cfg(feature = "headless-bench")]
impl RootView {
    pub fn benchmark_observe_history(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Arc<zork_client_core::state::History> {
        let conversation = self
            .core_device
            .conversation(self.history.session.as_deref().unwrap());
        let source = conversation.history();
        let fixture = zork_client_core::state::HistoryData {
            records: self.history.records.clone(),
            entries: self.history.entries.clone(),
            runtime: self.history.runtime.clone(),
            loaded: true,
            ..Default::default()
        };
        let mut state = (*conversation.snapshot()).clone();
        state.overview = Arc::new(zork_client_core::state::SessionOverview::fixture(&fixture));
        conversation.seed(state);
        source.seed(fixture);
        self.observe_history(cx).unwrap()
    }

    pub fn benchmark_history_live_clock(&mut self, cx: &mut Context<Self>) {
        self.history.clock_offset = self.history.now() - model::now();
        self.history.fixed_now = None;
        cx.emit(live::HistoryChanged::clock());
        zork_ui::components::region::invalidate(cx, &["history"]);
    }

    pub fn benchmark_history_runtime(
        &mut self,
        runtime: zork_client_core::state::HistoryRuntime,
        cx: &mut Context<Self>,
    ) {
        self.history.runtime = Some(runtime);
        zork_ui::components::region::invalidate(cx, &["history"]);
    }

    pub fn benchmark_prepend_history(
        &mut self,
        records: Vec<crate::session_history::Record>,
        cx: &mut Context<Self>,
    ) {
        let entries = crate::session_history::entries(&records);
        self.apply_history_update(
            zork_client_core::state::HistoryUpdate {
                state: Arc::new(zork_client_core::state::HistoryData {
                    records: records.into(),
                    entries: entries.into(),
                    loaded: true,
                    older: Some("fixture-older".into()),
                    ..Default::default()
                }),
                entries: None,
                prepended: true,
                reset: true,
                batch: None,
                cursor: zork_client_core::observe::Cursor {
                    source: 0,
                    revision: 0,
                },
            },
            cx,
        );
    }

    pub fn benchmark_scroll_history(&mut self, item: usize, offset: f32, cx: &mut Context<Self>) {
        self.history.positioned = true;
        self.history.following_latest = false;
        self.history.scroll.scroll_to(gpui::ListOffset {
            item_ix: item,
            offset_in_item: px(offset),
        });
        zork_ui::components::region::invalidate(cx, &["history"]);
    }
}

impl HistoryHost for RootView {
    fn history(&self) -> &zork_ui::history_page::State {
        &self.history.ui
    }
    fn history_mut(&mut self) -> &mut zork_ui::history_page::State {
        &mut self.history.ui
    }
    fn history_text(&self) -> zork_ui::resources::Text {
        let locale = self.locale;
        zork_ui::resources::Text(Rc::new(move |key| locale.text(key).into()))
    }
    fn history_subject(
        &self,
        activity: &Activity,
        entry: &Entry,
    ) -> (Option<String>, Option<zork_ui::history_page::Jump>) {
        self.activity_subject(activity, entry)
    }
    fn history_source(
        &self,
        cx: &gpui::App,
    ) -> zork_ui::components::liquid::overlay::SourceBinding {
        self.history_details.read(cx).source()
    }
    fn history_action(&mut self, action: zork_ui::history_page::Action, cx: &mut Context<Self>) {
        match action {
            zork_ui::history_page::Action::Scroll => {
                self.interrupt_message_scroll(cx);
                self.scroll_active = true;
                self.scroll_resume_task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(150))
                        .await;
                    let _ = this.update(cx, |v, cx| {
                        v.scroll_active = false;
                        v.save_reading_position();
                        v.notify_navigation(cx);
                        zork_ui::components::region::invalidate(cx, &["transcript"]);
                    });
                }));
            }
            zork_ui::history_page::Action::LoadOlder => {
                self.load_history(self.history.older.is_some(), cx)
            }
            zork_ui::history_page::Action::OpenEntry(id) => {
                self.history.detail = Some(id);
                self.history.agent_detail = None;
            }
            zork_ui::history_page::Action::Jump(jump) => self.jump_history_target(jump, cx),
        }
    }
    fn history_paging(&self) -> zork_ui::history_page::Paging {
        zork_ui::history_page::Paging {
            older: self.history.older.is_some(),
            busy: self.history.busy,
            loading_older: self.history.loading_older,
            loaded: self.history.loaded,
            error: self.history.error.is_some(),
        }
    }
    fn history_statistics(&mut self) -> zork_ui::history_page::Statistics {
        self.history_statistics_data()
    }
    fn history_row_built(&self) {
        #[cfg(feature = "headless-bench")]
        self.benchmark_rows.set(self.benchmark_rows.get() + 1);
    }
}
