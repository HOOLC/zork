//! Root view driven by the station-owned local IM entry.
//!
//! Device-owned conversations with virtualized messages, comments, files and history.
//! Event subscriptions drive live messages and activity; disconnected streams reconnect with backoff.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    div, prelude::*, px, rgb, Context, Div, Entity, FocusHandle, FollowMode, KeyDownEvent,
    ListAlignment, ListState, Render, Styled, Task, Window,
};

#[cfg(feature = "headless-bench")]
use crate::api::SessionStatus;
use crate::api::{AgentStatus, ProductTask, Role, SessionSummary, StationClient};
use crate::automation::{AutomationElementExt, AutomationRole};
use crate::components::text_input::{
    ComposerEdited, ComposerFilesPasted, ComposerInput, ComposerLayoutChanged, ComposerSubmit,
};
use crate::design::ZORK_UI;
use crate::i18n::{self, Locale};
use crate::shell::{ShellRoute, ShellState};
use crate::transcript::should_render_live_activity;
pub use crate::transcript::{Transcript, TranscriptLine};
use zork_ui::components::loading;
use zork_ui::history_page::Host as _;

mod agents;
#[cfg(feature = "headless-bench")]
pub mod benchmark;
mod cache;
mod comments;
mod composer_surface;
mod drive;
mod files;
mod history;
mod interactions;
mod message_presentation;
mod multi_agent;
mod new_chat;
mod panel_layout;
mod session_activity;

#[allow(non_snake_case)]
fn BG() -> u32 {
    ZORK_UI.palette.canvas
}
#[allow(non_snake_case)]
fn PROMPT() -> u32 {
    ZORK_UI.palette.prompt
}
#[allow(non_snake_case)]
fn BORDER() -> u32 {
    ZORK_UI.palette.border
}
#[allow(non_snake_case)]
fn TEXT() -> u32 {
    ZORK_UI.palette.text
}
#[allow(non_snake_case)]
fn DIM() -> u32 {
    ZORK_UI.palette.muted
}

#[cfg(test)]
use crate::api::SseEvent;
#[cfg(test)]
use zork_client_core::conversation::{decode_sse_event, DecodedSseEvent};

pub enum DesktopAction {
    ManageNode,
    SelectChatDevice(String),
}
impl gpui::EventEmitter<DesktopAction> for RootView {}
pub struct NavigationChanged {
    pub selection: crate::desktop::navigation::Selection,
    pub locale: Locale,
}
impl gpui::EventEmitter<NavigationChanged> for RootView {}
pub struct RootView {
    browser: Entity<crate::browser::BrowserPanel>,
    #[cfg(any(test, feature = "headless-bench"))]
    benchmark_offline: bool,
    #[cfg(feature = "headless-bench")]
    benchmark_rows: Rc<std::cell::Cell<usize>>,
    #[cfg(feature = "headless-bench")]
    benchmark_message_kinds: Rc<std::cell::Cell<u128>>,
    #[cfg(feature = "headless-bench")]
    benchmark_artifact_cards: Rc<std::cell::Cell<usize>>,
    #[cfg(feature = "headless-bench")]
    benchmark_artifact_indices: std::cell::RefCell<std::collections::HashSet<usize>>,
    new_chat_page: Option<Entity<zork_ui::new_chat::Page>>,
    first_chat_welcome: bool,
    new_chat_devices: zork_client_core::new_chat::Choice,
    new_chat_updates: Option<Task<()>>,
    local_cache: Option<(Arc<crate::desktop::store::ClientStore>, String)>,
    delivery_task: Option<Task<()>>,
    queued_count: usize,
    transcript_deliveries: zork_client_core::state::MessageDeliveries,
    restore_reading_pending: bool,
    node_agents: Arc<Vec<serde_json::Value>>,
    device_navigation: Option<Entity<crate::desktop::navigation::DeviceNavigation>>,
    device_name: Option<zork_ui::device_name::DeviceName>,
    tasks_by_leader: Arc<HashMap<String, Vec<ProductTask>>>,
    active_leader: Option<String>,
    history: history::HistoryState,
    chat_histories: HashMap<String, history::HistoryState>,
    session_activity_preview: Option<session_activity::SessionActivityPreview>,
    client: Arc<StationClient>,
    core_device: Arc<zork_client_core::state::Device>,
    device_updates: Option<zork_client_core::state::DeviceSubscription>,
    core_conversation: Option<Arc<zork_client_core::state::Conversation>>,
    conversation_updates: Option<zork_client_core::state::ConversationSubscription>,
    frame_delivery: zork_ui::components::frame_delivery::FrameDelivery,
    regions: zork_ui::components::region::Regions<Self>,
    shell: ShellState,
    locale: Locale,

    // Device sessions
    sessions: Arc<Vec<SessionSummary>>,
    sessions_loaded: bool,

    drive: drive::DriveState,

    mesh_status: crate::api::MeshStatus,

    selected_session: Option<String>,
    preview_original: Option<crate::desktop::navigation::Selection>,
    preview_overlay: bool,
    agent_online: bool,
    access_revoked: bool,
    last_confirmed_at: Option<String>,
    stop_pending: bool,

    // Transcript
    lines: Transcript,
    transcript_lookup: zork_client_core::state::TranscriptLookup,
    transcript_render_cache: crate::components::message::TranscriptRenderCache,
    older_cursor: Option<String>,
    has_older: bool,
    loading_older: bool,
    messages_loading: bool,
    messages_failed: bool,
    messages_request: u64,
    activity: Option<AgentStatus>,
    participants: Vec<crate::api::ParticipantStatus>,
    composer_surface: composer_surface::ComposerSurface,
    activity_revision: u64,

    // Conversation composer
    composer_input: Entity<ComposerInput>,
    draft_state: Arc<zork_client_core::state::Draft>,
    draft_task: Option<Task<()>>,
    comment_popover: Option<comments::CommentPopover>,
    /// Reply inputs of the draft quotes, by draft comment id.
    draft_inputs: HashMap<String, Entity<ComposerInput>>,
    focus_draft: Option<String>,
    /// The main input shows "补充说明（可选）" while drafts exist.
    extra_placeholder: bool,
    multi_agent: multi_agent::MultiAgentState,
    transcript_selection: Rc<std::cell::RefCell<crate::components::selection::TranscriptSelection>>,
    preparing_files: usize,
    file_ui: files::UiState,
    composer_surface_width: f32,
    composer_centering: panel_layout::Centering,
    composer_editor_height: f32,
    composer_overlay_height: f32,
    overlay_focus: FocusHandle,
    focus_initialized: bool,
    canceling: bool,
    connection_error: Option<String>,
    error: Option<String>,

    message_motion: message_presentation::MessageMotion,
    history_details: Entity<zork_ui::history_details::Details>,
    files_menu: Entity<zork_ui::conversation_contents::Menu>,
    transcript_list: ListState,
    updates_task: Option<Task<()>>,
    sse_task: Option<Task<()>>,
    scroll_active: bool,
    scroll_resume_task: Option<Task<()>>,
}

#[derive(Clone)]
pub struct InspectResource {
    pub target: zork_client_core::resources::InspectionTarget,
}
impl gpui::EventEmitter<InspectResource> for RootView {}

impl RootView {
    pub fn set_applications(
        &mut self,
        applications: Arc<Vec<zork_client_core::pages::ApplicationEntry>>,
        cx: &mut Context<Self>,
    ) {
        self.browser
            .update(cx, |browser, cx| browser.set_applications(applications, cx));
    }

    fn message_link_handler(&self, cx: &Context<Self>) -> Rc<dyn Fn(&str, &mut gpui::App)> {
        let root = cx.entity().downgrade();
        Rc::new(move |url, cx| {
            let action = root.update(cx, |view, _cx| {
                view.core_device()
                    .link_action(view.selected_session.as_deref(), url)
            });
            match action {
                Ok(zork_client_core::pages::LinkAction::Embedded(url)) => {
                    let _ = root.update(cx, |view, cx| {
                        let host = view.browser_host();
                        view.browser.update(cx, |panel, cx| {
                            panel.set_host(host, cx);
                            panel
                                .set_connection(view.client.clone(), view.selected_session.clone());
                            panel.open_shared_link(url, cx);
                        });
                        cx.notify();
                    });
                }
                Ok(zork_client_core::pages::LinkAction::External(url)) => cx.open_url(&url),
                Err(_) => {}
            }
        })
    }
    fn browser_host(&self) -> String {
        format!(
            "{}/{}",
            self.local_cache
                .as_ref()
                .map(|(_, id)| id.as_str())
                .unwrap_or("client"),
            self.selected_session.as_deref().unwrap_or("global")
        )
    }
    pub(crate) fn hide_browser(&mut self, cx: &mut Context<Self>) {
        self.browser
            .update(cx, |browser, _| browser.set_visible(false));
    }
    fn new(
        client: Arc<StationClient>,
        local_cache: Option<(Arc<crate::desktop::store::ClientStore>, String)>,
        cx: &mut Context<Self>,
    ) -> Self {
        let locale = i18n::load_locale(
            &i18n::preferences_path(),
            std::env::var("ZORK_GUI_LOCALE").ok().as_deref(),
        );
        cx.on_release(|view, cx| view.release_file_previews(cx))
            .detach();
        zork_client_core::desktop::trace_startup("gui.workspace_inputs_begin");
        let composer_input =
            cx.new(|cx| ComposerInput::new(locale.text("composer_placeholder"), cx));
        cx.subscribe(
            &composer_input,
            |view, _input, _event: &ComposerSubmit, cx| {
                if view.selected_session.is_some() {
                    view.send_composer(cx);
                }
            },
        )
        .detach();
        cx.subscribe(
            &composer_input,
            |view, _, event: &ComposerFilesPasted, cx| {
                view.paste_files(&event.0, cx);
            },
        )
        .detach();
        cx.subscribe(&composer_input, |view, _, _: &ComposerEdited, cx| {
            view.save_draft(cx);
        })
        .detach();
        cx.subscribe(&composer_input, |_, _, _: &ComposerLayoutChanged, cx| {
            zork_ui::components::region::invalidate(cx, &["composer", "home"]);
        })
        .detach();
        let files_menu = cx.new(|cx| {
            zork_ui::conversation_contents::Menu::new(
                zork_ui::resources::Text(Rc::new(|key| crate::i18n::Locale::ZhCn.text(key).into())),
                cx,
            )
        });
        cx.subscribe(
            &files_menu,
            |view, _, event: &zork_ui::conversation_contents::OpenChanged, cx| {
                view.set_conversation_files_open(event.0);
                zork_ui::components::region::invalidate_all(cx);
            },
        )
        .detach();
        cx.subscribe(
            &files_menu,
            |view, _, event: &zork_ui::conversation_contents::All, cx| {
                view.open_content_tab(
                    match event.0 {
                        zork_ui::conversation_contents::Kind::Page => {
                            zork_client_core::pages::ContentKind::Page
                        }
                        zork_ui::conversation_contents::Kind::File => {
                            zork_client_core::pages::ContentKind::File
                        }
                    },
                    cx,
                );
            },
        )
        .detach();
        let history_details = cx.new(|cx| {
            zork_ui::history_details::Details::new(
                zork_ui::resources::Text(Rc::new(|key| crate::i18n::Locale::ZhCn.text(key).into())),
                cx,
            )
        });
        cx.subscribe(
            &history_details,
            |view, _, _: &zork_ui::history_details::Closed, cx| {
                view.history.detail = None;
                view.history.agent_detail = None;
                zork_ui::components::region::invalidate_all(cx);
            },
        )
        .detach();
        let browser = {
            let browser = cx.new(crate::browser::BrowserPanel::new);
            cx.subscribe(
                &browser,
                |_, _, _: &crate::browser::BrowserVisibility, cx| {
                    zork_ui::components::region::invalidate_all(cx)
                },
            )
            .detach();
            cx.subscribe(&browser, |_, _, _: &crate::browser::BrowserResized, cx| {
                // Width changes invalidate cached bounds without invalidating
                // unrelated navigation, history data or message contents.
                cx.notify();
            })
            .detach();
            cx.subscribe(
                &browser,
                |view, _, closed: &crate::browser::NativePageClosed, cx| {
                    view.close_content_tab(&closed.0);
                    if closed.0 == "history" {
                        view.close_history();
                    }
                    zork_ui::components::region::invalidate_all(cx);
                },
            )
            .detach();
            cx.subscribe(
                &browser,
                |view, _, selection: &crate::browser::BrowserSelection, cx| {
                    if view.preview_original.is_some() || selection.host != view.browser_host() {
                        return;
                    }
                    let context = selection.inspection.context();
                    view.composer_input.update(cx, |input, cx| {
                        input.set_value(format!("{}{context}", input.value()), cx);
                    });
                    view.save_draft(cx);
                    zork_ui::components::region::invalidate_all(cx);
                },
            )
            .detach();
            browser
        };
        zork_client_core::desktop::trace_startup("gui.workspace_inputs_ready");
        Self {
            browser,
            #[cfg(any(test, feature = "headless-bench"))]
            benchmark_offline: false,
            #[cfg(feature = "headless-bench")]
            benchmark_rows: Default::default(),
            #[cfg(feature = "headless-bench")]
            benchmark_message_kinds: Default::default(),
            #[cfg(feature = "headless-bench")]
            benchmark_artifact_cards: Default::default(),
            #[cfg(feature = "headless-bench")]
            benchmark_artifact_indices: Default::default(),
            new_chat_page: None,
            first_chat_welcome: false,
            new_chat_devices: Default::default(),
            new_chat_updates: None,
            draft_state: Arc::new(Default::default()),
            draft_task: None,
            comment_popover: None,
            draft_inputs: HashMap::new(),
            focus_draft: None,
            extra_placeholder: false,
            multi_agent: Default::default(),
            transcript_selection: Rc::new(std::cell::RefCell::new(Default::default())),
            local_cache: local_cache.clone(),
            delivery_task: None,
            queued_count: 0,
            transcript_deliveries: Default::default(),
            restore_reading_pending: false,
            node_agents: Arc::new(Vec::new()),
            device_navigation: None,
            device_name: None,
            tasks_by_leader: Arc::new(HashMap::new()),
            active_leader: None,
            history: history::HistoryState::default(),
            chat_histories: HashMap::new(),
            session_activity_preview: None,
            core_device: {
                zork_client_core::desktop::trace_startup("gui.workspace_device_begin");
                let device =
                    zork_client_core::state::Device::open(client.clone(), local_cache, true);
                zork_client_core::desktop::trace_startup("gui.workspace_device_ready");
                device
            },
            device_updates: None,
            core_conversation: None,
            conversation_updates: None,
            frame_delivery: Default::default(),
            regions: Default::default(),
            client,
            shell: ShellState::default(),
            locale,
            sessions: Arc::new(Vec::new()),
            sessions_loaded: false,

            drive: drive::DriveState::default(),

            mesh_status: crate::api::MeshStatus::default(),

            selected_session: None,
            preview_original: None,
            preview_overlay: false,
            agent_online: false,
            access_revoked: false,
            last_confirmed_at: None,
            stop_pending: false,

            lines: Transcript::new(),
            transcript_lookup: Default::default(),
            transcript_render_cache: Default::default(),
            older_cursor: None,
            has_older: false,
            loading_older: false,
            messages_loading: false,
            messages_failed: false,
            messages_request: 0,
            activity: None,
            participants: vec![],
            composer_surface: composer_surface::ComposerSurface::default(),
            activity_revision: 0,
            composer_input,
            preparing_files: 0,
            file_ui: Default::default(),
            composer_surface_width: 744.,
            composer_centering: Default::default(),
            composer_editor_height: 40.,
            composer_overlay_height: 114.,
            overlay_focus: cx.focus_handle(),
            focus_initialized: false,
            canceling: false,
            connection_error: None,
            error: None,
            message_motion: Default::default(),
            history_details,
            files_menu,
            transcript_list: ListState::new(1, ListAlignment::Top, px(500.)),
            updates_task: None,
            sse_task: None,
            scroll_active: false,
            scroll_resume_task: None,
        }
    }

    // ---------------------------------------------------------------- tasks

    fn observe_scroll(list: &ListState, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        list.set_scroll_handler(move |_, _, cx| {
            let _ = view.update(cx, |v, cx| {
                v.interrupt_message_scroll(cx);
                // The selection pill does not follow the text while scrolling.
                if v.comment_popover.is_some() {
                    v.comment_popover = None;
                    zork_ui::components::region::invalidate(cx, &["overlays"]);
                }
                v.scroll_active = true;
                v.scroll_resume_task = Some(cx.spawn(async move |this, cx| {
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
            });
        });
    }

    fn start_background(&mut self, cx: &mut Context<Self>) {
        if self.updates_task.is_some() {
            return;
        }
        Self::observe_scroll(&self.transcript_list, cx);
        let mut updates = self.core_device.subscribe();
        if let Some(update) = updates.prepare() {
            let batch = update.batch.unwrap();
            self.apply_device_update(update, cx);
            updates.acknowledge(batch);
        }
        self.core_device.start();
        self.start_delivery(cx);
        let mut readiness = updates.readiness();
        self.device_updates = Some(updates);
        self.updates_task = Some(cx.spawn(async move |this, cx| {
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
    }

    fn apply_device_update(
        &mut self,
        update: zork_client_core::state::DeviceUpdate,
        cx: &mut Context<Self>,
    ) {
        use zork_client_core::state::Domains;
        let state = update.state;
        let changed = update.domains;
        if changed.contains(Domains::CONNECTION) {
            self.agent_online = state.online == Some(true);
            self.access_revoked = state.revoked;
            self.connection_error = state.connection_error.clone();
            // Reconnecting settles a preview that was kept while unreachable.
            self.sync_session_activity(cx);
            if let Some(ms) = state.confirmed_at_ms {
                self.last_confirmed_at =
                    chrono::DateTime::from_timestamp_millis(ms as i64).map(|t| t.to_rfc3339());
            }
        }
        if changed.contains(Domains::SESSIONS) {
            self.sessions = state.sessions.clone();
            self.sessions_loaded = state.sessions_loaded;
        }
        if changed.contains(Domains::AGENTS) {
            self.node_agents = state.agents.clone();
        }
        if changed.contains(Domains::TASKS) {
            self.tasks_by_leader = state.tasks.clone();
        }
        if changed.contains(Domains::ARTIFACTS) {
            self.drive.items = state.artifacts.clone();
            self.drive.pages = state.pages.clone();
            self.drive.contents = state.content_indices.clone();
        }
        if changed.contains(Domains::MESH) {
            self.mesh_status = state.mesh.as_ref().clone();
        }
        let mut regions = Vec::new();
        if changed
            .contains(Domains::CONNECTION | Domains::SESSIONS | Domains::AGENTS | Domains::TASKS)
        {
            regions.push("home");
        }
        if changed.contains(Domains::CONNECTION | Domains::SESSIONS) {
            regions.extend(["header", "composer", "history", "transcript"]);
        }
        if changed.contains(Domains::AGENTS | Domains::MESH) {
            regions.extend(["header", "transcript", "history"]);
        }
        if changed.contains(Domains::TASKS) {
            regions.extend(["header", "leader-sidebar"]);
        }
        if changed.contains(Domains::ARTIFACTS) {
            regions.extend([
                "header",
                "transcript",
                "conversation-content-pages",
                "conversation-content-files",
            ]);
        }
        if !regions.is_empty() {
            zork_ui::components::region::invalidate(cx, &regions);
        }
    }

    fn start_sse(&mut self, cx: &mut Context<Self>) {
        self.sse_task = None;
        self.conversation_updates = None;
        self.core_conversation = None;
        let Some(id) = self.selected_session.clone() else {
            return;
        };
        let conversation = self.core_device.conversation(&id);
        use zork_client_core::state::ConversationTopics as T;
        let mut updates = conversation.subscribe_topics(
            T::MESSAGES | T::ACTIVITY | T::PARTICIPANTS | T::LOADING | T::ARRIVALS,
        );
        if let Some(update) = updates.prepare() {
            let batch = update.batch.unwrap();
            self.apply_conversation_update(update, cx);
            updates.acknowledge(batch);
        }
        #[cfg(not(any(test, feature = "headless-bench")))]
        conversation.start();
        #[cfg(any(test, feature = "headless-bench"))]
        if !self.benchmark_offline {
            conversation.start();
        }
        self.core_conversation = Some(conversation);
        let mut readiness = updates.readiness();
        self.conversation_updates = Some(updates);
        self.sse_task = Some(cx.spawn(async move |this, cx| {
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
                if !zork_ui::components::frame_delivery::FrameDelivery::request(&this, cx, |view| {
                    &mut view.frame_delivery
                }) {
                    return;
                }
            }
        }));
    }

    fn deliver_core_updates(&mut self, cx: &mut Context<Self>) {
        if let Some(mut updates) = self.device_updates.take() {
            if let Some(update) = updates.prepare() {
                let batch = update.batch.unwrap();
                self.apply_device_update(update, cx);
                updates.acknowledge(batch);
            }
            self.device_updates = Some(updates);
        }
        if let Some(mut updates) = self.conversation_updates.take() {
            if let Some(update) = updates.prepare() {
                let batch = update.batch.unwrap();
                self.apply_conversation_update(update, cx);
                updates.acknowledge(batch);
            }
            self.conversation_updates = Some(updates);
        }
        self.deliver_history_updates(cx);
        self.deliver_session_activity_updates(cx);
    }
    fn apply_conversation_update(
        &mut self,
        update: zork_client_core::state::ConversationUpdate,
        cx: &mut Context<Self>,
    ) {
        let state = update.state;
        let mut regions = Vec::new();
        let unread = self.message_motion.unread;
        let scroll_anchor = self.transcript_list.logical_scroll_top();
        let following = self.transcript_list.is_following_tail()
            || self.message_motion.scroll.is_some()
            || self.transcript_list.is_scrolled_to_end().unwrap_or(false)
            || (!self.lines.is_empty()
                && scroll_anchor.item_ix == self.lines.len().saturating_sub(1));
        if let Some(edits) = update.messages {
            self.transcript_render_cache
                .apply(&state.lines, &edits, update.reset);
            if !update.reset {
                self.refresh_changed_message_sources(&edits);
            }
            self.lines = state.lines.clone();
            self.transcript_lookup = state.lookup.clone();
            self.transcript_deliveries = state.deliveries.clone();
            if update.reset {
                self.refresh_history_sources();
            }
            if self.history.open {
                regions.push("history");
            }
            for edit in edits {
                self.transcript_list.splice(edit.remove, edit.insert.len());
            }
            // The trailing activity row depends on the latest message (the
            // final reply hides it), so its cached height is stale too.
            self.transcript_list
                .remeasure_items(self.lines.len()..self.lines.len() + 1);
            if self.restore_reading_pending && state.loaded {
                self.restore_reading_position();
                self.restore_reading_pending = false;
            }
            regions.push("transcript");
        }
        if update.message_arrivals.count > 0 && following {
            self.transcript_list.set_follow_mode(FollowMode::Normal);
            self.transcript_list.scroll_to(scroll_anchor);
        }
        self.track_message_arrivals(&update.message_arrivals, following, cx);
        if self.message_motion.unread != unread {
            regions.push("composer");
        }
        if update.activity_changed || update.participants_changed {
            self.activity = state.activity.clone();
            self.participants = state.participants.as_ref().clone();
            self.stop_pending = state.stop_pending;
            self.canceling = state.canceling;
            self.activity_revision = self.activity_revision.wrapping_add(1);
            self.sync_session_activity(cx);
            self.transcript_list
                .remeasure_items(self.lines.len()..self.lines.len() + 1);
            regions.extend(["transcript", "header", "composer"]);
        }
        if update.loading_changed {
            self.messages_loading = state.loading;
            self.messages_failed = state.error.is_some() && !state.loaded;
            let older_done = self.loading_older && !state.loading_older;
            self.loading_older = state.loading_older;
            self.older_cursor = state.older_cursor.clone();
            self.has_older = state.older_cursor.is_some();
            self.error = state.error.clone();
            if older_done {
                self.older_loaded(cx);
            }
            regions.extend(["transcript", "composer"]);
        }
        if !regions.is_empty() {
            zork_ui::components::region::invalidate(cx, &regions);
        }
    }

    fn select_session(&mut self, id: &str, cx: &mut Context<Self>) {
        self.first_chat_welcome = false;
        self.navigate_shell(ShellRoute::Task(id.to_owned()), cx);
    }

    fn load_session(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.selected_session.as_deref() == Some(id) && !self.messages_failed {
            return;
        }
        self.messages_request = self.messages_request.wrapping_add(1);
        self.restore_reading_pending = true;
        self.messages_loading = true;
        self.messages_failed = false;
        if self.preview_original.is_none() {
            self.save_draft(cx);
            self.save_reading_position();
            self.save_chat_history();
        }
        self.message_motion = Default::default();
        self.selected_session = Some(id.to_owned());
        if self.preview_original.is_none() {
            self.persist_cache(zork_client_core::preferences::ViewState::LastSession, &id);
        }
        self.stop_pending = false;
        if self.preview_original.is_none() {
            self.restore_draft(cx);
        }
        if let Some(leader) = self.selected_leader_id() {
            self.active_leader = Some(leader);
        } else {
            self.active_leader = self
                .tasks_by_leader
                .iter()
                .find(|(_, tasks)| tasks.iter().any(|t| t.session_id.as_deref() == Some(id)))
                .map(|(leader, _)| leader.clone());
        }
        if self.preview_original.is_none() {
            self.refresh_agents(cx);
        }
        let old = self.lines.len();
        self.lines = Transcript::new();
        self.transcript_lookup = Default::default();
        self.transcript_deliveries = Default::default();
        self.transcript_list.splice(0..old, 0);
        self.transcript_list.set_follow_mode(FollowMode::Tail);
        self.transcript_list.scroll_to_end();
        self.older_cursor = None;
        self.has_older = false;
        self.loading_older = false;
        self.activity = None;
        self.participants.clear();
        self.session_activity_preview = None;
        self.composer_surface = composer_surface::ComposerSurface::default();
        self.activity_revision = self.activity_revision.wrapping_add(1);
        self.canceling = false;

        self.error = None;
        self.start_sse(cx);
        self.restore_chat_history(cx);
        self.notify_navigation(cx);
        zork_ui::components::region::invalidate_all(cx);
    }

    fn navigate_shell(&mut self, route: ShellRoute, cx: &mut Context<Self>) {
        self.shell.navigate(route.clone());
        self.restore_shell_route(route, cx);
        self.notify_navigation(cx);
    }

    fn restore_shell_route(&mut self, route: ShellRoute, cx: &mut Context<Self>) {
        // Apply focus after navigation has restored the destination composer.
        if self.preview_original.is_none() {
            self.focus_initialized = false;
        }
        match route {
            ShellRoute::Home => self.reset_session(cx),
            ShellRoute::Task(id) => self.load_session(&id, cx),
        }
        zork_ui::components::region::invalidate_all(cx);
    }

    fn reset_session(&mut self, cx: &mut Context<Self>) {
        self.messages_request = self.messages_request.wrapping_add(1);
        self.messages_loading = false;
        self.messages_failed = false;
        if self.preview_original.is_none() {
            self.save_draft(cx);
            self.save_chat_history();
        }
        self.selected_session = None;
        if self.preview_original.is_none() {
            self.restore_draft(cx);
        }

        self.sse_task = None;
        self.core_conversation = None;
        self.conversation_updates = None;
        self.restore_chat_history(cx);
        let old = self.lines.len();
        self.lines = Transcript::new();
        self.transcript_lookup = Default::default();
        self.transcript_deliveries = Default::default();
        self.transcript_list.splice(0..old, 0);
        self.older_cursor = None;
        self.has_older = false;
        self.loading_older = false;
        self.activity = None;
        self.participants.clear();
        self.session_activity_preview = None;
        self.composer_surface = composer_surface::ComposerSurface::default();
        self.activity_revision = self.activity_revision.wrapping_add(1);
        self.canceling = false;
        self.error = None;

        zork_ui::components::region::invalidate_all(cx);
    }

    fn load_older(&mut self, _cx: &mut Context<Self>) {
        if let Some(conversation) = &self.core_conversation {
            conversation.load_older();
        }
    }

    fn can_send_selected(&self) -> bool {
        self.sessions
            .iter()
            .find(|s| Some(&s.session_id) == self.selected_session.as_ref())
            .is_some_and(zork_client_core::conversation::can_send)
    }
    fn send_composer(&mut self, cx: &mut Context<Self>) {
        if self.preparing_files > 0 {
            return;
        }
        let Some(id) = self.selected_session.clone() else {
            return;
        };
        if !self.can_send_selected() {
            zork_ui::components::region::invalidate_all(cx);
            return;
        }
        // Every draft quote needs its own reply, or it is removed first.
        if !self.drafts_ready(cx) {
            return;
        }
        // Submit the core's current draft, not a display snapshot that may
        // still show the previous text until its subscription update arrives.
        let text = self.core_device.draft(&id).text.clone();
        self.queue_message(id, text, cx);
    }

    fn cancel_session(&mut self, _cx: &mut Context<Self>) {
        if let Some(conversation) = &self.core_conversation {
            conversation.stop();
        }
    }

    fn focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        let focus_handle = self.composer_input.read(cx).focus_handle();
        window.focus(&focus_handle, cx);
    }
}

// ---------------------------------------------------------------- rendering

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.frame_delivery.enter() {
            self.deliver_core_updates(cx);
        }
        {
            let host = self.browser_host();
            let rail = if self.preview_overlay {
                self.device_navigation
                    .as_ref()
                    .map(|nav| nav.read(cx).width(window.viewport_size().width.as_f32()))
                    .unwrap_or(0.)
            } else if !self.shell.rail_open {
                8.
            } else if let Some(nav) = &self.device_navigation {
                nav.read(cx).width(window.viewport_size().width.as_f32())
            } else {
                ZORK_UI.layout.rail_width
            };
            let available = window.viewport_size().width.as_f32() - rail;
            let locale = self.locale;
            let visible =
                self.preview_original.is_none() && !self.has_conversation_artifact_preview();
            let moving = self.browser.update(cx, |browser, cx| {
                if self.preview_original.is_none() {
                    browser.set_host(host, cx);
                    browser.set_connection(self.client.clone(), self.selected_session.clone());
                }
                browser.set_presentation(available, locale, cx);
                browser.set_visible(visible && browser.is_open());
                browser.animate_panel(cx)
            });
            if moving {
                let root = cx.entity().downgrade();
                window.on_next_frame(move |_, cx| {
                    let _ = root.update(cx, |_, cx| {
                        // Keep the old invalidation strategy available only to
                        // the panel benchmark for same-binary comparisons.
                        #[cfg(feature = "headless-bench")]
                        if std::env::var_os("ZORK_BENCH_PANEL_FORCE_INVALIDATION").is_some() {
                            zork_ui::components::region::invalidate(
                                cx,
                                &["transcript", "composer", "history"],
                            );
                            return;
                        }
                        // Cached regions already re-render when their bounds change.
                        // Advancing the panel does not change their content.
                        cx.notify();
                    });
                });
            }
        }
        #[cfg(not(feature = "headless-bench"))]
        self.start_background(cx);
        #[cfg(feature = "headless-bench")]
        if !self.benchmark_offline {
            self.start_background(cx);
        }
        self.measure_composer_geometry(window, cx);
        if !self.focus_initialized && self.preview_original.is_none() {
            self.focus_initialized = true;
            let focus_handle = self.composer_input.read(cx).focus_handle();
            window.focus(&focus_handle, cx);
        }

        self.focus_artifact_preview(window, cx);
        self.sync_history_details(cx);
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .track_focus(&self.overlay_focus)
            .on_mouse_move(cx.listener(|v, event: &gpui::MouseMoveEvent, window, cx| {
                if v.transcript_selection.borrow_mut().update(event.position) {
                    zork_ui::components::region::invalidate(cx,&["transcript"]);
                }
                if let Some(nav) = &v.device_navigation {
                    nav.update(cx, |nav, cx| {
                        nav.resize(
                            event.position.x.as_f32(),
                            window.viewport_size().width.as_f32(),
                            cx,
                        )
                    });
                }
            }))
            // A click anywhere else dismisses the "引用回复" pill; text and the
            // pill itself stop propagation before this.
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|v, _, _, cx| {
                    if v.comment_popover.is_some() {
                        v.dismiss_selection(cx);
                    }
                }),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|v, event, window, cx| {
                    v.finish_text_selection(event, window, cx);
                    if let Some(nav) = &v.device_navigation {
                        nav.update(cx, |nav, cx| nav.finish_resize(cx));
                    }
                    v.browser.update(cx, |panel, _| panel.finish_resize());
                }),
            )
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(|v, event, window, cx| {
                    v.finish_text_selection(event, window, cx);
                    v.browser.update(cx, |panel, _| panel.finish_resize());
                    if let Some(nav) = &v.device_navigation {
                        nav.update(cx, |nav, cx| nav.finish_resize(cx));
                    }
                }),
            )
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
                if event.keystroke.modifiers.platform
                    && event.keystroke.key == "c"
                    && view.overlay_focus.is_focused(window)
                {
                    if let Some(popover) = &view.comment_popover {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                            popover.source.quote.clone(),
                        ));
                        cx.stop_propagation();
                        return;
                    }
                }
                if event.keystroke.key == "escape" && view.has_conversation_artifact_preview() {
                    view.close_conversation_artifact();
                    zork_ui::components::region::invalidate_all(cx);
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" && view.close_conversation_files(cx) {
                    zork_ui::components::region::invalidate_all(cx); cx.stop_propagation(); return;
                }
                if event.keystroke.key == "escape" && view.comment_popover.is_some() {
                    view.dismiss_selection(cx);
                    zork_ui::components::region::invalidate_all(cx);
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.modifiers.platform && !event.keystroke.modifiers.shift && event.keystroke.key == "b" {
                    view.shell.rail_open = !view.shell.rail_open;
                    zork_ui::components::region::invalidate_all(cx);
                    cx.stop_propagation();
                }
            }))
            .bg(rgb(ZORK_UI.palette.window))
            .text_color(rgb(TEXT()))
            .font_family("Inter Variable")
            .text_size(px(13.))
            .child(self.panel_resize_events(cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()

                    .child(if self.preview_overlay {
                        div()
                    } else if self.shell.rail_open {
                        if let Some(navigation) = &self.device_navigation {
                            div()
                                .w(px(navigation
                                    .read(cx)
                                    .width(window.viewport_size().width.as_f32())))
                                .flex_shrink_0()
                                .h_full()
                                .child(navigation.clone())
                        } else {
                            div()
                        }
                    } else {
                        div().w(px(8.)).flex_shrink_0()
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .flex()
                            .relative()
                            .bg(rgb({
                                BG()
                            }))
                            .overflow_hidden()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .min_h_0()
                                    .relative()
                                    .flex()
                                    .flex_col()
                                    .when(
                                        !self.agent_online
                                            && self.connection_error.is_some(),
                                        |v| {
                                            v.child(
                                                div()
                                                    .id("device-offline-banner")
                                                    .flex()
                                                    .items_center()
                                                    .gap_3()
                                                    .child(
                                                        gpui::svg()
                                                            .path("icons/offline.svg")
                                                            .size(px(20.))
                                                            .text_color(rgb(crate::design::ZORK_UI
                                                                .palette
                                                                .muted))
                                                            .flex_shrink_0(),
                                                    )
                                                    .px_4()
                                                    .py_2()
                                                    .text_size(px(12.))
                                                    .text_color(rgb(DIM()))
                                                    .border_b(gpui::px(zork_ui::design::BORDER_WIDTH))
                                                    .border_color(rgb(BORDER()))
                                                    .child(self.connection_hint())
                                                    .when(self.access_revoked, |v| {
                                                        v.child(
                                                            zork_ui::controls::button(
                                                                "device-reconnect",
                                                                self.locale
                                                                    .text("device_reconnect")
                                                                    .to_string(),
                                                                false,
                                                                true,
                                                            )
                                                            .h(px(28.))
                                                            .min_h(px(28.))
                                                                .on_click(cx.listener(
                                                                    |v, _, _, cx| {
                                                                        v.access_revoked = false;
                                                                        v.updates_task = None;
                                                                        v.start_background(cx);
                                                                        zork_ui::components::region::invalidate_all(cx);
                                                                    },
                                                                ))
                                                                .automation(
                                                                    AutomationRole::Button,
                                                                    self.locale
                                                                        .text("device_reconnect"),
                                                                ),
                                                        )
                                                    })
                                                    .automation(
                                                        AutomationRole::Status,
                                                        self.connection_hint(),
                                                    ),
                                            )
                                        },
                                    )
                                    .child(self.render_shell_content(window, cx))
                                    .when(self.selected_session.is_some() && self.preview_original.is_none(), |v| v.child(self.render_panel_tools(cx)))
                                    ,
                            )
                            .map(|v| {
                                if self.has_page_workspace(cx) {
                                    let native = self.browser.read(cx).is_native_page_active("history") && self.history.open;
                                    let content = self.active_content_kind(cx);
                                    let width = self.browser.read(cx).panel_width();
                                    return v.child(div().id("page-workspace").relative().w(px(width)).h_full().overflow_hidden()
                                        .border_l(gpui::px(zork_ui::design::BORDER_WIDTH)).border_color(rgb(BORDER()))
                                        .flex_shrink_0().flex().flex_col().min_h_0()
                                        .child(self.browser.clone())
                                        .when(native, |v| v.child(self.regions.element("history",
                                            gpui::StyleRefinement::default().w_full().flex_1().min_h_0(),
                                            cx, |v,window,cx| v.render_history_page(window, cx).id("history-page").automation(AutomationRole::ScrollArea, v.locale.text("history_title")).into_any_element())))
                                        .when_some(content, |v, kind| v.child(self.regions.element(drive::content_tab_id(kind),
                                            gpui::StyleRefinement::default().w_full().flex_1().min_w_0().min_h_0(),
                                            cx, move |v, window, cx| v.render_content_page(kind, window, cx).into_any_element())))
                                        .child(div().id("page-resize").absolute().left_0().top_0().h_full().w(px(8.))
                                            .occlude().cursor_col_resize()
                                            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|v, event: &gpui::MouseDownEvent, window, cx| {
                                                let pointer_width = (window.viewport_size().width - event.position.x).as_f32();
                                                v.browser.update(cx, |panel, _| panel.begin_resize(pointer_width));
                                                cx.stop_propagation();
                                            }))
                                            .automation(AutomationRole::Status, self.locale.text("panel_resize")))
                                        );
                                }
                                v
                            })
                            .when(self.preview_original.is_none() && (self.selected_session.is_some() || self.browser.read(cx).is_present()), |v| {
                                v.child(div().absolute().top(px(8.)).right(px(10.))
                                    .child(self.render_session_header("", cx)))
                            }),
                    ),
            )
            .when(self.preview_original.is_none(), |view| {
                view.child(self.history_details.clone())
                    .child(self.render_comment_popover(window, cx))
                    .child(self.render_conversation_artifact_preview(window, cx))
            })
    }
}

impl RootView {
    fn has_page_workspace(&self, cx: &gpui::App) -> bool {
        self.browser.read(cx).is_present()
    }

    fn panel_tools_right_inset(&self, cx: &gpui::App) -> f32 {
        // Reserve the toggle's space continuously as the panel closes.
        (56. - self.browser.read(cx).panel_width()).max(12.)
    }

    fn render_shell_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.browser.read(cx).is_expanded() {
            return div().w_0().h_full();
        }
        if matches!(self.shell.route(), ShellRoute::Home) {
            return div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(self.render_new_chat(window, cx));
        }
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .child(self.render_center_pane(window, cx))
    }

    fn render_center_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.preview_original.is_none() {
            self.sync_conversation_files(cx);
        }
        match self.selected_session.clone() {
            Some(session) => div()
                .relative()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.regions.element(
                    "transcript",
                    gpui::StyleRefinement::default().w_full().flex_1().min_h_0(),
                    cx,
                    |v, window, cx| v.render_transcript(window, cx).into_any_element(),
                ))
                .children(self.render_chat_header(&session, cx))
                .when(
                    self.preview_original.is_some() || self.can_send_selected(),
                    |pane| {
                        pane.child(div().absolute().bottom_0().left_0().w_full().child(
                            self.regions.auto_height(
                                "composer",
                                self.composer_surface_width,
                                cx,
                                |v, window, cx| {
                                    v.measure_composer(window, cx);
                                    v.render_composer(window, cx).into_any_element()
                                },
                            ),
                        ))
                    },
                ),
            None => div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .child(self.render_new_chat(window, cx)),
        }
    }

    /// The Chat header: the Chat's stacked agent avatars before its title,
    /// floating in the transcript's top clearance.
    fn render_chat_header(&self, session: &str, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let chat = self
            .device_navigation
            .as_ref()
            .and_then(|navigation| navigation.read(cx).chat(session));
        let title = chat
            .as_ref()
            .map(|chat| chat.title.clone())
            .filter(|title| !title.is_empty())
            .or_else(|| {
                self.sessions
                    .iter()
                    .find(|s| s.session_id == session)
                    .and_then(|s| s.title.clone())
            })
            .filter(|title| !title.trim().is_empty())?;
        let (discs, more) = match &chat {
            Some(chat) => (
                chat.avatar
                    .agents
                    .iter()
                    .map(|agent| zork_ui::components::message_row::Disc {
                        tint: agent.tint,
                        maker: agent.maker.clone(),
                        initial: agent.initial.clone(),
                    })
                    .collect::<Vec<_>>(),
                chat.avatar.more,
            ),
            None => zork_ui::components::message_row::presentation::agent_stack(
                &self.multi_agent.presented,
            ),
        };
        let canvas = ZORK_UI.palette.canvas;
        Some(
            div()
                .absolute()
                .top(px(6.))
                .left_0()
                .w_full()
                .h(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .max_w(px((self.composer_surface_width * 0.5).max(160.)))
                        .h(px(32.))
                        .px(px(10.))
                        .rounded_full()
                        .bg(rgb(canvas))
                        .flex()
                        .items_center()
                        .child(zork_ui::components::message_row::identity::chat_title(
                            "chat-header",
                            &discs,
                            more,
                            &title,
                            canvas,
                        )),
                )
                .into_any_element(),
        )
    }

    fn render_session_header(&mut self, _id: &str, cx: &mut Context<Self>) -> impl IntoElement {
        crate::desktop::ui::icon_button_sized(
            "conversation-browser",
            true,
            crate::desktop::ui::IconButtonSize::Compact,
        )
        .bg(rgb(ZORK_UI.palette.canvas))
        .child(crate::desktop::ui::icon("icons/columns.svg", 16.))
        .on_click(cx.listener(|v, _, _, cx| {
            v.browser.update(cx, |browser, cx| browser.toggle(cx));
            zork_ui::components::region::invalidate_all(cx);
        }))
        .automation(
            AutomationRole::Button,
            self.locale.text(if self.browser.read(cx).is_open() {
                "browser_hide"
            } else {
                "pages"
            }),
        )
    }

    fn render_panel_tools(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.configure_conversation_files(cx);
        let members = self
            .conversation_members()
            .into_iter()
            .map(|member| zork_ui::conversation_toolbar::Member {
                id: member.id,
                name: member.name,
            })
            .collect();
        zork_ui::conversation_toolbar::render(
            members,
            self.files_menu.clone(),
            self.panel_tools_right_inset(cx),
            self.locale.text("history_title").into(),
            cx,
            |v, id, cx| {
                if let Some(member) = v
                    .conversation_members()
                    .into_iter()
                    .find(|member| member.id == id)
                {
                    v.toggle_history(&member.session_id, cx);
                }
            },
        )
    }

    fn render_transcript(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let link_handler = self.message_link_handler(cx);
        let pending = self.transcript_deliveries.clone();
        let delivery_root = cx.entity().downgrade();
        let delivery_locale = self.locale;
        let lines = self.lines.clone();
        let documents = self.transcript_render_cache.prepare(&lines);
        let file_previews = self.file_ui.message_previews.clone();
        let item_count = lines.len();
        let content_width = self.composer_surface_width;
        // Live activity is the list's trailing item, right after the latest
        // message; it scrolls with the messages and follows the tail with them.
        let activity = self.render_session_activity(window, cx);
        let activity_row = activity.clone();

        self.transcript_selection.borrow_mut().begin_frame();
        let selection_state = self.transcript_selection.clone();
        let session_id = self.selected_session.clone().unwrap_or_default();
        let focus = self.overlay_focus.clone();
        let root = cx.entity().downgrade();
        let notify: Rc<dyn Fn(&mut gpui::App)> = Rc::new(move |cx| {
            let _ = root.update(cx, |_, cx| {
                zork_ui::components::region::invalidate(cx, &["transcript"])
            });
        });
        let local_name = self.device_name.as_ref().map(|name| name.display.clone());
        let local_agents = self
            .node_agents
            .iter()
            .filter_map(|a| a["id"].as_str().map(str::to_owned))
            .collect::<std::collections::HashSet<_>>();
        let mut aliases = self
            .mesh_status
            .peers
            .iter()
            .map(|p| (p.origin.clone(), p.name.clone()))
            .collect::<HashMap<_, _>>();
        let device = self.core_device.snapshot();
        let local_origin = device.info["sync"]["owner"]
            .as_str()
            .or(self.mesh_status.origin.as_deref());
        if let (Some(origin), Some(name)) = (local_origin, &local_name) {
            aliases.insert(origin.to_owned(), name.clone());
        }
        #[cfg(feature = "headless-bench")]
        let benchmark_rows = self.benchmark_rows.clone();
        #[cfg(feature = "headless-bench")]
        let benchmark_message_kinds = self.benchmark_message_kinds.clone();
        let arrivals = self.message_motion.arrivals.clone();
        let reader_root = cx.entity().downgrade();
        let locale = self.locale;
        let has_older = self.has_older;
        let loading_older = self.loading_older;
        let older_root = cx.entity().downgrade();
        let bottom_inset = if self.can_send_selected() {
            self.composer_overlay_height + 8.
        } else {
            0.
        };
        // Core's presentation of every row; the UI adds the one-screen
        // measurement, callbacks, the jump wash and draft marks.
        let presented = self.transcript_presentation(cx);
        let screen = (self.transcript_list.viewport_bounds().size.height.as_f32() - bottom_inset).max(0.);
        let reduce_motion = cx.reduce_motion();
        let row_context = multi_agent::RowContext {
            show_device: presented.multi_device && content_width >= multi_agent::PHONE_WIDTH,
            presented,
            heights: self.multi_agent.heights.clone(),
            omissions: self.multi_agent.omissions.clone(),
            content_width,
            screen,
            reply_text: multi_agent::reply_text(self.locale),
            reply_loading: self.multi_agent.reply_loading.clone(),
            wash: self.multi_agent.wash.as_ref().map(|wash| {
                (wash.id.clone(), wash.passage.clone(), self.wash_alpha(reduce_motion))
            }),
            drafts: Rc::new(self.draft_passages()),
            quote_label: self.locale.text("quote_message").into(),
            root: cx.entity().downgrade(),
        };
        let list = gpui::list(self.transcript_list.clone(), move |ix, window, cx| {
            if ix < lines.len() {
                #[cfg(feature = "headless-bench")]
                {
                    benchmark_rows.set(benchmark_rows.get() + 1);
                    benchmark_message_kinds
                        .set(benchmark_message_kinds.get() | benchmark::message_kind_mask(ix));
                }
                let TranscriptLine::Message {
                    role,
                    content,
                    metadata,
                    ..
                } = &lines[ix];
                let selection_text = documents[ix].selection_text(role, content);
                let decorations = row_context.decorations(
                    ix,
                    &lines,
                    &documents,
                    selection_text.clone(),
                    documents[ix].comment_documents(role, content),
                );
                let selection = crate::components::selection::SelectionContext::new(
                    format!(
                        "{}:{}",
                        session_id,
                        metadata
                            .id
                            .clone()
                            .unwrap_or_else(|| format!("legacy-{ix}"))
                    ),
                    crate::comments::CommentSource {
                        session_id: session_id.clone(),
                        message_id: metadata.id.clone(),
                        author: metadata.author_name.clone(),
                        author_agent_id: metadata.author_agent_id.clone(),
                        quote: String::new(),
                    },
                    selection_text,
                    selection_state.clone(),
                    focus.clone(),
                    notify.clone(),
                )
                .with_link_handler(link_handler.clone());
                // Laid-out heights feed the omission rule's one-screen distance.
                let heights = row_context.heights.clone();
                let height_key = metadata.id.clone().unwrap_or_else(|| format!("row-{ix}"));
                div()
                    .relative()
                    .child(
                        gpui::canvas(
                            move |bounds, _, _| {
                                heights.borrow_mut().record(
                                    &height_key,
                                    content_width,
                                    bounds.size.height.as_f32(),
                                );
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    // Keep the first message clear of floating controls; this
                    // space scrolls away with the message instead of forming a header.
                    .when(ix == 0, |row| {
                        row.relative()
                            .pt(px(32.))
                            .when(has_older || loading_older, |row| {
                                let older_root = older_root.clone();
                                // Use the first row's existing inset so pagination
                                // scrolls out of view without changing row heights.
                                row.child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .w_full()
                                        .h(px(32.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(12.))
                                        .text_color(rgb(DIM()))
                                        .child(if loading_older {
                                            loading::status(
                                                "messages-older-loading",
                                                locale.text("loading_earlier"),
                                            )
                                            .into_any_element()
                                        } else {
                                            zork_ui::controls::button(
                                                "load-older",
                                                locale.text("load_earlier").to_string(),
                                                false,
                                                true,
                                            )
                                            .h(px(28.))
                                            .min_h(px(28.))
                                            .on_click(move |_, _, cx| {
                                                    let _ = older_root.update(cx, |v, cx| {
                                                        v.load_older(cx);
                                                    });
                                                })
                                                .automation(
                                                    AutomationRole::Button,
                                                    locale.text("load_earlier"),
                                                )
                                                .into_any_element()
                                        }),
                                )
                            })
                    })
                    .child(render_line(
                        ix,
                        &lines[ix],
                        documents.shared(ix).unwrap(),
                        content_width,
                        Some(&selection),
                        window,
                        crate::transcript::message_device_label(
                            metadata,
                            &local_agents,
                            local_name.as_deref(),
                            &aliases,
                        ),
                        decorations,
                    ))
                    .when_some(crate::components::interaction::render(lines.shared(ix).unwrap(), &documents[ix], locale, &session_id, reader_root.clone(), cx), |row, card| {
                        row.child(div().pt_2().pb_2().child(card))
                    })
                    .when(!documents[ix].files(content).is_empty(), |row| {
                        row.child(files::message::render(
                            documents[ix].shared_files(content),
                            &session_id,
                            content_width,
                            *role == Role::User,
                            reader_root.clone(),
                            ix,
                            file_previews.clone(),
                            cx,
                        ))
                    })
                    .when_some(
                        metadata
                            .id
                            .as_ref()
                            .and_then(|id| pending.get(id)),
                        |row, message| {
                            let retry_root = delivery_root.clone();
                            let delete_root = delivery_root.clone();
                            let retry_id = message.request_id.clone();
                            let delete_id = message.request_id.clone();
                            row.child(
                                div()
                                    .w(px(content_width))
                                    .mx_auto()
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .gap_2()
                                    .text_size(px(12.))
                                    .text_color(rgb(if message.status == "failed" {
                                        ZORK_UI.palette.danger
                                    } else {
                                        DIM()
                                    }))
                                    .when(message.status == "failed", |row| {
                                        row.child(
                                            zork_ui::controls::icon("icons/attention.svg", 14.)
                                                .text_color(rgb(ZORK_UI.palette.danger)),
                                        )
                                    })
                                    .child(
                                        match message.status.as_str() {
                                            "failed" => delivery_locale.text("delivery_failed"),
                                            "sending" => delivery_locale.text("delivery_sending"),
                                            _ => "",
                                        },
                                    )
                                    .when(message.status == "failed", |row| {
                                        row.child(
                                            zork_ui::controls::button(
                                                format!("retry-queued-{retry_id}"),
                                                delivery_locale.text("delivery_resend").to_string(),
                                                false,
                                                true,
                                            )
                                            .h(px(28.))
                                            .min_h(px(28.))
                                            .on_click(move |_, _, cx| {
                                                    let _ = retry_root.update(cx, |v, cx| {
                                                        v.resend_queued(&retry_id, cx)
                                                    });
                                                })
                                                .automation(
                                                    AutomationRole::Button,
                                                    delivery_locale.text("delivery_resend"),
                                                ),
                                        )
                                        .child(
                                            zork_ui::controls::quiet_button(
                                                format!("delete-queued-{delete_id}"),
                                                delivery_locale.text("delivery_delete").to_string(),
                                                true,
                                                zork_ui::controls::IconButtonSize::Compact,
                                            )
                                            .px(px(12.))
                                            .on_click(move |_, _, cx| {
                                                    let _ = delete_root.update(cx, |v, cx| {
                                                        v.delete_failed_queued(&delete_id, cx)
                                                    });
                                                })
                                                .automation(
                                                    AutomationRole::Button,
                                                    delivery_locale.text("delivery_delete"),
                                                ),
                                        )
                                    }),
                            )
                        },
                    )
                    .map(|row| {
                        // A new message fades in and rises into place; with reduced
                        // motion it only fades briefly. The clock is the arrival
                        // time, so a row re-mounted by virtualization continues
                        // instead of restarting.
                        use zork_ui::motion::{self, Mode};
                        let (ms, offset) = match motion::mode(cx) {
                            Mode::Full => (motion::BASE, motion::ROW_OFFSET),
                            Mode::Short => (motion::REDUCED_FADE, 0.),
                            Mode::Static => (0, 0.),
                        };
                        let total = motion::duration(ms);
                        if let Some(started) = metadata
                            .id
                            .as_ref()
                            .and_then(|id| arrivals.get(id))
                            .copied()
                            .filter(|time| time.elapsed() < total)
                        {
                            let t = (started.elapsed().as_secs_f32()
                                / total.as_secs_f32().max(0.001))
                            .min(1.);
                            let t = motion::bezier(0.2, 0.7, 0.2, 1.0)(t);
                            window.request_animation_frame();
                            row.relative()
                                .top(px(offset * (1. - t)))
                                .opacity(t)
                                .into_any_element()
                        } else {
                            row.into_any_element()
                        }
                    })
            } else if let Some(activity) = &activity_row {
                // With no message yet, the row keeps the first message's
                // clearance from the floating controls.
                div()
                    .when(ix == 0, |row| row.pt(px(32.)))
                    .child(activity(window, cx))
                    .into_any_element()
            } else {
                div().h_0().into_any()
            }
        });

        let history = if item_count == 0 && activity.is_none() {
            zork_ui::components::message_placeholder::render(
                zork_ui::components::message_placeholder::Data {
                    loading: self.messages_loading,
                    message: self
                        .locale
                        .text(if self.messages_loading {
                            "loading_messages"
                        } else if !self.agent_online && self.connection_error.is_some() {
                            "device_no_cached_messages"
                        } else if self.connection_error.is_some() {
                            "task_unavailable"
                        } else if self.messages_failed {
                            "messages_load_failed"
                        } else {
                            "waiting_first_update"
                        })
                        .into(),
                    older: self.has_older.then(|| {
                        (
                            self.locale.text("load_earlier").to_owned(),
                            !self.loading_older,
                        )
                    }),
                },
                cx,
                |view, cx| view.load_older(cx),
            )
        } else {
            div()
                .flex_1()
                .min_h_0()
                .pt(px(14.))
                .flex()
                .flex_col()
                .child(list.flex_1().min_h_0().pb(px(bottom_inset)))
                .into_any_element()
        };

        div()
            .id("conversation-transcript")
            .size_full()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(history)
            .automation(
                AutomationRole::ScrollArea,
                self.locale.text("conversation_messages"),
            )
    }

    fn render_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.render_composer_frame(window, cx)
    }

    fn render_composer_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let root = cx.entity().downgrade();
        let composer = self.render_shared_composer(window, cx);
        let frame = div()
            .relative()
            .flex_shrink_0()
            .flex()
            .pb(px(ZORK_UI.layout.composer_bottom_inset));
        let frame = frame.justify_center().px_6();
        // Only the surface paints/occludes. The full-width positioning frame is
        // transparent, while list padding lets the last message scroll above it.
        frame
            .flex_col()
            .items_center()
            .when(self.message_motion.unread > 0, |frame| {
                frame.child(
                    crate::desktop::ui::quiet_button(
                        "messages-new",
                        "",
                        true,
                        crate::desktop::ui::IconButtonSize::Standard,
                    )
                    .mb_2()
                    .px_3()
                    .py_1()
                    .h_auto()
                    .text_size(px(12.))
                    .child(self.locale.text("messages_new"))
                    .on_click(cx.listener(|v, _, _, cx| v.animate_message_tail(None, cx)))
                    .automation(AutomationRole::Button, self.locale.text("messages_new")),
                )
            })
            .group(composer_surface::COMPOSER_DROP_GROUP)
            .on_drop(cx.listener(|v, paths: &gpui::ExternalPaths, _, cx| {
                v.attach_paths(paths.paths().to_vec(), cx);
            }))
            .child(self.render_composer_extras(window, cx))
            .child(composer)
            .child(
                gpui::canvas(
                    move |bounds, _, cx| {
                        // The draft file row is inside the surface, so the
                        // measured frame is the whole overlay height.
                        let height = bounds.size.height.as_f32();
                        let root = root.clone();
                        cx.defer(move |cx| {
                            let _ = root.update(cx, |view, cx| {
                                if (view.composer_overlay_height - height).abs() < 0.5 {
                                    return;
                                }
                                let following = view.transcript_list.is_following_tail();
                                view.composer_overlay_height = height;
                                if following {
                                    view.transcript_list.scroll_to_end();
                                }
                                zork_ui::components::region::invalidate(cx, &["transcript"]);
                            });
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
    }

    /// Measure the current conversation editor with its actual font and available width.
    fn measure_composer_geometry(&mut self, window: &Window, cx: &Context<Self>) {
        let rail = if self.preview_overlay {
            self.device_navigation
                .as_ref()
                .map(|nav| nav.read(cx).width(window.viewport_size().width.as_f32()))
                .unwrap_or(0.)
        } else if !self.shell.rail_open {
            8.
        } else if let Some(navigation) = &self.device_navigation {
            navigation
                .read(cx)
                .width(window.viewport_size().width.as_f32())
        } else {
            0.
        };
        let available = f32::from(window.viewport_size().width) - rail;
        // An overlay may cover the page, but its workspace still occupies width.
        // Use the same condition as the rendered split, not page visibility.
        let panel_present = self.has_page_workspace(cx);
        let panel_width = if panel_present {
            self.browser.read(cx).panel_width()
        } else {
            0.
        };
        let width = available - panel_width;

        self.composer_surface_width = {
            let viewport = f32::from(window.viewport_size().width);
            let column = if viewport >= 1550. { 1040. } else { 1000. };
            let gutter = if viewport <= 1120. { 22. } else { 26. };
            // The positioning frame has 24 px padding on each side. Do not
            // measure a wider composer and then let flex layout shrink it:
            // left-based aperture coordinates and right-based file placement
            // would otherwise disagree by four pixels in compact windows.
            let target_width = if panel_present {
                self.browser.read(cx).target_width()
            } else {
                0.
            };
            let column_width = self.composer_centering.sample(
                available,
                panel_width,
                target_width,
                column,
                cx.background_executor().now(),
                cx.reduce_motion()
                    || !self
                        .browser
                        .read(cx)
                        .is_animating(cx.background_executor().now()),
            );
            (column_width - 2. * gutter).min(width - 48.).max(1.)
        };
    }
    fn measure_composer(&mut self, _window: &Window, cx: &Context<Self>) {
        self.composer_editor_height = self
            .composer_input
            .read(cx)
            .content_height()
            .unwrap_or(24.)
            .clamp(24., ZORK_UI.composer.thread_max_editor_height);
    }

    fn render_composer_extras(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Div {
        let feedback = if self.preparing_files > 0 {
            Some(self.locale.text("preparing_files").to_owned())
        } else if self.queued_count > 0 {
            Some(format!(
                "{} {}",
                self.locale.text("pending_messages"),
                self.queued_count
            ))
        } else if self.canceling {
            Some(self.locale.text("stopping").to_owned())
        } else if self.stop_pending {
            Some(self.locale.text("device_stop_unconfirmed").to_owned())
        } else {
            self.error.as_ref().and_then(|error| {
                if ["sse:", "stream:", "load messages:"]
                    .iter()
                    .any(|prefix| error.starts_with(prefix))
                {
                    self.agent_online
                        .then(|| self.locale.text("device_sync_retry").to_owned())
                } else {
                    Some(error.clone())
                }
            })
        };
        div()
            .w(px(self.composer_surface_width))
            .max_w_full()
            .flex()
            .flex_col()
            .children(self.render_hint())
            .map(|frame| {
                frame.when_some(feedback, |frame, text| {
                    frame.child(
                        div()
                            .pb_2()
                            .text_size(px(12.))
                            .text_color(rgb(DIM()))
                            .child(text),
                    )
                })
            })
    }

    // Side rails fold before the 393px chat minimum.

    fn activity_presentations(&self) -> Vec<crate::components::activity::Presentation> {
        self.participants
            .iter()
            .filter_map(|p| {
                let status = p
                    .activity
                    .as_ref()
                    .filter(|s| should_render_live_activity(s, self.lines.last()))?;
                Some(crate::components::activity::Presentation {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    label: agent_status_label(status, self.locale),
                    failed: matches!(status, AgentStatus::Failed { .. }),
                    running: matches!(
                        status,
                        AgentStatus::Live { .. }
                            | AgentStatus::Thinking
                            | AgentStatus::ToolsStarted { .. }
                            | AgentStatus::ToolsWaiting { .. }
                    ),
                })
            })
            .collect()
    }
}

// ------------------------------------------------------------- line render

#[allow(clippy::too_many_arguments)]
fn render_line(
    index: usize,
    line: &TranscriptLine,
    cached: Arc<crate::components::message::MessageRenderDocument>,
    content_width: f32,
    selection: Option<&crate::components::selection::SelectionContext>,
    window: &mut Window,
    device: Option<String>,
    decorations: zork_ui::components::message_row::Decorations,
) -> gpui::AnyElement {
    let TranscriptLine::Message {
        role,
        content,
        metadata,
    } = line;
    let document = cached.document(role, content);
    if *role == Role::User && document.plain_text().is_empty() && !cached.files(content).is_empty()
    {
        return div().into_any_element();
    }
    zork_ui::components::message_row::Row {
        index,
        user: *role == Role::User,
        document,
        content_width,
        selection,
        author_name: metadata.author_name.clone(),
        device,
        model: metadata.model.clone(),
        time: metadata.created_at.as_ref().map(|time| {
            chrono::DateTime::parse_from_rfc3339(time)
                .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string())
                .unwrap_or_else(|_| time.clone())
        }),
    }
    .render_with(window, decorations)
}

fn agent_status_label(status: &AgentStatus, locale: Locale) -> String {
    match status {
        AgentStatus::Live { presentation } => presentation.label(if locale == Locale::ZhCn {
            "zh-CN"
        } else {
            "en"
        }),
        AgentStatus::Clear => locale.text("status_wait").to_owned(),
        AgentStatus::Thinking => locale.text("status_thinking").to_owned(),
        AgentStatus::ToolsStarted {
            calls,
            thinking: true,
        } if !calls.is_empty() => {
            let label = locale.text("status_thinking");
            if locale == Locale::ZhCn {
                format!("{label} · {} 项操作执行中", calls.len())
            } else {
                format!("{label} · {} operations running", calls.len())
            }
        }
        AgentStatus::ToolsStarted { calls, .. } | AgentStatus::ToolsWaiting { calls, .. } => {
            if calls.is_empty() {
                return locale.text("status_tools").to_owned();
            }
            calls
                .iter()
                .map(|call| {
                    call.activity_label(if locale == Locale::ZhCn {
                        "zh-CN"
                    } else {
                        "en"
                    })
                })
                .collect::<Vec<_>>()
                .join(" · ")
        }
        AgentStatus::ToolFinished { .. } => locale.text("status_thinking").to_owned(),
        AgentStatus::Waiting { reason, .. } => format!("{} · {reason}", locale.text("status_wait")),
        AgentStatus::Failed { reason } => format!("{} · {reason}", locale.text("status_failed")),
        AgentStatus::Finished => locale.text("status_finished").to_owned(),
        AgentStatus::Interrupted => locale.text("status_interrupted").to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn conversation_tabs_restore_history_without_leaking_to_another_chat(
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
            view.selected_session = Some("chat-a".into());
            view.restore_chat_history(cx);
            view.open_history("chat-a", cx);
            view.history.detail = Some("event-a".into());
            view.save_chat_history();
            view.selected_session = Some("chat-b".into());
            view.restore_chat_history(cx);
            assert!(!view.history.open);
            assert!(view.history.detail.is_none());
            assert!(!view.browser.read(cx).is_open());
            view.save_chat_history();
            view.selected_session = Some("chat-a".into());
            view.restore_chat_history(cx);
            assert!(view.history.open);
            assert_eq!(view.history.detail.as_deref(), Some("event-a"));
            assert!(view.browser.read(cx).is_native_page_active("history"));
            view.reset_session(cx);
            assert!(!view.history.open);
            assert!(!view.browser.read(cx).is_open());
        });
    }

    #[gpui::test]
    fn selecting_current_task_keeps_inflight_message_request(cx: &mut gpui::TestAppContext) {
        let view = cx.new(|cx| {
            RootView::new(
                Arc::new(StationClient::new("http://127.0.0.1:9", None)),
                None,
                cx,
            )
        });
        view.update(cx, |view, cx| {
            view.selected_session = Some("current".into());
            view.messages_request = 7;
            view.messages_loading = true;
            view.load_session("current", cx);
            assert_eq!(view.messages_request, 7);
            assert!(view.messages_loading);
            view.reset_session(cx);
            assert_eq!(view.messages_request, 8);
            assert!(!view.messages_loading);
            assert!(view.selected_session.is_none());
        });
    }

    #[test]
    fn ignores_agent_internal_transcript_event_names() {
        for name in ["wait", "assistant_delta", "tool"] {
            let decoded = decode_sse_event(&SseEvent {
                name: name.to_owned(),
                data: "{}".to_owned(),
            })
            .expect("unknown internal events are ignored without parsing");
            assert_eq!(decoded, None);
        }
    }

    #[test]
    fn decodes_and_labels_rich_tool_status() {
        let decoded = decode_sse_event(&SseEvent {
                name: "status".to_owned(),
                data: r#"{"state":"tools_started","calls":[{"tool_call_id":"call-1","tool_name":"read_file"}]}"#
                    .to_owned(),
            })
            .expect("valid status payload")
            .expect("known event");

        let DecodedSseEvent::Status(status) = decoded else {
            panic!("expected status event");
        };
        assert_eq!(agent_status_label(&status, Locale::En), "Working");
        assert_eq!(agent_status_label(&status, Locale::ZhCn), "执行操作");
        let status: AgentStatus = serde_json::from_value(serde_json::json!({
                "state":"tools_started", "thinking":true,
                "calls":[{"tool_call_id":"a","tool_name":"custom","labels":{"en":"Inspecting","zh-CN":"检查"},"detail":"report"}]
            })).unwrap();
        assert_eq!(
            agent_status_label(&status, Locale::ZhCn),
            "思考中 · 1 项操作执行中"
        );
    }
}
