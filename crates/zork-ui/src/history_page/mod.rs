//! Complete history page. Native and Playground supply the same read-only
//! snapshots, resolved destinations and typed event adapter.
use crate::components::{
    history::{activity_accent, activity_color, kind_icon, kind_label, ActivityHeader},
    message::{render_document, MessageDocument},
};
use crate::history::{
    self as model,
    activity::{Activity, Kind, Projection, Row},
    Entry,
};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::ZORK_UI,
    resources::Text,
};
use gpui::{prelude::*, *};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    time::Duration,
};
#[allow(non_snake_case)]
fn DIM() -> u32 {
    crate::design::ZORK_UI.palette.muted
}
#[allow(non_snake_case)]
fn TEXT() -> u32 {
    crate::design::ZORK_UI.palette.text
}
#[allow(non_snake_case)]
fn SUBTLE() -> u32 {
    crate::design::ZORK_UI.palette.subtle
}
#[allow(non_snake_case)]
fn BORDER() -> u32 {
    crate::design::ZORK_UI.palette.border
}
/// A received message's quote bar.
#[allow(non_snake_case)]
fn QUOTE() -> u32 {
    crate::design::ZORK_UI.palette.border_strong
}
/// A message sent to a Chat sits on a light persimmon wash.
fn sent_wash() -> Rgba {
    rgba((crate::design::INTERACTION.accent << 8) | 0x14)
}
/// A received or sent message previews three lines until it is opened.
const MESSAGE_LINES: usize = 3;
/// A file revealed in a record's details previews this many lines.
const FILE_LINES: usize = 20;
mod live;
pub use live::HistoryChanged;
mod statistics;
pub use statistics::{Runtime, Statistics};

#[derive(Clone)]
pub enum Jump {
    Conversation(String),
    Agent(String),
    Entry(String),
    File(String),
}
pub enum Action {
    Scroll,
    LoadOlder,
    OpenEntry(String),
    Jump(Jump),
}
#[derive(Clone, Default)]
pub struct Paging {
    pub older: bool,
    pub busy: bool,
    pub loading_older: bool,
    pub loaded: bool,
    pub error: bool,
}

pub struct State {
    pub entries: zork_observe::List<Entry>,
    pub projection: Projection,
    pub rows: Vec<Row>,
    pub selected: Option<String>,
    pub expanded: HashSet<String>,
    /// Model rows whose Markdown body is expanded past its clipped preview.
    pub output_expanded: HashSet<String>,
    pub records_expanded: HashSet<String>,
    pub following_latest: bool,
    pub follow_locked: bool,
    pub positioned: bool,
    pub documents: RefCell<HashMap<String, (String, Rc<MessageDocument>)>>,
    pub previews: RefCell<HashMap<String, Rc<std::cell::Cell<f32>>>>,
    pub loaded_usage: model::usage::UsageSummary,
    pub model_calls: usize,
    pub models: Vec<String>,
    pub rendered_width: f32,
    pub scroll: ListState,
    pub scroll_observed: bool,
    pub fixed_now: Option<i64>,
    pub clock_offset: i64,
}
impl Default for State {
    fn default() -> Self {
        Self {
            entries: Default::default(),
            projection: Default::default(),
            rows: vec![],
            selected: None,
            expanded: Default::default(),
            output_expanded: Default::default(),
            records_expanded: Default::default(),
            following_latest: true,
            follow_locked: false,
            positioned: false,
            documents: Default::default(),
            previews: Default::default(),
            loaded_usage: Default::default(),
            model_calls: 0,
            models: Vec::new(),
            rendered_width: 440.,
            scroll: ListState::new(1, ListAlignment::Top, px(200.))
                .with_uniform_item_height(px(26.)),
            scroll_observed: false,
            fixed_now: None,
            clock_offset: 0,
        }
    }
}
impl State {
    pub fn with_metrics(mut self) -> Self {
        self.update_metrics();
        self
    }
    pub fn update_metrics(&mut self) {
        let metrics = model::usage::LoadedUsage::new(self.entries.iter());
        self.loaded_usage = metrics.usage;
        self.model_calls = metrics.calls;
        self.models = metrics.models;
    }

    pub fn now(&self) -> i64 {
        self.fixed_now
            .unwrap_or_else(|| model::now() + self.clock_offset)
    }
}
impl State {
    pub fn row_entry(&self, row: usize) -> Option<&Entry> {
        let row = self.rows.get(row)?;
        let activity = row
            .activity
            .unwrap_or(self.projection.blocks[row.block].start);
        self.entries.get(self.projection.activities[activity].entry)
    }

    pub fn row_for_id(&self, id: &str) -> Option<usize> {
        let entry = self.entries.iter().position(|e| e.id == id)?;
        let block = self
            .projection
            .entry_to_block
            .get(entry)
            .copied()
            .flatten()?;
        self.rows
            .iter()
            .position(|row| {
                row.block == block
                    && row
                        .activity
                        .is_some_and(|a| self.projection.activities[a].entry == entry)
            })
            .or_else(|| self.rows.iter().position(|row| row.block == block))
    }

    pub fn rebuild_rows(&mut self) {
        let previous = self.rows.len();
        let offset = self.scroll.logical_scroll_top();
        let anchor = offset
            .item_ix
            .checked_sub(1)
            .and_then(|i| self.rows.get(i))
            .copied();
        self.rows = self.projection.rows(self.entries.iter(), &self.expanded);
        self.scroll.splice(1..previous + 1, self.rows.len());
        self.scroll.clone().with_uniform_item_height(px(26.));
        if let Some(index) = anchor.and_then(|anchor| self.rows.iter().position(|r| *r == anchor)) {
            self.scroll.scroll_to(ListOffset {
                item_ix: index + 1,
                ..offset
            });
        } else {
            self.scroll.scroll_to(offset);
        }
    }

    pub fn hold_disclosure(&mut self, _row: usize) {
        self.following_latest = false;
        self.follow_locked = true;
        // A virtual list must retain the first visible row. A negative offset
        // anchored at the clicked row would leave its preceding rows unpainted.
        let offset = self.scroll.logical_scroll_top();
        self.scroll.scroll_to(offset);
    }

    fn document(&self, id: &str, text: &str) -> Rc<MessageDocument> {
        let mut cache = self.documents.borrow_mut();
        if let Some((source, document)) = cache.get(id) {
            if source == text {
                return document.clone();
            }
        }
        if cache.len() >= 128 {
            cache.clear();
        }
        let document = Rc::new(MessageDocument::parse(text));
        cache.insert(id.to_owned(), (text.to_owned(), document.clone()));
        document
    }
}

pub trait Host: Sized + EventEmitter<HistoryChanged> + 'static {
    fn history(&self) -> &State;
    fn history_mut(&mut self) -> &mut State;
    fn history_text(&self) -> Text;
    fn history_subject(&self, activity: &Activity, entry: &Entry)
        -> (Option<String>, Option<Jump>);
    fn history_action(&mut self, action: Action, cx: &mut Context<Self>);
    fn history_paging(&self) -> Paging;
    fn history_statistics(&mut self) -> Statistics;
    fn history_row_built(&self) {}
    fn history_select(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(e) = self.history().entries.get(index) {
            let id = e.id.clone();
            self.history_mut().selected = Some(id.clone());
            self.history_mut().following_latest = false;
            self.history_mut().follow_locked = true;
            self.history_mut().records_expanded.insert(id.clone());
            self.history_mut().output_expanded.insert(id.clone());
            if let Some(block) = self
                .history()
                .projection
                .entry_to_block
                .get(index)
                .copied()
                .flatten()
            {
                let b = &self.history().projection.blocks[block];
                if b.is_group() {
                    let first = self.history().projection.activities[b.start].entry;
                    let first_id = self.history().entries[first].id.clone();
                    self.history_mut().expanded.insert(first_id);
                    self.history_mut().rebuild_rows();
                }
                if let Some(row) = self.history().row_for_id(&id) {
                    self.history().scroll.scroll_to_reveal_item(row + 1);
                }
            }
            crate::components::region::invalidate(cx, &["history", "header"]);
        }
    }
    /// The page state: a centred 11px tertiary line 10px above the
    /// records, holding the loading, failed, paging, start and empty states.
    fn render_history_older(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let paging = self.history_paging();
        let busy = paging.busy && (!paging.loaded || paging.loading_older);
        let label = self.history_text().text(if busy {
            "history_loading"
        } else if paging.error {
            "history_failed"
        } else if self.history().rows.is_empty() {
            "history_empty"
        } else {
            "history_start"
        });
        div()
            .id("history-older")
            .w_full()
            .pb(px(10.))
            .flex()
            .flex_col()
            .items_center()
            .text_center()
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(rgb(SUBTLE()))
            .when(busy || paging.error || !paging.older, |v| {
                v.child(div().when(!busy, |v| v.mt(px(6.))).child(label.clone()))
            })
            .when(!paging.busy && (paging.older || paging.error), |v| {
                v.child(
                    crate::controls::button(
                        "history-load-page",
                        self.history_text().text(if paging.error {
                            "history_retry"
                        } else {
                            "history_older"
                        }),
                        false,
                        true,
                    )
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.history_mut().following_latest = false;
                        v.history_action(Action::LoadOlder, cx);
                        cx.notify();
                    })),
                )
            })
            .automation_enabled(false, AutomationRole::Status, label)
    }
    fn render_history_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if !self.history().scroll_observed {
            let list = self.history().scroll.clone();
            let owner = cx.entity().downgrade();
            list.set_scroll_handler(move |_, _, cx| {
                let owner = owner.clone();
                cx.defer(move |cx| {
                    let _ = owner.update(cx, |v, cx| {
                        if !v.history().follow_locked {
                            v.history_mut().following_latest =
                                v.history().scroll.is_scrolled_to_end().unwrap_or(true);
                        }
                        v.history_action(Action::Scroll, cx);
                        crate::components::region::invalidate(cx, &["history"]);
                        cx.notify();
                    });
                });
            });
            self.history_mut().scroll_observed = true;
        }
        if !self.history().positioned && self.history_paging().loaded {
            self.history_mut().positioned = true;
            self.history().scroll.scroll_to_end();
        }
        let owner = cx.entity().downgrade();
        let width = self.history().rendered_width;
        let follow_focus =
            crate::components::history::focus_for("history-follow-focus".into(), window, cx);
        div()
            .relative()
            .w_full()
            .h_full()
            .flex_shrink_0()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let measured = bounds.size.width.as_f32();
                        if (width - measured).abs() > 0.5 {
                            let _ = owner.update(cx, |v, cx| {
                                v.history_mut().rendered_width = measured;
                                cx.emit(HistoryChanged::clock());
                                cx.notify();
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(self.history_statistics().render(self.history_text(), width, window, cx))
            .child(
                // The page body holds the scroll region the follow button
                // floats over; the record list pads the records 12/16/24.
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .child(
                        div()
                            .id("history-ledger")
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .px(px(16.))
                            .pt(px(12.))
                            .pb(px(24.))
                            .child(
                                gpui::list(
                                    self.history().scroll.clone(),
                                    cx.processor(move |v, index, window, cx| {
                                        if index == 0 {
                                            v.render_history_older(cx).into_any_element()
                                        } else if index <= v.history_mut().rows.len() {
                                            live::row(v, index - 1, window, cx)
                                        } else {
                                            div().into_any_element()
                                        }
                                    }),
                                )
                                .flex_1()
                                .min_h_0(),
                            )
                            .automation(
                                AutomationRole::ScrollArea,
                                self.history_text().text("history_records"),
                            ),
                    )
                    .when(!self.history().following_latest, |body| {
                        body.child(
                            div()
                                .absolute()
                                .bottom(px(16.))
                                .left_0()
                                .right_0()
                                .flex()
                                .justify_center()
                                .child(
                                    div()
                                        .id("history-follow-latest")
                                        .block_mouse_except_scroll()
                                        .track_focus(&follow_focus)
                                        .tab_stop(true)
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .px(px(12.))
                                        .py(px(7.))
                                        .rounded_full()
                                        .border_1()
                                        .border_color(rgb(BORDER()))
                                        .bg(rgb(ZORK_UI.palette.canvas))
                                        .shadow_sm()
                                        .text_size(px(12.))
                                        .text_color(rgb(DIM()))
                                        .cursor_pointer()
                                        .child(crate::controls::icon(
                                            "interface/chevron-down.svg",
                                            12.,
                                        ))
                                        .child(self.history_text().text("history_latest"))
                                        .on_click(cx.listener(|v, _, _, cx| {
                                            cx.stop_propagation();
                                            v.follow_history_latest(cx);
                                        }))
                                        .automation(
                                            AutomationRole::Button,
                                            self.history_text().text("history_latest"),
                                        ),
                                )
                                // Appears only when reading history; fades in, never slides.
                                .with_animation(
                                    "history-follow-latest-enter",
                                    crate::motion::enter(crate::motion::BASE),
                                    |pill, t| pill.opacity(t),
                                ),
                        )
                    }),
            )
    }

    fn follow_history_latest(&mut self, cx: &mut Context<Self>) {
        self.history_mut().following_latest = true;
        self.history_mut().follow_locked = false;
        self.history().scroll.scroll_to_end();
        crate::components::region::invalidate(cx, &["history"]);
        cx.notify();
    }

    /// A model reply is the row: it has no icon or label, so the
    /// Markdown document, its disclosure and the absolute record clock stand
    /// alone. Usage belongs to the page overview, never to one row.
    fn render_history_output(
        &self,
        index: usize,
        id: &str,
        text: &str,
        entry: &Entry,
        focus: FocusHandle,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let expanded = self.history().output_expanded.contains(id);
        // the record list pads the records 16px on both sides, so the
        // document lays out at the panel width minus that inset.
        let width = (self.history().rendered_width - 32.).max(1.);
        let document = self.history().document(id, text);
        let height = {
            let mut previews = self.history().previews.borrow_mut();
            if previews.len() > 128 {
                previews.clear();
            }
            previews
                .entry(format!("{id}:{width}"))
                .or_insert_with(|| Rc::new(std::cell::Cell::new(110.)))
                .clone()
        };
        let limit = height.get();
        let owner = cx.entity().downgrade();
        let disclosure_label = self.history_text().text(if expanded {
            "history_output_show_less"
        } else {
            "history_output_show_more"
        });
        let toggle = id.to_owned();
        let click_focus = focus.clone();
        // The output disclosure sits 8px under the document; the
        // record clock, when revealed, carries a 6px bottom margin.
        let mut footer = div().w_full().flex().flex_col();
        if expanded {
            footer = footer.child(record_time(index, entry.start.or(entry.end)));
        }
        let disclosure = div()
            .id(format!("history-output-disclosure-{index}"))
            .flex()
            .items_center()
            .gap(px(4.))
            .mt(px(8.))
            .cursor_pointer()
            .track_focus(&focus)
            .tab_stop(true)
            .text_size(px(12.))
            .text_color(rgb(SUBTLE()))
            .child(disclosure_label.clone())
            .when(expanded, |v| {
                v.child(
                    crate::controls::icon("interface/chevron-down.svg", 12.).with_transformation(
                        gpui::Transformation::rotate(gpui::radians(std::f32::consts::PI)),
                    ),
                )
            })
            .on_click(cx.listener(move |v, _, window, cx| {
                window.focus(&click_focus, cx);
                cx.stop_propagation();
                v.toggle_history_output(index, &toggle, cx);
            }))
            .automation(AutomationRole::Button, disclosure_label);
        footer = footer.child(disclosure);
        crate::components::message_preview::MessagePreview {
            lines: Some(crate::components::message_preview::LinePreview {
                height,
                notify: Rc::new(move |cx| {
                    let _ = owner.update(cx, |_, cx| {
                        cx.emit(HistoryChanged::clock());
                        crate::components::region::invalidate(cx, &["history"]);
                        cx.notify();
                    });
                }),
            }),
            // History keeps the shared Zork Markdown presentation. Only the
            // preview and disclosure belong to the history page.
            body: div()
                .text_size(px(13.))
                .line_height(px(22.))
                .child(render_document(
                    &format!("history-output-document-{index}"),
                    document.as_ref(),
                ))
                .into_any_element(),
            footer: footer.into_any_element(),
            width,
            limit,
            more: false,
            expanded,
            fade: false,
            background: rgb(ZORK_UI.palette.canvas).into(),
        }
    }

    fn toggle_history_output(&mut self, index: usize, id: &str, cx: &mut Context<Self>) {
        self.history_mut().hold_disclosure(index);
        if !self.history_mut().output_expanded.remove(id) {
            self.history_mut().output_expanded.insert(id.to_owned());
        }
        crate::components::region::invalidate_all(cx);
        cx.emit(HistoryChanged::clock());
        cx.notify();
    }

    /// The model reply alone owns a Markdown body, so it is the one row with no
    /// line: no icon box, no label, no status.
    fn render_history_reply(
        &self,
        index: usize,
        activity: &Activity,
        entry: &Entry,
        focus: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let row = div().id(("history-row", index)).py(px(18.)).child(
            // The reply row is a plain document: no icon, no label, no
            // status. The automation surface still names it by its text.
            div()
                .id(("history-record", index))
                .child(self.render_history_output(
                    index,
                    &entry.id,
                    &activity.summary,
                    entry,
                    focus,
                    window,
                    cx,
                ))
                .automation(AutomationRole::Status, activity.summary.clone()),
        );
        row
    }

    fn render_history_activity(
        &self,
        index: usize,
        now: i64,
        focus: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.history_row_built();
        let row = self.history().rows[index];
        let block = &self.history().projection.blocks[row.block];
        let a_index = row.activity.unwrap_or(block.start);
        let a = &self.history().projection.activities[a_index];
        let entry = &self.history().entries[a.entry];
        if a.kind == Kind::End && entry.state == "succeeded" {
            return div()
                .id(("history-row", index))
                // A finished turn ends with space, not a rule.
                .h(px(13.))
                .into_any_element();
        }
        let first_entry =
            &self.history().entries[self.history().projection.activities[block.start].entry];
        let group_id = first_entry.id.clone();
        let group = row.activity.is_none();
        let (mut subject, mut jump) = if group {
            (None, None)
        } else {
            self.history_subject(a, entry)
        };
        if !group && !a.details.files.is_empty() {
            // The file reads inline in the record's details, never in a dialog.
            subject = Some(a.details.files[0].path.clone());
            jump = None;
        }
        if !group && a.kind == Kind::Input {
            return self
                .render_history_message(index, a, entry, subject, jump, false, focus, cx)
                .into_any_element();
        }
        if !group && a.kind == Kind::SendMessage {
            return self
                .render_history_message(index, a, entry, subject, jump, true, focus, cx)
                .into_any_element();
        }
        let expanded = self.history().records_expanded.contains(&entry.id);
        let action = if group {
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
            .map(|(count, key)| {
                self.history_text()
                    .text(key)
                    .replace("{count}", &count.to_string())
            })
            .collect::<Vec<_>>()
            .join(" · ")
        } else if a.kind == Kind::Thinking {
            entry
                .start
                .map(|start| {
                    self.history_text()
                        .text("history_thinking_duration")
                        .replace("{seconds}", &thinking_seconds(now.saturating_sub(start)))
                })
                .unwrap_or_else(|| self.history_text().text("history_thinking_unmeasured"))
        } else if matches!(a.kind, Kind::SendMessage | Kind::SendFile) {
            if subject.is_some() {
                self.history_text()
                    .text("history_message_send_to")
                    .to_owned()
            } else {
                self.history_text()
                    .text("history_message_send_to_name")
                    .replace(
                        "{name}",
                        &self.history_text().text("history_ref_fallback").to_owned(),
                    )
            }
        } else if a.kind == Kind::Wait {
            let reason = a
                .summary
                .strip_prefix("waiting for ")
                .or_else(|| a.summary.strip_prefix("wait for "))
                .or_else(|| a.summary.trim_start().strip_prefix("等待"))
                .map(str::trim)
                .filter(|reason| !reason.is_empty());
            let mut label = reason.map_or_else(
                || self.history_text().text("history_action_wait").to_owned(),
                |reason| {
                    self.history_text()
                        .text("history_wait_target")
                        .replace("{reason}", reason)
                },
            );
            label.push(' ');
            label.push_str(&wait_time(a, entry, now, &self.history_text()));
            label
        } else if a.kind == Kind::UnknownTool {
            self.history_text().text("history_action_tool")
        } else {
            self.history_text().text(kind_label(a.kind)).to_owned()
        };
        let mut summary = if group {
            // The group line carries only its counts; the members appear when
            // the group is expanded.
            String::new()
        } else if matches!(
            a.kind,
            Kind::Thinking | Kind::Wait | Kind::SendMessage | Kind::SendFile
        ) {
            // A wait reason and a message body already read as the label, so
            // these rows carry no trailing text.
            String::new()
        } else {
            a.summary.clone()
        };
        if !group && subject.as_ref() == Some(&summary) {
            summary.clear();
        }
        let tail = a.kind == Kind::Thinking;
        let status = if group {
            None
        } else {
            match entry.state.as_str() {
                // A wait and a live call read as their own label: only a
                // failure or a cancellation adds a status there.
                "running" if matches!(a.kind, Kind::Wait | Kind::Thinking | Kind::Output) => None,
                "running" => Some(self.history_text().text("history_running").into()),
                "failed" | "timed_out" => Some(self.history_text().text("history_error").into()),
                "cancelled" | "interrupted" => {
                    Some(self.history_text().text("history_cancelled").into())
                }
                "succeeded"
                    if matches!(a.kind, Kind::SendMessage | Kind::SendFile | Kind::Notify) =>
                {
                    None
                }
                // Only operations carry a terminal status. A reply, a live call
                // and a finished wait read as their own label.
                "succeeded" if matches!(a.kind, Kind::Output | Kind::Thinking | Kind::Wait) => None,
                "succeeded" => Some(self.history_text().text("history_success").into()),
                _ => None,
            }
        };
        let id = entry.id.clone();
        if !group && a.kind == Kind::Output {
            return self
                .render_history_reply(index, a, entry, focus, window, cx)
                .into_any_element();
        }
        let live = entry.state == "running"
            && !matches!(a.kind, Kind::SendMessage | Kind::SendFile | Kind::Notify);
        div()
            .id(("history-row", index))
            .when(
                !group
                    && block.is_group()
                    && block.end - block.start - usize::from(block.active.is_some()) > 1
                    && block.active != Some(a_index),
                |v| {
                    let first = (block.start..block.end).find(|i| Some(*i) != block.active);
                    let last = (block.start..block.end)
                        .rev()
                        .find(|i| Some(*i) != block.active);
                    v.when(first == Some(a_index), |v| v.pt(px(2.)))
                        .when(last == Some(a_index), |v| v.pb(px(4.)))
                },
            )
            .child(crate::components::history::activity_header(
                ("history-record", index),
                ActivityHeader {
                    focus,
                    // A group uses a console when every member is a
                    // command and with a folder otherwise.
                    icon: Some(if group {
                        if block.counts.shell > 0
                            && block.counts.read == 0
                            && block.counts.written == 0
                            && block.counts.edited == 0
                            && block.counts.queries == 0
                        {
                            "history/terminal.svg"
                        } else {
                            "interface/folder1.svg"
                        }
                    } else if matches!(entry.state.as_str(), "failed" | "timed_out") {
                        "interface/x.svg"
                    } else {
                        kind_icon(a.kind)
                    }),
                    color: if group {
                        SUBTLE()
                    } else {
                        activity_color(a.kind, &entry.state)
                    },
                    accent: !group && activity_accent(a.kind),
                    action,
                    subject,
                    clickable_subject: jump.is_some(),
                    summary,
                    tail,
                    status,
                    failed: matches!(entry.state.as_str(), "failed" | "timed_out"),
                    live,
                    tabular: !group && a.kind == Kind::Wait,
                    group_summary: group,
                    chevron: if group {
                        self.history().expanded.contains(&group_id)
                    } else {
                        expanded
                    },
                },
                window,
                cx,
                move |v, _, cx| {
                    v.history_mut().hold_disclosure(index);
                    if group {
                        if !v.history_mut().expanded.remove(&group_id) {
                            v.history_mut().expanded.insert(group_id.clone());
                        }
                        v.history_mut().rebuild_rows();
                    } else {
                        if !v.history_mut().records_expanded.remove(&id) {
                            v.history_mut().records_expanded.insert(id.clone());
                        }
                    }
                    crate::components::region::invalidate_all(cx);
                    cx.emit(HistoryChanged::clock());
                },
                move |v, _, cx| {
                    if let Some(jump) = jump.clone() {
                        v.history_action(Action::Jump(jump), cx);
                    }
                },
            ))
            .when(!group && expanded, |row| {
                row.child(self.render_history_details(index, a, entry, cx))
            })
            .into_any_element()
    }

    fn toggle_history_record(&mut self, index: usize, id: &str, cx: &mut Context<Self>) {
        self.history_mut().hold_disclosure(index);
        if !self.history_mut().records_expanded.remove(id) {
            self.history_mut().records_expanded.insert(id.to_owned());
        }
        crate::components::region::invalidate_all(cx);
        cx.emit(HistoryChanged::clock());
        cx.notify();
    }

    /// A received message is a quote under its source; a message sent to a
    /// Chat is a light persimmon block. Both preview three lines and open
    /// inline with their record clock.
    #[allow(clippy::too_many_arguments)]
    fn render_history_message(
        &self,
        index: usize,
        activity: &Activity,
        entry: &Entry,
        subject: Option<String>,
        jump: Option<Jump>,
        sent: bool,
        focus: FocusHandle,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let text = self.history_text();
        let expanded = self.history().records_expanded.contains(&entry.id);
        let body = if activity.details.text.trim().is_empty() {
            activity.summary.clone()
        } else {
            activity.details.text.clone()
        };
        let unknown = text.text("history_source_unknown");
        let name = subject
            .filter(|name| !name.is_empty() && name != &unknown)
            .unwrap_or_else(|| text.text("history_ref_fallback"));
        let template = text.text(if sent {
            "history_message_send_to_chat"
        } else {
            "history_message_received_from"
        });
        let heading = template.replace("{name}", &name);
        let (before, after) = template
            .split_once("{name}")
            .map(|(before, after)| (before.to_owned(), after.to_owned()))
            .unwrap_or_else(|| (template.clone(), String::new()));
        let failed = matches!(entry.state.as_str(), "failed" | "timed_out");
        // The source or destination is the one link on the line, in the
        // primary ink rather than a separate link colour.
        let target = match jump {
            Some(jump) => div()
                .id(format!("history-target-{index}"))
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(TEXT()))
                .cursor_pointer()
                .focusable()
                .tab_stop(true)
                .child(name.clone())
                .on_click(cx.listener(move |v, _, _, cx| {
                    cx.stop_propagation();
                    v.history_action(Action::Jump(jump.clone()), cx);
                }))
                .automation(AutomationRole::Button, name.clone())
                .into_any_element(),
            None => div()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(DIM()))
                .child(name.clone())
                .into_any_element(),
        };
        let icon_color = if failed {
            crate::components::history::ACTIVITY_ERROR_COLOR()
        } else if sent {
            crate::design::INTERACTION.accent
        } else {
            SUBTLE()
        };
        let header = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .min_w_0()
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(rgb(SUBTLE()))
            .child(
                crate::controls::icon(kind_icon(activity.kind), 14.)
                    .flex_shrink_0()
                    .text_color(rgb(icon_color)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .whitespace_nowrap()
                    .when(!before.is_empty(), |v| v.child(before))
                    .child(target)
                    .when(!after.is_empty(), |v| v.child(after)),
            )
            .when(failed, |v| {
                v.child(
                    div()
                        .flex_shrink_0()
                        .text_color(rgb(crate::components::history::ACTIVITY_ERROR_COLOR()))
                        .child(text.text("history_error")),
                )
            });
        let long = body.lines().count() > MESSAGE_LINES;
        let content = div()
            .flex_1()
            .min_w_0()
            .text_size(px(13.))
            .line_height(px(21.))
            .text_color(rgb(TEXT()))
            .whitespace_normal();
        // Opened, the complete text is the record's inline detail.
        let content = if expanded {
            content
                .id(("history-inline-detail", index))
                .child(body.clone())
                .automation(AutomationRole::Status, body.clone())
                .into_any_element()
        } else {
            content
                .line_clamp(MESSAGE_LINES)
                .child(body.clone())
                .into_any_element()
        };
        let id = entry.id.clone();
        let click_focus = focus.clone();
        let record = div()
            .id(("history-record", index))
            .track_focus(&focus)
            .tab_stop(true)
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(4.))
            .cursor_pointer()
            .on_click(cx.listener(move |v, _, window, cx| {
                window.focus(&click_focus, cx);
                v.toggle_history_record(index, &id, cx);
            }))
            .child(header);
        let record = if sent {
            record
                .px(px(12.))
                .py(px(8.))
                .rounded(px(crate::design::RADIUS.block))
                .bg(sent_wash())
                .child(content)
        } else {
            record.child(
                div()
                    .flex()
                    .gap(px(10.))
                    .min_w_0()
                    .child(
                        div()
                            .w(px(2.))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(rgb(QUOTE())),
                    )
                    .child(content),
            )
        };
        div().id(("history-row", index)).py(px(6.)).child(
            record
                .when(expanded, |v| {
                    v.child(record_time(index, entry.start.or(entry.end)))
                })
                .when(!expanded && long, |v| {
                    v.child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(SUBTLE()))
                            .child(text.text("history_output_show_more")),
                    )
                })
                .automation(AutomationRole::Button, format!("{heading} {body}")),
        )
    }

    fn render_history_details(
        &self,
        index: usize,
        activity: &Activity,
        entry: &Entry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let detail = &activity.details;
        let code = || {
            div()
                .min_w_0()
                .font_family(crate::assets::CODE_FONT_FAMILY)
                .text_size(px(12.))
                .line_height(px(19.))
                .whitespace_normal()
        };
        div()
            .id(("history-inline-detail", index))
            .min_w_0()
            .mt(px(3.))
            .mb(px(8.))
            // Aligned under the label: the 14px icon plus the 8px gap.
            .ml(px(22.))
            .pt(px(4.))
            .pb(px(6.))
            .pl(px(12.))
            .border_l_1()
            .border_color(rgb(BORDER()))
            .text_size(px(12.))
            .line_height(px(20.))
            .text_color(rgb(DIM()))
            .child(record_time(index, entry.start.or(entry.end)))
            .when(!detail.command.is_empty(), |body| {
                body.child(code().child(format!("$ {}", detail.command)))
            })
            .when(!detail.text.is_empty(), |body| {
                body.child(div().whitespace_normal().child(detail.text.clone()))
            })
            .when(!detail.output.is_empty(), |body| {
                body.child(code().mt(px(4.)).child(detail.output.clone()))
            })
            // A file reads inline: its path, the replaced text when an edit
            // recorded it, then the first lines with their own disclosure.
            .children(detail.files.iter().enumerate().map(|(i, file)| {
                let key = format!("{}#file{i}", entry.id);
                let open = self.history().output_expanded.contains(&key);
                let lines = file.content.lines().count();
                let shown = if open || lines <= FILE_LINES {
                    file.content.clone()
                } else {
                    file.content
                        .lines()
                        .take(FILE_LINES)
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                let label = self.history_text().text(if open {
                    "history_output_show_less"
                } else {
                    "history_output_show_more"
                });
                div()
                    .id(format!("history-file-{index}-{i}"))
                    .mt(px(6.))
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .min_w_0()
                            .text_color(rgb(TEXT()))
                            .child(crate::controls::icon("interface/file.svg", 14.).flex_shrink_0())
                            .child(div().min_w_0().truncate().child(file.path.clone())),
                    )
                    .when_some(file.previous.clone(), |v, previous| {
                        v.child(
                            code()
                                .mt(px(4.))
                                .text_color(rgb(SUBTLE()))
                                .child(previous),
                        )
                    })
                    .child(code().mt(px(4.)).child(shown))
                    .when(lines > FILE_LINES, |v| {
                        v.child(
                            div()
                                .id(format!("history-file-disclosure-{index}-{i}"))
                                .mt(px(4.))
                                .text_color(rgb(SUBTLE()))
                                .cursor_pointer()
                                .child(label.clone())
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    cx.stop_propagation();
                                    v.toggle_history_output(index, &key, cx);
                                }))
                                .automation(AutomationRole::Button, label),
                        )
                    })
                    .automation(AutomationRole::Status, file.path.clone())
            }))
            .when(detail.truncated, |body| {
                body.child(
                    div()
                        .mt(px(8.))
                        .text_color(rgb(SUBTLE()))
                        .child(self.history_text().text("history_content_truncated")),
                )
            })
            .automation(
                AutomationRole::Status,
                [
                    detail.text.as_str(),
                    detail.command.as_str(),
                    detail.output.as_str(),
                ]
                .join("\n"),
            )
    }
}
/// The record clock is an absolute local `HH:MM:SS` at 12px tertiary, rendered
/// only inside the content a disclosure reveals. Rows carry no relative age.
fn record_time(index: usize, timestamp: Option<i64>) -> impl IntoElement {
    let clock = timestamp.map_or_else(String::new, |ms| model::clock(Some(ms)));
    div()
        .id(("history-output-clock", index))
        .mb(px(6.))
        .text_size(px(12.))
        .text_color(rgb(SUBTLE()))
        .font_features(crate::components::history::tabular_nums())
        .child(clock.clone())
        .automation(AutomationRole::Status, clock)
}

/// Format a thinking duration with one decimal below ten seconds.
fn thinking_seconds(ms: i64) -> String {
    let seconds = (ms.max(0) as f64) / 1000.;
    if ms < 10_000 {
        format!("{seconds:.1}").trim_end_matches(".0").to_owned()
    } else {
        format!("{:.0}", seconds)
    }
}

/// The wait's own timing: the finished elapsed, the live progress against the
/// requested maximum, or unmeasured and maximum-only wording.
fn wait_time(a: &Activity, entry: &Entry, now: i64, text: &Text) -> String {
    let maximum = a
        .requested_wait_ms
        .map(|ms| (ms.max(0) as f64 / 1000.).round() as i64);
    let elapsed = entry
        .duration(now)
        .map(|ms| (ms.max(0) as f64 / 1000.).floor() as i64);
    match (elapsed, maximum) {
        (Some(elapsed), _) if entry.state != "running" => text
            .text("history_wait_finished")
            .replace("{elapsed}", &elapsed.to_string()),
        (Some(elapsed), Some(maximum)) => text
            .text("history_wait_progress")
            .replace("{elapsed}", &elapsed.to_string())
            .replace("{maximum}", &maximum.to_string()),
        (_, Some(maximum)) => format!(
            "{} · {}",
            text.text("history_wait_maximum")
                .replace("{maximum}", &maximum.to_string()),
            text.text("history_wait_unmeasured")
        ),
        _ => text.text("history_wait_unmeasured").to_owned(),
    }
}

#[cfg(feature = "stories")]
pub mod stories;

#[cfg(test)]
mod tests {
    use super::{model, thinking_seconds, wait_time, Text};
    use crate::history::activity::{Activity, Kind};
    use crate::history::Entry;
    use std::rc::Rc;

    /// Only the wait wording needs templates; every other key reads as itself.
    fn text() -> Text {
        Text(Rc::new(|key| match key {
            "history_wait_finished" => "waited {elapsed}s".to_owned(),
            "history_wait_progress" => "{elapsed}s/{maximum}s".to_owned(),
            "history_wait_maximum" => "<={maximum}s".to_owned(),
            "history_wait_unmeasured" => "unmeasured".to_owned(),
            _ => key.to_owned(),
        }))
    }

    fn entry(state: &str, start: i64, end: Option<i64>) -> Entry {
        Entry {
            id: "wait".into(),
            lane: 2,
            action: "wait".into(),
            summary: String::new(),
            start: Some(start),
            end,
            state: state.into(),
            raw: vec![],
            usage: None,
            model: None,
            outcome_summary: None,
        }
    }

    fn wait(seconds: Option<f64>) -> Activity {
        Activity {
            entry: 0,
            kind: Kind::Wait,
            subject: None,
            summary: String::new(),
            routine: None,
            requested_wait_ms: seconds.map(|seconds| (seconds * 1000.) as i64),
            details: Default::default(),
        }
    }

    #[test]
    fn a_record_clock_is_absolute_wall_time() {
        let clock = model::clock(Some(15_000));
        assert_eq!(clock.len(), 8, "{clock}");
        assert_eq!(clock.matches(':').count(), 2, "{clock}");
        assert_eq!(model::clock(None), "—");
    }

    #[test]
    fn a_thinking_duration_keeps_one_decimal_below_ten_seconds() {
        for (ms, expected) in [(0, "0"), (4_000, "4"), (4_500, "4.5"), (42_000, "42")] {
            assert_eq!(thinking_seconds(ms), expected);
        }
    }

    #[test]
    fn a_wait_reads_its_finished_elapsed_or_its_remaining_maximum() {
        let text = text();
        assert_eq!(
            wait_time(
                &wait(Some(20.)),
                &entry("succeeded", 0, Some(15_000)),
                20_000,
                &text
            ),
            "waited 15s"
        );
        assert_eq!(
            wait_time(&wait(Some(20.)), &entry("running", 0, None), 5_000, &text),
            "5s/20s"
        );
        assert_eq!(
            wait_time(&wait(Some(20.)), &entry("running", 0, None), 0, &text),
            "0s/20s"
        );
        assert_eq!(
            wait_time(&wait(None), &entry("running", 0, None), 5_000, &text),
            "unmeasured"
        );
    }
}
