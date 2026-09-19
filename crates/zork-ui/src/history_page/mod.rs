//! Complete history page. Native and Playground supply the same read-only
//! snapshots, resolved destinations and typed event adapter.
use crate::components::{
    history::{activity_accent, activity_color, kind_icon, kind_label, ActivityHeader},
    loading,
    message::{render_document, MessageDocument},
};
use crate::history::{
    self as model,
    activity::{self, Activity, Kind, Projection, Row},
    Entry,
};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::CUE_UI,
    resources::Text,
};
use gpui::{prelude::*, *};
use std::{collections::HashSet, time::Duration};
const DIM: u32 = CUE_UI.palette.muted;
const TEXT: u32 = CUE_UI.palette.text;
const SUBTLE: u32 = CUE_UI.palette.subtle;
mod live;
pub use live::HistoryChanged;
mod statistics;
pub use statistics::{Runtime, Statistics};

#[derive(Clone)]
pub enum Jump {
    Conversation(String),
    Agent(String),
    Entry(String),
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
    pub rendered_width: f32,
    pub scroll: ListState,
    pub scroll_observed: bool,
    pub timeline: Option<Entity<crate::history_timeline::Timeline>>,
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
            rendered_width: 440.,
            scroll: ListState::new(1, ListAlignment::Top, px(200.)),
            scroll_observed: false,
            timeline: None,
            fixed_now: None,
            clock_offset: 0,
        }
    }
}
impl State {
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
        self.rows = self.projection.rows(self.entries.iter(), &self.expanded);
        self.scroll.splice(1..previous + 1, self.rows.len());
    }
}

pub trait Host: Sized + EventEmitter<HistoryChanged> + 'static {
    fn history(&self) -> &State;
    fn history_mut(&mut self) -> &mut State;
    fn history_text(&self) -> Text;
    fn history_subject(&self, activity: &Activity, entry: &Entry)
        -> (Option<String>, Option<Jump>);
    fn history_source(&self, cx: &App) -> crate::components::liquid::overlay::SourceBinding;
    fn history_action(&mut self, action: Action, cx: &mut Context<Self>);
    fn history_paging(&self) -> Paging;
    fn history_statistics(&mut self) -> Statistics;
    fn history_row_built(&self) {}
    fn history_select(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(e) = self.history().entries.get(index) {
            let id = e.id.clone();
            self.history_mut().selected = Some(id.clone());
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
    fn render_history_older(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let paging = self.history_paging();
        div()
            .id("history-older")
            .w_full()
            .py(px(3.))
            .line_height(px(16.5))
            .text_center()
            .text_size(px(11.))
            .text_color(rgb(DIM))
            .when(!paging.busy && (paging.older || paging.error), |v| {
                v.cursor_pointer()
            })
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .when(
                paging.busy && (!paging.loaded || paging.loading_older),
                |v| v.child(loading::indicator("history-loading", 14.)),
            )
            .child(self.history_text().text(
                if paging.busy && (!paging.loaded || paging.loading_older) {
                    "history_loading"
                } else if paging.error {
                    "history_failed"
                } else if paging.older {
                    "history_older"
                } else {
                    "history_start"
                },
            ))
            .on_click(cx.listener(|v, _, _, cx| {
                let paging = v.history_paging();
                if !paging.busy && (paging.older || paging.error) {
                    v.history_action(Action::LoadOlder, cx);
                }
            }))
            .automation_enabled(
                !paging.busy && (paging.older || paging.error),
                AutomationRole::Button,
                self.history_text().text("history_older"),
            )
    }
    fn render_history_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let paging = self.history_paging();
        if !self.history().scroll_observed {
            let list = self.history().scroll.clone();
            let owner = cx.entity().downgrade();
            list.set_scroll_handler(move |_, _, cx| {
                let _ = owner.update(cx, |v, cx| v.history_action(Action::Scroll, cx));
            });
            self.history_mut().scroll_observed = true;
        }
        let selection_range = self
            .history()
            .timeline
            .as_ref()
            .and_then(|view| view.read(cx).selection_range());
        div()
            .relative()
            .w_full()
            .h_full()
            .flex_shrink_0()
            .min_h_0()
            .flex()
            .flex_col()
            .font_weight(FontWeight(450.))
            .child(
                div()
                    .id("history-ledger")
                    .when(
                        paging.loaded && self.history().rows.is_empty() && !paging.error,
                        |v| {
                            v.child(
                                div()
                                    .p_4()
                                    .text_size(px(12.))
                                    .text_color(rgb(DIM))
                                    .child(self.history_text().text("history_empty")),
                            )
                        },
                    )
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        gpui::list(
                            self.history().scroll.clone(),
                            cx.processor(move |v, index, window, cx| {
                                if index == 0 {
                                    v.render_history_older(cx).into_any_element()
                                } else if index <= v.history_mut().rows.len() {
                                    live::row(v, index - 1, selection_range, window, cx)
                                } else {
                                    div().into_any_element()
                                }
                            }),
                        )
                        .flex_1()
                        .min_h_0(),
                    )
                    .automation(AutomationRole::ScrollArea, self.history_text().text("history_records")),
            )
            .child(self.history_statistics().render(self.history_text()))
            .child(live::timeline(window, cx))
    }

    fn render_history_timeline_panel(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let text = self.history_text();
        if self.history().timeline.is_none() {
            let timeline = cx.new(|cx| crate::history_timeline::Timeline::new(text.clone(), cx));
            cx.subscribe(
                &timeline,
                |v, _, selected: &crate::history_timeline::Selected, cx| {
                    v.history_select(selected.0, cx);
                },
            )
            .detach();
            cx.subscribe(
                &timeline,
                |_, _, _: &crate::history_timeline::Changed, cx| {
                    crate::components::region::invalidate(cx, &["history", "header"]);
                },
            )
            .detach();
            self.history_mut().timeline = Some(timeline);
        }
        let timeline = self.history().timeline.as_ref().unwrap().clone();
        timeline.update(cx, |view, cx| {
            view.configure(
                self.history().entries.clone(),
                self.history().selected.clone(),
                self.history().now(),
                text,
                cx,
            )
        });
        timeline.into_any_element()
    }
    /// A model reply is the row: Cue paints no icon and no label on it, so the
    /// Markdown document, its disclosure and the absolute record clock stand
    /// alone. Usage belongs to the page overview, never to one row.
    fn render_history_output(
        &self,
        index: usize,
        id: &str,
        text: &str,
        entry: &Entry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let expanded = self.history().output_expanded.contains(id);
        let width = (self.history().rendered_width - 40.).max(1.);
        let document = MessageDocument::parse(text);
        let disclosure_label = self.history_text().text(if expanded {
            "history_output_show_less"
        } else {
            "history_output_show_more"
        });
        let toggle = id.to_owned();
        let mut footer = div().w_full().flex().flex_col().gap(px(2.)).pt(px(2.));
        if expanded {
            footer = footer.child(record_time(entry.start.or(entry.end)));
        }
        let disclosure = div()
            .id(format!("history-output-disclosure-{index}"))
            .flex()
            .items_center()
            .gap(px(4.))
            .cursor_pointer()
            .text_size(px(11.))
            .text_color(rgb(SUBTLE))
            .hover(|v| v.underline())
            .child(disclosure_label.clone())
            .when(expanded, |v| {
                v.child(crate::controls::icon("cue/chevron-down.svg", 12.))
            })
            .on_click(cx.listener(move |v, _, _, cx| {
                cx.stop_propagation();
                if !v.history_mut().output_expanded.remove(&toggle) {
                    v.history_mut().output_expanded.insert(toggle.clone());
                }
                crate::components::region::invalidate_all(cx);
            }))
            .automation(AutomationRole::Button, disclosure_label);
        footer = footer.child(disclosure);
        crate::components::message_preview::MessagePreview {
            body: render_document(&format!("history-output-document-{index}"), &document),
            footer: footer.into_any_element(),
            width,
            limit: 110.,
            more: false,
            expanded,
            background: rgb(CUE_UI.palette.canvas).into(),
        }
    }

    /// The model reply alone owns a Markdown body, so it is the one row with no
    /// line: no icon box, no label, no status.
    fn render_history_reply(
        &self,
        index: usize,
        activity: &Activity,
        entry: &Entry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut row = div()
            .id(("history-row", index))
            .my(px(18.))
            .child(
                // Cue's reply row is a plain document: no icon, no label, no
                // status. The automation surface still names it by its text.
                div()
                    .id(("history-record", index))
                    .child(self.render_history_output(
                        index,
                        &entry.id,
                        &activity.summary,
                        entry,
                        cx,
                    ))
                    .automation(AutomationRole::Status, activity.summary.clone()),
            );
        if self.history().selected.as_deref() == Some(entry.id.as_str()) {
            row = row.bg(rgb(CUE_UI.palette.sidebar_hover));
        }
        row
    }

    fn render_history_activity(
        &self,
        index: usize,
        now: i64,
        selection_range: Option<(i64, i64)>,
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
                .px(px(12.))
                .py(px(6.))
                .child(div().h(px(1.)).w_full().bg(rgba(0x80808030)))
                .into_any_element();
        }
        let first_entry =
            &self.history().entries[self.history().projection.activities[block.start].entry];
        let group_id = first_entry.id.clone();
        let group = row.activity.is_none();
        let (mut subject, jump) = if group {
            (None, None)
        } else {
            self.history_subject(a, entry)
        };
        let action = if group {
            let counts = &block.counts;
            [
                (counts.read, "history_group_read"),
                (counts.written, "history_group_write"),
                (counts.edited, "history_group_edit"),
                (counts.shell, "history_group_command"),
                (counts.queries, "history_group_query"),
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
        } else if a.kind == Kind::Input {
            // Cue reads the user's own message as a receipt from its resolved
            // source, with no separate target on the line.
            let unknown = self.history_text().text("history_source_unknown");
            let name = subject
                .take()
                .filter(|name| name != &unknown)
                .unwrap_or_else(|| self.history_text().text("history_ref_fallback").to_owned());
            self.history_text()
                .text("history_message_received_from")
                .replace("{name}", &name)
        } else if a.kind == Kind::Thinking {
            match entry.start.or(entry.end) {
                Some(start) => self
                    .history_text()
                    .text("history_thinking_duration")
                    .replace("{seconds}", &thinking_seconds(now.saturating_sub(start))),
                None => self.history_text().text("history_thinking_unmeasured").to_owned(),
            }
        } else if matches!(a.kind, Kind::SendMessage | Kind::SendFile) {
            if subject.is_some() {
                self.history_text().text("history_message_send_to").to_owned()
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
            activity::preview(&entry.action)
        } else {
            self.history_text().text(kind_label(a.kind)).to_owned()
        };
        let mut summary = if group {
            // Cue's group line carries only its counts; the members appear when
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
        let tail = matches!(a.kind, Kind::Input);
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
                    Some(self.history_text().text("history_sent").into())
                }
                // Only operations carry a terminal status. A reply, a live call
                // and a finished wait read as their own label.
                "succeeded"
                    if matches!(a.kind, Kind::Output | Kind::Thinking | Kind::Wait) =>
                {
                    None
                }
                "succeeded" => Some(self.history_text().text("history_success").into()),
                _ => None,
            }
        };
        let outside = selection_range.is_some_and(|(lo, hi)| {
            let start = if group {
                block.start_at
            } else {
                entry.start.or(entry.end)
            };
            let end = if group {
                block
                    .end_at
                    .map(|end| if block.running { end.max(now) } else { end })
            } else {
                entry
                    .end
                    .or_else(|| (entry.state == "running").then_some(now))
                    .or(entry.start)
            };
            end.is_none_or(|end| end < lo) || start.is_none_or(|start| start > hi)
        });
        let id = entry.id.clone();
        let selected = self.history().selected.as_ref() == Some(&id);
        if !group && a.kind == Kind::Output {
            return self.render_history_reply(index, a, entry, cx).into_any_element();
        }
        let live = entry.state == "running"
            && !matches!(a.kind, Kind::SendMessage | Kind::SendFile | Kind::Notify);
        div()
            .id(("history-row", index))
            .when(selected, |v| v.bg(rgb(CUE_UI.palette.sidebar_hover)))
            .when(outside, |v| v.opacity(0.3))
            .child(crate::components::history::activity_header_sources(
                ("history-record", index),
                ActivityHeader {
                    // Cue paints a group with a console when every member is a
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
                            "cue/folder1.svg"
                        }
                    } else {
                        kind_icon(a.kind)
                    }),
                    color: if group {
                        SUBTLE
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
                },
                (
                    (!group).then(|| self.history_source(cx)),
                    matches!(jump, Some(Jump::Agent(_) | Jump::Entry(_)))
                        .then(|| self.history_source(cx)),
                ),
                cx,
                move |v, _, cx| {
                    if group {
                        if !v.history_mut().expanded.remove(&group_id) {
                            v.history_mut().expanded.insert(group_id.clone());
                        }
                        v.history_mut().rebuild_rows();
                        if let Some(row) = v.history_mut().row_for_id(&group_id) {
                            // Keep the group heading visible rather than jumping to its last child.
                            let summary = v.history_mut().rows[..=row]
                                .iter()
                                .rposition(|r| r.activity.is_none())
                                .unwrap_or(row);
                            v.history_mut().scroll.scroll_to_reveal_item(summary + 1);
                        }
                    } else {
                        v.history_action(Action::OpenEntry(id.clone()), cx);
                        v.history_mut().selected = Some(id.clone());
                    }
                    crate::components::region::invalidate_all(cx);
                },
                move |v, _, cx| {
                    if let Some(jump) = jump.clone() {
                        v.history_action(Action::Jump(jump), cx);
                    }
                },
            ))
            .into_any_element()
    }
}
/// Cue's record clock is an absolute local `HH:MM:SS` at 9px tertiary, rendered
/// only inside the content a disclosure reveals. Rows carry no relative age.
fn record_time(timestamp: Option<i64>) -> Div {
    div()
        .mb(px(6.))
        .text_size(px(9.))
        .text_color(rgb(SUBTLE))
        .child(timestamp.map_or_else(String::new, |ms| model::clock(Some(ms))))
}

/// Cue formats a thinking duration with one decimal below ten seconds.
fn thinking_seconds(ms: i64) -> String {
    let seconds = (ms.max(0) as f64) / 1000.;
    if ms < 10_000 {
        format!("{seconds:.1}").trim_end_matches(".0").to_owned()
    } else {
        format!("{:.0}", seconds)
    }
}

/// The wait's own timing: the finished elapsed, the live progress against the
/// requested maximum, or Cue's unmeasured and maximum-only wording.
fn wait_time(a: &Activity, entry: &Entry, now: i64, text: &Text) -> String {
    let maximum = a
        .requested_wait_ms
        .map(|ms| (ms.max(0) as f64 / 1000.).round() as i64);
    let elapsed = entry.duration(now).map(|ms| (ms.max(0) as f64 / 1000.).floor() as i64);
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
