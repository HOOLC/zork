//! Multi-agent transcript presentation (docs/design/interface.md 多 Agent 消息).
//!
//! Core's `message_presentation::present` runs once per transcript change and
//! again when a time label changes (`next_change_ms`); rows draw from its
//! result. This module keeps only what the UI owns: the "within one laid-out
//! screen" half of the omission rule, reply jumps with the fading wash,
//! load-then-jump for not-loaded originals, and the short hint.
use super::*;
use std::{cell::RefCell, time::Instant};
use zork_ui::components::message::{MarkKind, TextMarks};
use zork_ui::components::message_row::{
    presentation::{self, RowHeights},
    CommentPairView, CommentsView, Decorations, ReplyText,
};

const WASH_HOLD: Duration = Duration::from_millis(1600);
const WASH_FADE: Duration = Duration::from_millis(600);
const HINT_TIME: Duration = Duration::from_millis(2400);
/// Below this column width the device name moves into the hover detail.
pub(super) const PHONE_WIDTH: f32 = 480.;

pub(super) struct JumpWash {
    pub id: String,
    pub passage: Option<String>,
    pub started: Instant,
}

/// Omission decisions stay fixed for one column width, screen height and
/// transcript, so a line never appears or disappears while scrolling.
#[derive(Default)]
pub(super) struct Omissions {
    width: f32,
    screen: f32,
    decisions: HashMap<usize, bool>,
}

#[derive(Default)]
pub(super) struct MultiAgentState {
    pub presented: Rc<presentation::Transcript>,
    source: Option<(Transcript, bool, Locale)>,
    due: bool,
    refresh: Option<Task<()>>,
    pub heights: Rc<RefCell<RowHeights>>,
    pub omissions: Rc<RefCell<Omissions>>,
    pub wash: Option<JumpWash>,
    wash_task: Option<Task<()>>,
    /// The not-loaded reply whose click is loading older history.
    pub reply_loading: Option<String>,
    pub hint: Option<String>,
    hint_task: Option<Task<()>>,
}

pub(super) fn reply_text(locale: Locale) -> ReplyText {
    ReplyText {
        reply: locale.text("reply_to").into(),
        earlier: locale.text("reply_earlier").into(),
        not_loaded: locale.text("reply_not_loaded").into(),
        loading: locale.text("reply_loading").into(),
        deleted: locale.text("reply_deleted").into(),
        summary: locale.text("reply_summary").into(),
    }
}

impl RootView {
    fn presentation_options(&self) -> zork_client_core::message_presentation::PresentOptions {
        use zork_client_core::message_presentation::{DeviceLabel, PresentOptions};
        let mut devices: HashMap<String, DeviceLabel> = self
            .mesh_status
            .peers
            .iter()
            .map(|peer| {
                (
                    peer.origin.clone(),
                    DeviceLabel {
                        display: peer.name.clone(),
                        machine: None,
                    },
                )
            })
            .collect();
        let snapshot = self.core_device.snapshot();
        let local_origin = snapshot.info["sync"]["owner"]
            .as_str()
            .or(self.mesh_status.origin.as_deref());
        if let (Some(origin), Some(name)) = (local_origin, &self.device_name) {
            devices.insert(
                origin.to_owned(),
                DeviceLabel {
                    display: name.display.clone(),
                    machine: None,
                },
            );
        }
        // Origins without a Mesh name read as themselves unless they look like
        // opaque ids (the transcript's existing device-label rule).
        for TranscriptLine::Message { metadata, .. } in self.lines.iter() {
            if let Some(origin) = metadata.device.as_deref() {
                if !devices.contains_key(origin)
                    && !zork_client_core::device_label::is_id_like(origin)
                {
                    devices.insert(
                        origin.to_owned(),
                        DeviceLabel {
                            display: origin.to_owned(),
                            machine: None,
                        },
                    );
                }
            }
        }
        let agent_models = self
            .node_agents
            .iter()
            .filter_map(|agent| {
                Some((
                    agent["id"].as_str()?.to_owned(),
                    agent["model"]
                        .as_str()
                        .filter(|m| !m.is_empty())?
                        .to_owned(),
                ))
            })
            .collect();
        PresentOptions {
            now: chrono::Local::now().fixed_offset(),
            locale: match self.locale {
                Locale::ZhCn => zork_client_core::message_time::TimeLocale::ZhCn,
                Locale::En => zork_client_core::message_time::TimeLocale::En,
            },
            has_older: self.has_older,
            devices,
            agent_models,
        }
    }

    /// Core's presentation of the loaded transcript, recomputed when the rows,
    /// the older-history state or the locale change, and when a label is due.
    pub(super) fn transcript_presentation(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Rc<presentation::Transcript> {
        let fresh = self
            .multi_agent
            .source
            .as_ref()
            .is_some_and(|(lines, older, locale)| {
                lines.ptr_eq(&self.lines) && *older == self.has_older && *locale == self.locale
            });
        if fresh && !self.multi_agent.due {
            return self.multi_agent.presented.clone();
        }
        let options = self.presentation_options();
        let presented = zork_client_core::message_presentation::present(
            self.lines
                .iter()
                .map(zork_client_core::message_presentation::PresentRow::from),
            &options,
        );
        let parsed = serde_json::to_value(&presented)
            .map(presentation::parse)
            .unwrap_or_default();
        if !self
            .multi_agent
            .source
            .as_ref()
            .is_some_and(|(lines, _, _)| lines.ptr_eq(&self.lines))
        {
            *self.multi_agent.omissions.borrow_mut() = Omissions::default();
        }
        self.multi_agent.source = Some((self.lines.clone(), self.has_older, self.locale));
        self.multi_agent.due = false;
        self.multi_agent.refresh = parsed.next_change_ms.map(|wait| {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(wait.max(250) as u64 + 50))
                    .await;
                let _ = this.update(cx, |v, cx| {
                    v.multi_agent.due = true;
                    zork_ui::components::region::invalidate(cx, &["transcript"]);
                });
            })
        });
        self.multi_agent.presented = Rc::new(parsed);
        self.multi_agent.presented.clone()
    }

    /// Scrolls so the message at `index` starts ~24 px below the top and marks
    /// the quoted passage (or the whole message) with the warm wash.
    pub(super) fn jump_to_message(
        &mut self,
        index: usize,
        passage: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self
            .lines
            .get(index)
            .and_then(|TranscriptLine::Message { metadata, .. }| metadata.id.clone())
        else {
            return;
        };
        self.animate_scroll_to(index, px(-24.), cx);
        self.multi_agent.wash = Some(JumpWash {
            id,
            passage,
            started: Instant::now(),
        });
        let reduce = cx.reduce_motion();
        self.multi_agent.wash_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(WASH_HOLD).await;
            if !reduce {
                let began = Instant::now();
                while began.elapsed() < WASH_FADE {
                    let alive = this
                        .update(cx, |_, cx| {
                            zork_ui::components::region::invalidate(cx, &["transcript"])
                        })
                        .is_ok();
                    if !alive {
                        return;
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(16))
                        .await;
                }
            }
            let _ = this.update(cx, |v, cx| {
                v.multi_agent.wash = None;
                zork_ui::components::region::invalidate(cx, &["transcript"]);
            });
        }));
        zork_ui::components::region::invalidate(cx, &["transcript"]);
    }

    /// Current wash opacity: full for ~1.6 s, then fading out.
    pub(super) fn wash_alpha(&self, reduce_motion: bool) -> f32 {
        let Some(wash) = &self.multi_agent.wash else {
            return 0.;
        };
        let elapsed = wash.started.elapsed();
        if elapsed < WASH_HOLD {
            return 1.;
        }
        if reduce_motion {
            return 0.;
        }
        (1. - (elapsed - WASH_HOLD).as_secs_f32() / WASH_FADE.as_secs_f32()).clamp(0., 1.)
    }

    /// Animated scroll to `offset` from the top of item `index` (ease-out,
    /// ~200 ms; immediate with reduced motion). A wheel interrupts it.
    fn animate_scroll_to(&mut self, index: usize, offset: gpui::Pixels, cx: &mut Context<Self>) {
        self.message_motion.scroll.take();
        let list = self.transcript_list.clone();
        let start_offset = list.logical_scroll_top();
        let start = -list.scroll_px_offset_for_scrollbar().y.as_f32();
        list.set_follow_mode(FollowMode::Normal);
        list.scroll_to(gpui::ListOffset {
            item_ix: index,
            offset_in_item: px(0.),
        });
        list.scroll_by(offset);
        let target_offset = list.logical_scroll_top();
        if cx.reduce_motion() {
            zork_ui::components::region::invalidate(cx, &["transcript"]);
            return;
        }
        let target = -list.scroll_px_offset_for_scrollbar().y.as_f32();
        list.scroll_to(start_offset);
        self.message_motion.scroll = Some(cx.spawn(async move |this, cx| {
            let began = Instant::now();
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let done = this
                    .update(cx, |v, cx| {
                        let t = (began.elapsed().as_secs_f32() / 0.20).min(1.);
                        let current = -v
                            .transcript_list
                            .scroll_px_offset_for_scrollbar()
                            .y
                            .as_f32();
                        let next = start + (target - start) * (1. - (1. - t).powi(3));
                        v.transcript_list.scroll_by(px(next - current));
                        if t >= 1. {
                            v.transcript_list.scroll_to(target_offset);
                            v.message_motion.scroll = None;
                        }
                        zork_ui::components::region::invalidate(cx, &["transcript"]);
                        t >= 1.
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
        }));
    }

    /// First click on a not-loaded original: load older history only. The
    /// list keeps its reading anchor, so the distance from the bottom stays.
    pub(super) fn load_for_reply(&mut self, reply_id: String, cx: &mut Context<Self>) {
        if self.loading_older {
            return;
        }
        self.multi_agent.reply_loading = Some(reply_id);
        self.load_older(cx);
        zork_ui::components::region::invalidate(cx, &["transcript"]);
    }

    /// Called when older history finished loading.
    pub(super) fn older_loaded(&mut self, cx: &mut Context<Self>) {
        if self.multi_agent.reply_loading.take().is_some() {
            self.show_hint(self.locale.text("reply_loaded_hint").into(), cx);
        }
    }

    /// A short dark confirmation above the composer.
    pub(super) fn show_hint(&mut self, text: String, cx: &mut Context<Self>) {
        self.multi_agent.hint = Some(text);
        self.multi_agent.hint_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HINT_TIME).await;
            let _ = this.update(cx, |v, cx| {
                v.multi_agent.hint = None;
                zork_ui::components::region::invalidate(cx, &["composer"]);
            });
        }));
        zork_ui::components::region::invalidate(cx, &["composer"]);
    }

    pub(super) fn render_hint(&self) -> Option<gpui::AnyElement> {
        self.multi_agent.hint.clone().map(|hint| {
            div()
                .w_full()
                .flex()
                .justify_center()
                .pb_2()
                .child(
                    zork_ui::components::message_row::identity::dark_pill(
                        "transcript-hint",
                        hint.clone(),
                    )
                    .automation(AutomationRole::Status, hint),
                )
                .into_any_element()
        })
    }
}

/// Everything a transcript row needs besides its document, captured once per
/// frame by the list's render closure.
#[derive(Clone)]
pub(super) struct RowContext {
    pub presented: Rc<presentation::Transcript>,
    pub heights: Rc<RefCell<RowHeights>>,
    pub omissions: Rc<RefCell<Omissions>>,
    pub content_width: f32,
    pub screen: f32,
    pub show_device: bool,
    pub reply_text: ReplyText,
    pub reply_loading: Option<String>,
    pub wash: Option<(String, Option<String>, f32)>,
    /// Draft passages by source message id.
    pub drafts: Rc<HashMap<String, Vec<String>>>,
    pub quote_label: String,
    pub root: gpui::WeakEntity<RootView>,
}

impl RowContext {
    /// Laid-out height of row `index`, or its estimate when never rendered.
    fn height(
        &self,
        lines: &Transcript,
        documents: &zork_client_core::observe::List<
            crate::components::message::MessageRenderDocument,
        >,
        index: usize,
    ) -> f32 {
        let row = self.presented.rows.get(index);
        let id = row
            .and_then(|row| row.id.clone())
            .unwrap_or_else(|| format!("row-{index}"));
        if let Some(height) = self.heights.borrow().get(&id, self.content_width) {
            return height;
        }
        let (Some(TranscriptLine::Message { role, content, .. }), Some(document)) =
            (lines.get(index), documents.get(index))
        else {
            return 0.;
        };
        presentation::estimate_height(
            &document.selection_text(role, content),
            self.content_width,
            row.is_some_and(|row| row.group_head),
            *role == Role::User,
        )
    }

    fn omit(
        &self,
        lines: &Transcript,
        documents: &zork_client_core::observe::List<
            crate::components::message::MessageRenderDocument,
        >,
        index: usize,
    ) -> bool {
        {
            let mut omissions = self.omissions.borrow_mut();
            if (omissions.width - self.content_width).abs() > 0.5
                || (omissions.screen - self.screen).abs() > 0.5
            {
                *omissions = Omissions {
                    width: self.content_width,
                    screen: self.screen,
                    decisions: HashMap::new(),
                };
            }
            if let Some(decision) = omissions.decisions.get(&index) {
                return *decision;
            }
        }
        let decision = presentation::omit_row_reply(
            &self.presented.rows,
            index,
            |at| self.height(lines, documents, at),
            self.screen,
        );
        self.omissions
            .borrow_mut()
            .decisions
            .insert(index, decision);
        decision
    }

    /// The row's decorations: core's presentation plus the UI's omission
    /// decision, callbacks, wash and draft marks.
    pub fn decorations(
        &self,
        index: usize,
        lines: &Transcript,
        documents: &zork_client_core::observe::List<
            crate::components::message::MessageRenderDocument,
        >,
        selection_text: gpui::SharedString,
        comment_documents: Option<&crate::components::message::CommentDocuments>,
    ) -> Decorations {
        let Some(row) = self.presented.rows.get(index) else {
            return Decorations::default();
        };
        let loading = row.id.is_some() && row.id == self.reply_loading;
        let omit = row.reply.is_some() && self.omit(lines, documents, index);
        let mut decor = Decorations::from_presentation(
            row,
            self.show_device,
            omit,
            loading,
            self.reply_text.clone(),
        );
        if let (Some(line), Some(reply_id)) = (row.reply.as_ref(), row.id.clone()) {
            let root = self.root.clone();
            let mark = line.mark();
            decor.on_reply = match (line.state, line.target_index) {
                (presentation::TargetState::Linked, Some(target)) => Some(Rc::new(move |_, cx| {
                    let mark = mark.clone();
                    let _ = root.update(cx, |v, cx| v.jump_to_message(target, mark, cx));
                })),
                (presentation::TargetState::NotLoaded, _) if !loading => {
                    Some(Rc::new(move |_, cx| {
                        let reply_id = reply_id.clone();
                        let _ = root.update(cx, |v, cx| v.load_for_reply(reply_id, cx));
                    }))
                }
                _ => None,
            };
        }
        let mut marks = Vec::new();
        if let Some((id, passage, alpha)) = self
            .wash
            .as_ref()
            .filter(|(id, _, _)| row.id.as_ref() == Some(id))
        {
            let _ = id;
            match passage
                .as_deref()
                .and_then(|passage| presentation::find_passage(&selection_text, passage))
            {
                Some(range) => marks.push((range, MarkKind::Wash(*alpha))),
                None => decor.wash = *alpha,
            }
        }
        if let Some(passages) = row.id.as_ref().and_then(|id| self.drafts.get(id)) {
            for passage in passages {
                if let Some(range) = presentation::find_passage(&selection_text, passage) {
                    marks.push((range, MarkKind::Draft));
                }
            }
        }
        if !marks.is_empty() {
            decor.marks = Some(TextMarks::new(selection_text.clone(), marks));
        }
        if let Some(source_id) = row.id.clone() {
            let root = self.root.clone();
            let text = selection_text.clone();
            decor.quote_action = Some((
                self.quote_label.clone(),
                Rc::new(move |_, cx| {
                    let quote: String = text
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .chars()
                        .take(60)
                        .collect();
                    let source_id = source_id.clone();
                    let _ = root.update(cx, |v, cx| v.quote_whole_message(&source_id, quote, cx));
                }),
            ));
        }
        if let (Some(view), Some((docs, extra))) = (&row.comments, comment_documents) {
            decor.comments = Some(CommentsView {
                pairs: view
                    .pairs
                    .iter()
                    .zip(docs)
                    .map(|(pair, document)| {
                        let root = self.root.clone();
                        let passage = pair.pair.quote.clone();
                        CommentPairView {
                            author: pair.source.quote_author(),
                            passage: pair.pair.quote.clone(),
                            reply: document.clone(),
                            on_click: pair.source_index.map(|target| {
                                Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                                    let passage = passage.clone();
                                    let _ = root.update(cx, |v, cx| {
                                        v.jump_to_message(target, Some(passage), cx)
                                    });
                                })
                                    as zork_ui::components::message_row::Callback
                            }),
                        }
                    })
                    .collect(),
                extra: extra.clone(),
            });
        }
        decor
    }
}
