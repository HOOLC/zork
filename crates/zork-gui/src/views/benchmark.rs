//! Fixed, disconnected fixtures using the production view and list renderers.
use super::*;
#[path = "benchmark/messages.rs"]
mod messages;

pub const RECORDS: usize = 600;
pub const NOW: i64 = 1_800_000_600_000;

impl RootView {
    pub fn benchmark_composer_material(&self) -> serde_json::Value {
        self.composer_surface.scene.inspect()
    }
    pub fn benchmark_history_modal(
        &self,
        window: &gpui::Window,
        cx: &gpui::App,
    ) -> serde_json::Value {
        let mut value = self.history_details.read(cx).inspect(window, cx);
        value["detail"] = serde_json::json!(self.history.detail);
        value["agent"] = serde_json::json!(self.history.agent_detail);
        value
    }
    pub fn benchmark_page_catalog(
        &mut self,
        pages: zork_client_core::pages::PageCatalog,
        cx: &mut Context<Self>,
    ) {
        self.drive.contents = Arc::new(zork_client_core::pages::content_indices(
            &self.drive.items,
            &pages,
        ));
        self.drive.pages = Arc::new(pages);
        zork_ui::components::region::invalidate_all(cx);
    }
    pub fn benchmark_follow_messages(&mut self, cx: &mut Context<Self>) {
        Self::observe_scroll(&self.transcript_list, cx);
        self.transcript_list.set_follow_mode(FollowMode::Tail);
        self.transcript_list.scroll_to_end();
        zork_ui::components::region::invalidate(cx, &["transcript"]);
    }
    pub fn benchmark_message_motion(&self) -> (bool, usize, bool) {
        (
            self.message_motion.scroll.is_some(),
            self.message_motion.unread,
            self.transcript_list.is_following_tail(),
        )
    }
    pub fn benchmark_transcript_len(&self) -> usize {
        self.lines.len()
    }
    /// Top and bottom of a rendered transcript item in window coordinates;
    /// `lines.len()` is the trailing live activity item.
    pub fn benchmark_transcript_item_bounds(&self, index: usize) -> Option<(f32, f32)> {
        self.transcript_list
            .bounds_for_item(index)
            .map(|bounds| (bounds.top().as_f32(), bounds.bottom().as_f32()))
    }
    pub fn benchmark_file_geometry(&self) -> serde_json::Value {
        serde_json::json!({
            "composer_width":self.composer_surface_width,
            "band":self.draft_files_band(),
            "files":self.draft_state.files.iter().map(|f|serde_json::json!({"id":f.id})).collect::<Vec<_>>()
        })
    }
    /// Draft/message thumbnails that finished decoding.
    pub fn benchmark_file_thumbnails(&self) -> usize {
        self.file_ui.message_previews.borrow().image_count()
    }
    pub fn benchmark_restore_draft(&mut self, cx: &mut Context<Self>) {
        self.restore_draft(cx);
    }
    pub fn benchmark_core_device(&self) -> Arc<zork_client_core::state::Device> {
        self.core_device.clone()
    }
    pub(super) fn benchmark_delivery_snapshot(&mut self, state: &zork_client_core::state::Outbox) {
        if self.benchmark_offline && self.core_conversation.is_none() {
            self.transcript_deliveries = state
                .items
                .iter()
                .map(|message| {
                    (
                        format!("client-{}-{}", message.session_id, message.request_id),
                        zork_client_core::state::DeliveryState {
                            request_id: message.request_id.clone(),
                            attempted: message.attempted,
                            status: state
                                .phases
                                .get(&message.request_id)
                                .cloned()
                                .unwrap_or_default(),
                            error: message.error.clone(),
                        },
                    )
                })
                .collect();
        }
    }
    pub fn benchmark_bind_core(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Arc<zork_client_core::state::Conversation> {
        let conversation = self
            .core_device
            .conversation(self.selected_session.as_deref().unwrap());
        conversation.seed(zork_client_core::state::ConversationData {
            lines: self.lines.clone(),
            deliveries: self.transcript_deliveries.clone(),
            loaded: true,
            ..Default::default()
        });
        let old = self.lines.len();
        self.lines = Transcript::new();
        self.transcript_lookup = Default::default();
        self.transcript_list.splice(0..old, 0);
        self.start_sse(cx);
        self.transcript_list.scroll_to_end();
        conversation
    }
    pub fn benchmark_region_counts(
        &self,
        cx: &gpui::App,
    ) -> std::collections::HashMap<String, [usize; 4]> {
        let mut counts = self.regions.counters(cx);
        if let Some(navigation) = &self.device_navigation {
            counts.extend(
                navigation
                    .read(cx)
                    .benchmark_region_counts(cx)
                    .into_iter()
                    .map(|(key, value)| (format!("navigation/{key}"), value)),
            );
        }
        counts
    }

    pub fn benchmark_panel_width(&self, cx: &gpui::App) -> f32 {
        self.browser.read(cx).panel_width()
    }
    pub fn benchmark_preview_session(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        if !self.sessions.iter().any(|session| session.session_id == id) {
            let mut session = self
                .sessions
                .iter()
                .find(|session| session.session_id == "render-fixture")
                .cloned()
                .expect("render fixture session");
            session.session_id = id.to_owned();
            session.title = Some(id.to_owned());
            Arc::make_mut(&mut self.sessions).push(session);
        }
        self.preview_session(id, false, cx)
    }
    pub fn benchmark_restore_preview(&mut self, cx: &mut Context<Self>) {
        self.restore_preview(cx);
    }
    pub fn render_benchmark_fixture(
        history: bool,
        store: Arc<crate::desktop::store::ClientStore>,
        cx: &mut Context<Self>,
    ) -> Self {
        let node = crate::desktop::store::SavedNode { machine_name: None,
            id: "mini1".into(),
            name: "mini1".into(),
            url: "http://127.0.0.1:9".into(),
            token: None,
            local: false,
            mesh: None,
            group: None,
        };
        let navigation = cx.new(|cx| {
            crate::desktop::navigation::DeviceNavigation::new(store.clone(), &[node], cx)
        });
        let mut view = Self::new_desktop(
            Arc::new(StationClient::new("http://127.0.0.1:9", None)),
            store,
            "mini1".into(),
            cx,
        );
        view.benchmark_offline = true;
        // Fixtures own delivery states; never run a network delivery worker.
        view.delivery_task = None;
        view.locale = std::env::var("ZORK_GUI_LOCALE")
            .ok()
            .as_deref()
            .and_then(Locale::parse)
            .unwrap_or(Locale::ZhCn);
        view.focus_initialized = true;
        view.selected_session = Some("render-fixture".into());
        view.shell
            .navigate(ShellRoute::Task("render-fixture".into()));
        view.agent_online = true;
        view.scroll_active = true;
        Arc::make_mut(&mut view.sessions).push(SessionSummary {
            title: Some("Render fixture".into()),
            kind: "leader".into(),
            session_id: "render-fixture".into(),
            can_send: Some(true),
            profile_id: "fixture".into(),
            model: "fixture-model".into(),
            thinking: "off".into(),
            workspace: "/fixture/workspace".into(),
            status: SessionStatus::Wait,
            runtime_available: true,
            task: None,
        });
        Arc::make_mut(&mut view.node_agents).push(
            serde_json::json!({"id":"leader","name":"产品 Leader",
            "role":"leader","avatar":"fox","session_id":"render-fixture",
            "profile_id":"fixture","model":"fixture-model","thinking":"off"}),
        );
        view.active_leader = Some("leader".into());
        // Channel membership is supplied by core now; the production UI no
        // longer invents a participant from the selected execution session.
        // Keep the same named participant used by the earlier fixture.
        view.participants = vec![crate::api::ParticipantStatus {
            id: "leader".into(),
            name: "产品 Leader".into(),
            avatar: Some("fox".into()),
            session_id: "render-fixture".into(),
            subscribed: true,
            assigned: false,
            activity: None,
        }];
        view.attach_navigation(navigation.clone(), "mini1".into());
        let (selection, locale) = view.navigation_selection();
        let mut directory = zork_client_core::state::DeviceData::default();
        directory.online = Some(true);
        directory.sessions = view.sessions.clone();
        directory.agents = view.node_agents.clone();
        directory.tasks = view.tasks_by_leader.clone();
        let data = zork_client_core::state::NavigationData::project(&directory, Default::default());
        navigation.update(cx, |nav, cx| {
            nav.activate("mini1", cx);
            nav.set_preview("mini1", data, selection, locale, cx);
        });
        if history {
            let records = (0..RECORDS)
                .map(|i| crate::session_history::Record {
                    event_id: format!("{:016x}", i + 1),
                    event: serde_json::json!({"kind":"input_appended","input":{
                        "content":format!("记录 {i:04}：检查项目配置并运行测试"),
                        "received_at_ms":NOW - 600_000 + i as i64 * 500,
                    }}),
                    metadata: Default::default(),
                })
                .collect();
            view.history = history::HistoryState::benchmark(records, NOW);
            let host = view.browser_host();
            view.browser
                .update(cx, |panel, cx| panel.set_host(host, cx));
            view.open_history_tab(cx);
        } else {
            let all_types = std::env::var_os("ZORK_SCROLL_ALL_MESSAGES").is_some();
            let count = std::env::var("ZORK_BENCH_MESSAGE_COUNT")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(if all_types { 100_000 } else { RECORDS })
                .clamp(1, 1_000_000);
            let markdown_stress = std::env::var_os("ZORK_SCROLL_MARKDOWN_STRESS").is_some();
            view.lines = (0..count).map(|i| if all_types { messages::line(i) } else { TranscriptLine::Message {
                role: if !markdown_stress && i % 2 == 0 { Role::User } else { Role::Assistant },
                content: if markdown_stress {
                    include_str!("../../tests/fixtures/markdown-scroll.md").replace("{{index}}", &i.to_string())
                } else { format!("消息 {i:04}：检查项目配置并运行测试\n\n**验证结果**：文件读取正常。\n\n```rust\nlet result = check_config();\n```") },
                metadata: Default::default(),
            }}).collect();
            if all_types {
                view.benchmark_seed_artifacts(count);
                let (store, node) = view.local_cache.as_ref().unwrap().clone();
                for (offset, state) in ["pending", "sending", "failed"].iter().enumerate() {
                    if count < 3 {
                        break;
                    }
                    let index = count - 3 + offset;
                    let TranscriptLine::Message {
                        role,
                        content,
                        metadata,
                    } = &mut view.lines[index];
                    *role = Role::User;
                    let request_id = format!("stress-{state}");
                    *metadata = crate::api::MessageMetadata {
                        id: Some(format!("client-render-fixture-{request_id}")),
                        ..Default::default()
                    };
                    store
                        .enqueue_and_clear_draft(
                            &node,
                            &crate::desktop::store::QueuedMessage {
                                request_id,
                                session_id: "render-fixture".into(),
                                content: content.clone(),
                                attempted: *state != "pending",
                                sent_at_ms: if *state == "pending" { 0 } else { 1 },
                                error: (*state == "failed").then(|| "测试投递失败".into()),
                            },
                        )
                        .expect("benchmark outbox");
                }
                view.refresh_queued();
            }
            view.transcript_list = ListState::new(count + 1, ListAlignment::Top, px(500.));
            view.transcript_list.scroll_to(gpui::ListOffset {
                item_ix: std::env::var("ZORK_BENCH_ANCHOR")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(100)
                    .min(count - 1),
                offset_in_item: px(0.),
            });
        }
        // Exercise the same read-only outbox subscription during retry checks;
        // benchmark_offline prevents starting any network delivery worker.
        view.start_delivery(cx);
        view
    }

    /// Replace only the transcript for fixed-input interaction regressions.
    /// Device navigation, desktop mode and disconnected fixture setup stay the
    /// same as the scrolling benchmark's production desktop fixture.
    pub fn benchmark_replace_messages(
        &mut self,
        messages: Vec<TranscriptLine>,
        cx: &mut Context<Self>,
    ) {
        self.lines = messages.into();
        self.transcript_lookup = Default::default();
        self.transcript_deliveries = Default::default();
        self.transcript_render_cache = Default::default();
        self.transcript_list = ListState::new(self.lines.len() + 1, ListAlignment::Top, px(500.));
        self.transcript_selection.borrow_mut().clear();
        self.comment_popover = None;
        zork_ui::components::region::invalidate_all(cx);
    }

    pub fn benchmark_story_placeholder(&mut self, placeholder: String, cx: &mut Context<Self>) {
        self.composer_input
            .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
    }

    /// Zork Design fixture for the Session activity band: a short Chat with
    /// the member "Studio" working in its own Session. `step` advances the
    /// running tool so live stories can show a step change.
    pub fn benchmark_activity_story(&mut self, state: &str, cx: &mut Context<Self>) {
        let line = |user: bool, text: &str| TranscriptLine::Message {
            role: if user { Role::User } else { Role::Assistant },
            content: text.into(),
            metadata: crate::api::MessageMetadata {
                author_name: (!user).then(|| "Studio".into()),
                ..Default::default()
            },
        };
        self.benchmark_replace_messages(
            vec![
                line(true, "先把会话动态按定稿重做：放到消息列表最后，跟着消息滚动。"),
                line(false, "好的。我先读一下现在的实现和定稿，再改布局和动效。\n\n- 收起时一行：谁、在做什么\n- 展开时最近三步和完整历史"),
                line(true, "改完跑一下相关的测试。"),
            ],
            cx,
        );
        self.benchmark_follow_messages(cx);
        let session = "studio-session";
        let history = self.core_device.conversation(session).history();
        history.seed(zork_client_core::state::HistoryData {
            loaded: true,
            ..Default::default()
        });
        self.participants = vec![crate::api::ParticipantStatus {
            id: "studio".into(),
            name: "Studio".into(),
            avatar: None,
            session_id: session.into(),
            subscribed: true,
            assigned: false,
            activity: Some(AgentStatus::Thinking),
        }];
        self.sync_session_activity(cx);
        if state != "waiting" {
            history.seed(zork_client_core::state::HistoryData {
                records: Self::benchmark_activity_records(0).into(),
                loaded: true,
                ..Default::default()
            });
            self.deliver_session_activity_updates(cx);
        }
        if let Some(preview) = self.session_activity_preview.as_mut() {
            preview.expanded = state == "expanded";
            if state == "finished" {
                preview.stopped = true;
                for row in &mut preview.rows {
                    row.running = false;
                }
            }
        }
        if state == "disconnected" {
            self.agent_online = false;
        }
        zork_ui::components::region::invalidate_all(cx);
    }

    /// Replays the live fixture's next step.
    pub fn benchmark_activity_step(&mut self, step: usize, cx: &mut Context<Self>) {
        self.core_device
            .conversation("studio-session")
            .history()
            .seed(zork_client_core::state::HistoryData {
                records: Self::benchmark_activity_records(step).into(),
                loaded: true,
                ..Default::default()
            });
        self.deliver_session_activity_updates(cx);
    }

    /// Ends the live fixture's round: it holds briefly, then leaves.
    pub fn benchmark_activity_finish(&mut self, cx: &mut Context<Self>) {
        for participant in &mut self.participants {
            participant.activity = None;
        }
        self.sync_session_activity(cx);
    }

    fn benchmark_activity_records(step: usize) -> Vec<crate::session_history::Record> {
        let t = NOW - 60_000;
        let tools = [
            ("read-1", "file.read", serde_json::json!({"path":"src/chat.rs"}), 400),
            ("shell-1", "shell.run", serde_json::json!({"command":"cargo check -p zork-ui"}), 12_000),
            ("write-1", "file.write", serde_json::json!({"path":"src/activity.rs"}), 2_000),
            ("edit-1", "file.edit", serde_json::json!({"path":"crates/zork-ui/src/device_name.rs"}), 1_000),
            ("shell-2", "shell.run", serde_json::json!({"command":"cargo test -p zork-gui --test headless_chat_activity"}), 9_000),
        ];
        let last = (2 + step) % tools.len().max(1);
        let last = last.max(2);
        let mut records = Vec::new();
        let mut at = t;
        for (index, (id, tool, arguments, took)) in tools.iter().take(last + 1).enumerate() {
            records.push(crate::session_history::Record {
                event_id: format!("{:016x}", index * 2 + 1),
                event: serde_json::json!({"kind":"step_completed","step_id":format!("s{index}"),
                    "completed_at_ms":at,"invocations":[{"invocation_id":id,"tool":tool,
                    "started_at_ms":at,"arguments":arguments}]}),
                metadata: Default::default(),
            });
            if index < last {
                at += took;
                records.push(crate::session_history::Record {
                    event_id: format!("{:016x}", index * 2 + 2),
                    event: serde_json::json!({"kind":"tool_result","result":{"invocation_id":id,
                        "tool":tool,"outcome":"succeeded","finished_at_ms":at,"data":{}}}),
                    metadata: Default::default(),
                });
                at += 300;
            }
        }
        records
    }

    pub fn benchmark_story_history(
        &mut self,
        records: Vec<crate::session_history::Record>,
        now: i64,
        cx: &mut Context<Self>,
    ) {
        self.history = history::HistoryState::story(records, now);
        self.refresh_history_sources();
        let host = self.browser_host();
        self.browser
            .update(cx, |panel, cx| panel.set_host(host, cx));
        self.open_history_tab(cx);
        zork_ui::components::region::invalidate_all(cx);
    }

    pub fn benchmark_record_count(&self, history: bool) -> usize {
        if history {
            RECORDS
        } else {
            self.lines.len()
        }
    }

    pub fn benchmark_jump_to(&self, index: usize, cx: &mut Context<Self>) {
        self.transcript_list.scroll_to(gpui::ListOffset {
            item_ix: index.min(self.lines.len().saturating_sub(1)),
            offset_in_item: px(0.),
        });
        zork_ui::components::region::invalidate_all(cx);
    }

    pub fn benchmark_scroll_to_end(&self, cx: &mut Context<Self>) {
        self.transcript_list.scroll_to_end();
        zork_ui::components::region::invalidate_all(cx);
    }

    pub fn benchmark_message_coverage(&self) -> serde_json::Value {
        let mut coverage = messages::coverage(self.lines.len());
        coverage["measured_kinds"] = serde_json::json!(messages::kinds()
            .iter()
            .enumerate()
            .filter(|(index, _)| self.benchmark_message_kinds.get()
                & ((1 << index) | (1 << (index + messages::kinds().len())))
                != 0)
            .map(|(_, kind)| *kind)
            .collect::<Vec<_>>());
        coverage["both_roles_measured"] = (0..messages::kinds().len())
            .all(|index| {
                let mask = (1 << index) | (1 << (index + messages::kinds().len()));
                self.benchmark_message_kinds.get() & mask == mask
            })
            .into();
        coverage["file_and_image_artifacts"] = self.benchmark_artifact_count().into();
        coverage["delivery_states"] = serde_json::json!(["pending", "sending", "failed"]);
        coverage["parsed_documents"] = self.transcript_render_cache.parsed_document_count().into();
        coverage
    }

    pub fn benchmark_kind_count() -> usize {
        messages::kinds().len()
    }

    pub fn benchmark_restore_delivery_failure(&mut self, cx: &mut Context<Self>) {
        let (store, node) = self.local_cache.as_ref().unwrap();
        store
            .fail_delivery(node, "stress-failed", "测试投递失败")
            .unwrap();
        self.refresh_queued();
        zork_ui::components::region::invalidate_all(cx);
    }

    pub fn benchmark_activity(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        let calls = vec![crate::api::PublicToolCall {
            tool_call_id: "benchmark-tool".into(),
            tool_name: "read_file".into(),
            detail: "检查配置".into(),
            action: "读取配置".into(),
            labels: [
                ("zh-CN".into(), "读取".into()),
                ("en".into(), "Reading".into()),
            ]
            .into(),
        }];
        let status = match index {
            0 => AgentStatus::Clear,
            1 => AgentStatus::Thinking,
            2 => AgentStatus::ToolsStarted {
                calls,
                thinking: false,
            },
            3 => AgentStatus::ToolFinished {
                tool_call_id: "benchmark-tool".into(),
            },
            4 => AgentStatus::ToolsWaiting {
                calls,
                deadline_ms: NOW + 60000,
            },
            5 => AgentStatus::Waiting {
                reason: "等待任务完成".into(),
                deadline_ms: NOW + 60000,
            },
            6 => AgentStatus::Failed {
                reason: "测试失败状态".into(),
            },
            7 => AgentStatus::Finished,
            _ => AgentStatus::Interrupted,
        };
        zork_client_core::conversation::apply_status(
            &mut self.activity,
            &mut self.participants,
            &mut self.stop_pending,
            self.selected_session.as_deref(),
            status,
        );
        self.activity_revision = self.activity_revision.wrapping_add(1);
        self.transcript_list
            .remeasure_items(self.lines.len()..self.lines.len() + 1);
        self.transcript_list.scroll_to(gpui::ListOffset {
            item_ix: self.lines.len(),
            offset_in_item: px(0.),
        });
        zork_ui::components::region::invalidate_all(cx);
        !self.activity_presentations().is_empty()
    }

    pub fn benchmark_begin_frame(&self) {
        self.benchmark_rows.set(0);
        self.benchmark_artifact_cards.set(0);
        self.benchmark_artifact_indices.borrow_mut().clear();
    }

    pub fn benchmark_frame_state(&self, history: bool) -> (usize, usize, f32, usize) {
        let offset = if history {
            self.history.benchmark_offset()
        } else {
            self.transcript_list.logical_scroll_top()
        };
        (
            self.benchmark_rows.get(),
            offset.item_ix,
            offset.offset_in_item.as_f32(),
            self.benchmark_artifact_cards.get(),
        )
    }
}

pub(super) fn message_kind_mask(index: usize) -> u128 {
    1 << (index % messages::kinds().len()
        + if (index / messages::kinds().len()) % 3 == 0 {
            0
        } else {
            messages::kinds().len()
        })
}

pub(super) fn artifact_is_image(index: usize) -> Option<bool> {
    match messages::kinds()[index % messages::kinds().len()] {
        "image_reference" => Some(true),
        "file_link" => Some(false),
        _ => None,
    }
}
