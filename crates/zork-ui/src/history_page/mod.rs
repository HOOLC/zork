//! Complete history page. Native and Playground supply the same read-only
//! snapshots, resolved destinations and typed event adapter.
use crate::components::{
    history::{activity_color, kind_icon, kind_label, ActivityHeader},
    loading,
};
use crate::history::{
    self as model,
    activity::{self, Activity, Kind, Projection, Row},
    Entry,
};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::ZORK_UI,
    resources::Text,
};
use gpui::{prelude::*, *};
use std::{collections::HashSet, time::Duration};
const DIM: u32 = ZORK_UI.palette.muted;
const TEXT: u32 = ZORK_UI.palette.text;
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
                    "history_retry"
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
                                    .child(self.history_text().text("history_no_activity")),
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
                    .automation(AutomationRole::ScrollArea, "History records"),
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
                .child(div().h(px(1.)).w_full().bg(rgba(0x80808030)));
        }
        let first_entry =
            &self.history().entries[self.history().projection.activities[block.start].entry];
        let group_id = first_entry.id.clone();
        let group = row.activity.is_none();
        let (subject, jump) = if group {
            (None, None)
        } else {
            self.history_subject(a, entry)
        };
        let action = if group {
            let counts = &block.counts;
            [
                (counts.read, "history_read_count"),
                (counts.written, "history_write_count"),
                (counts.shell, "history_shell_count"),
                (counts.queries, "history_query_count"),
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
        } else {
            let mut label = if a.kind == Kind::UnknownTool {
                activity::preview(&entry.action)
            } else {
                self.history_text().text(kind_label(a.kind)).to_owned()
            };
            if a.kind == Kind::Wait {
                if let Some(duration) = entry.duration(now) {
                    label.push(' ');
                    label.push_str(&model::duration(duration));
                }
            }
            label
        };
        let connector = (!group
            && subject.is_some()
            && subject.as_deref()
                != Some(self.history_text().text("history_source_unknown").as_str())
            && matches!(
                a.kind,
                Kind::Received
                    | Kind::SendMessage
                    | Kind::SendFile
                    | Kind::Notify
                    | Kind::Assign
                    | Kind::Rework
            ))
        .then(|| {
            self.history_text()
                .text(if a.kind == Kind::Received {
                    "history_from"
                } else {
                    "history_to"
                })
                .to_owned()
        });
        let mut summary = if group {
            block.summary.clone()
        } else {
            a.summary.clone()
        };
        if !group && subject.as_ref() == Some(&summary) {
            summary.clear();
        }
        if a.kind == Kind::Wait && summary.is_empty() {
            if let Some(ms) = a.requested_wait_ms {
                summary = self
                    .history_text()
                    .text("history_wait_requested")
                    .replace("{duration}", &model::duration(ms));
            }
        }
        let status = if group {
            None
        } else {
            match entry.state.as_str() {
                "running" => Some(
                    self.history_text()
                        .text(if a.kind == Kind::Wait {
                            "history_waiting"
                        } else {
                            "history_running"
                        })
                        .into(),
                ),
                "failed" | "timed_out" => Some(self.history_text().text("history_error").into()),
                "cancelled" | "interrupted" => {
                    Some(self.history_text().text("history_cancelled").into())
                }
                "succeeded"
                    if matches!(a.kind, Kind::SendMessage | Kind::SendFile | Kind::Notify) =>
                {
                    Some(self.history_text().text("history_sent").into())
                }
                _ => None,
            }
        };
        let first = if group { first_entry } else { entry };
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
        div()
            .id(("history-row", index))
            .when(selected, |v| v.bg(rgb(ZORK_UI.palette.sidebar_hover)))
            .when(outside, |v| v.opacity(0.3))
            .child(crate::components::history::activity_header_sources(
                ("history-record", index),
                ActivityHeader {
                    icon: if group {
                        "history/operations.svg"
                    } else {
                        kind_icon(a.kind)
                    },
                    color: if group {
                        DIM
                    } else {
                        activity_color(a.kind, &entry.state)
                    },
                    action,
                    connector,
                    subject,
                    clickable_subject: jump.is_some(),
                    summary,
                    time: relative_time(first.start.or(first.end), now, &self.history_text()),
                    status,
                    nested: row.activity.is_some() && block.is_group(),
                    group,
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
    }
}
fn relative_time(timestamp: Option<i64>, now: i64, locale: &Text) -> String {
    let Some(time) = timestamp else {
        return locale.text("history_unknown_time").into();
    };
    let age = now.saturating_sub(time);
    if age < 0 {
        return locale.text("history_clock_ahead").into();
    }
    let seconds = age / 1000;
    let (key, value) = match seconds {
        0..=4 => return locale.text("history_relative_just_now").into(),
        5..=59 => ("history_relative_seconds", seconds),
        60..=3599 => ("history_relative_minutes", seconds / 60),
        3600..=86399 => ("history_relative_hours", seconds / 3600),
        _ => ("history_relative_days", seconds / 86400),
    };
    locale.text(key).replace("{count}", &value.to_string())
}

#[cfg(feature = "stories")]
pub mod stories;

#[cfg(test)]
mod tests {
    use super::{relative_time, Text};
    #[test]
    fn relative_time_handles_unknown_future_and_unit_boundaries() {
        let text = Text(std::rc::Rc::new(|key| format!("{key}:{{count}}")));
        for (timestamp, now, expected) in [
            (None, 1000, "history_unknown_time:{count}"),
            (Some(1001), 1000, "history_clock_ahead:{count}"),
            (Some(0), 59000, "history_relative_seconds:59"),
            (Some(0), 60000, "history_relative_minutes:1"),
            (Some(0), 3600000, "history_relative_hours:1"),
        ] {
            assert_eq!(relative_time(timestamp, now, &text), expected);
        }
    }
}
