//! Shared history record presentation; event collection and paging belong to the host.
use crate::history::activity::Kind;
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::ZORK_UI,
    history::Entry,
};
use gpui::{div, prelude::*, px, relative, rgb, rgba, Context, Div, FontWeight};

// Restrained semantic accents, shared by rows, destinations and the timeline.
pub const SEND_COLOR: u32 = 0x536779;
pub const RECEIVE_COLOR: u32 = 0x5A6D62;
pub const MODEL_COLOR: u32 = 0x92969D;

/// History activity accents use utility ramp step 700: a received row
/// is utility blue, a sent row utility purple, a wait utility warning and a
/// failure utility error. The timeline keeps its own neutral accents.
pub const ACTIVITY_RECEIVE_COLOR: u32 = 0x175CD3;
pub const ACTIVITY_SEND_COLOR: u32 = 0x5925DC;
pub const ACTIVITY_WAIT_COLOR: u32 = crate::design::ZORK_UI.palette.warning;
pub const ACTIVITY_ERROR_COLOR: u32 = crate::design::ZORK_UI.palette.danger;

/// The 12% accent tint behind a row icon fills its 20px box.
fn accent_chip(color: u32) -> gpui::Rgba {
    rgba((color << 8) | 0x1F)
}

/// Tabular figures for clocks, waits
/// and overview facts. gpui exposes the OpenType `tnum` feature instead.
pub fn tabular_nums() -> gpui::FontFeatures {
    gpui::FontFeatures(std::sync::Arc::new(vec![("tnum".to_owned(), 1)]))
}

/// Rows with an observable activity kind carry the accent chip and the
/// accented label; ordinary operation rows stay neutral.
pub fn activity_accent(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Input | Kind::SendMessage | Kind::SendFile | Kind::Notify | Kind::Wait
    )
}

/// Shared history geometry. The host resolves labels and destinations; no
/// network, identity lookup or argument parsing occurs during row rendering.
pub struct ActivityHeader {
    pub focus: gpui::FocusHandle,
    /// A model reply carries only its Markdown document: it gets no icon box
    /// and no label, so the whole line is absent.
    pub icon: Option<&'static str>,
    pub color: u32,
    /// Observable activity rows accent the icon and
    /// label and tinting a chip behind the icon; neutral operation rows stay muted.
    pub accent: bool,
    pub action: String,
    pub subject: Option<String>,
    pub clickable_subject: bool,
    pub summary: String,
    /// A text row renders its trailing text as tail prose at the line size;
    /// an operation row renders its target and status at 11px.
    pub tail: bool,
    pub status: Option<String>,
    pub failed: bool,
    /// A live row spins a loading ring in place of its static icon.
    pub live: bool,
    /// The wait label and record clocks use tabular figures.
    pub tabular: bool,
    /// A group heading shrinks and ellipsises.
    pub group_summary: bool,
    /// An expanded group heading carries a 12px rotated chevron.
    pub chevron: bool,
}

pub fn activity_header<V: 'static>(
    id: impl Into<gpui::ElementId>,
    header: ActivityHeader,
    _window: &mut gpui::Window,
    cx: &mut Context<V>,
    open: impl Fn(&mut V, &mut gpui::Window, &mut Context<V>) + 'static,
    navigate: impl Fn(&mut V, &mut gpui::Window, &mut Context<V>) + 'static,
) -> impl IntoElement {
    let icon = header.icon;
    // The full chip needs 20px, while the row's text keeps its former start.
    let label_inset = if icon.is_some() { -4. } else { 0. };
    let open = std::rc::Rc::new(open);
    let navigate = std::rc::Rc::new(navigate);
    let id = id.into();
    let focus = header.focus.clone();
    let click_focus = focus.clone();
    let spinner = format!("history-spinner-{id:?}");
    let accessible = format!(
        "{} {} {} {}",
        header.action,
        header.subject.as_deref().unwrap_or_default(),
        header.summary,
        header.status.as_deref().unwrap_or_default(),
    );
    let subject_id = format!("history-target-{id:?}");
    let subject_label = header.subject.clone().unwrap_or_default();
    // An activity label is accented; an operation label uses the tertiary color.
    let label_color = if header.accent {
        header.color
    } else if !header.tail && !header.group_summary {
        crate::design::ZORK_UI.palette.muted
    } else {
        crate::design::ZORK_UI.palette.subtle
    };
    // The row icon is tertiary; an accent row tints it with the action
    // colour, a failed tool paints it error and a live row without an activity
    // kind reads primary.
    let icon_color = if header.failed {
        ACTIVITY_ERROR_COLOR
    } else if header.accent {
        header.color
    } else if header.live {
        crate::design::ZORK_UI.palette.text
    } else {
        crate::design::ZORK_UI.palette.subtle
    };
    let status_color = if header.failed {
        ACTIVITY_ERROR_COLOR
    } else {
        crate::design::ZORK_UI.palette.subtle
    };
    div()
        .id(id)
        .track_focus(&focus)
        .tab_stop(true)
        .flex()
        .items_center()
        .gap(px(8.))
        .w_full()
        .min_h(px(if header.tail || header.group_summary {
            26.
        } else {
            30.
        }))
        .min_w_0()
        .text_size(px(12.))
        .text_color(rgb(crate::design::ZORK_UI.palette.muted))
        .cursor_pointer()
        .focus_visible(|v| v.bg(rgb(ZORK_UI.palette.sidebar_hover)))
        .on_click(cx.listener(move |v, _, window, cx| {
            window.focus(&click_focus, cx);
            open(v, window, cx);
        }))
        .when_some(icon, |v, path| {
            v.child(
                div()
                    .relative()
                    .w(px(20.))
                    .h(px(26.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(header.accent, |v| {
                        v.child(
                            div()
                                .absolute()
                                .left_0()
                                .top(px(3.))
                                .w(px(20.))
                                .h(px(20.))
                                .rounded(px(crate::design::RADIUS.inline))
                                .bg(accent_chip(header.color)),
                        )
                    })
                    .child(if header.live {
                        div()
                            .text_color(rgb(icon_color))
                            .child(
                                crate::components::loading::indicator(spinner.clone(), 14.)
                                    .without_delay(),
                            )
                            .into_any_element()
                    } else {
                        crate::controls::icon(path, 14.)
                            .text_color(rgb(icon_color))
                            .into_any_element()
                    }),
            )
        })
        .child(match header.group_summary {
            // The group summary shrinks and ellipsises its counts.
            true => div()
                .min_w_0()
                .ml(px(label_inset))
                .text_size(px(11.))
                .text_color(rgb(label_color))
                .whitespace_nowrap()
                .truncate()
                .when(header.tabular, |v| v.font_features(tabular_nums()))
                .child(header.action),
            // The activity label is `flex: none` at 11px, medium when accented.
            false => div()
                .min_w_0()
                .ml(px(label_inset))
                .when(header.tail, |v| v.flex_shrink_0())
                .when(!header.tail, |v| v.truncate())
                .text_size(px(11.))
                .font_weight(if header.accent {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::NORMAL
                })
                .text_color(rgb(label_color))
                .whitespace_nowrap()
                .when(header.tabular, |v| v.font_features(tabular_nums()))
                .child(header.action),
        })
        .when(header.chevron, |v| {
            v.child(
                crate::controls::icon("interface/chevron-down.svg", 12.)
                    .text_color(rgb(crate::design::ZORK_UI.palette.subtle))
                    .flex_shrink_0(),
            )
        })
        .when_some(header.subject, |v, text| {
            // The record target may shrink (`flex: 0 1 auto`) so a
            // long target ellipsises instead of pushing the status off the line.
            let subject = div()
                .min_w_0()
                .text_size(px(11.))
                .text_color(rgb(if header.clickable_subject {
                    crate::design::ZORK_UI.palette.text
                } else {
                    crate::design::ZORK_UI.palette.muted
                }))
                .truncate()
                .child(text);
            if !header.clickable_subject {
                // Plain targets carry no link decoration and no extra hitbox.
                return v.child(subject.into_any_element());
            }
            v.child(
                subject
                    .id(subject_id)
                    .focusable()
                    .tab_stop(true)
                    .cursor_pointer()
                    .underline()
                    .on_click(cx.listener(move |v, _, window, cx| {
                        cx.stop_propagation();
                        navigate(v, window, cx);
                    }))
                    .automation(AutomationRole::Button, subject_label),
            )
        })
        .when(!header.summary.is_empty(), |v| {
            // The tail right-aligns and clips a text row's trailing
            // prose; an operation target keeps its own width before the status.
            if header.tail {
                return v.child(
                    div()
                        .relative()
                        .flex()
                        .flex_shrink(1.)
                        .min_w_0()
                        .overflow_hidden()
                        .justify_end()
                        .text_size(px(12.))
                        .text_color(rgb(crate::design::ZORK_UI.palette.muted))
                        .child(
                            div()
                                .flex_none()
                                .min_w(relative(1.))
                                .pl(px(12.))
                                .whitespace_nowrap()
                                .child(header.summary),
                        )
                        .child(div().absolute().left_0().top_0().bottom_0().w(px(12.)).bg(
                            gpui::linear_gradient(
                                90.,
                                gpui::linear_color_stop(rgb(0xFFFFFF), 0.),
                                gpui::linear_color_stop(rgba(0xFFFFFF00), 1.),
                            ),
                        )),
                );
            }
            v.child(
                div()
                    .min_w_0()
                    .text_size(px(11.))
                    .text_color(rgb(crate::design::ZORK_UI.palette.muted))
                    .truncate()
                    .child(header.summary),
            )
        })
        .when_some(header.status, |v, text| {
            v.child(
                div()
                    .flex_shrink_0()
                    .text_size(px(10.))
                    .text_color(rgb(status_color))
                    .child(text),
            )
        })
        .automation(AutomationRole::Button, accessible)
}

pub fn focus_for(id: String, window: &mut gpui::Window, cx: &mut gpui::App) -> gpui::FocusHandle {
    window
        .use_keyed_state(id, cx, |_, cx| cx.focus_handle().tab_stop(true))
        .read(cx)
        .clone()
}
pub fn color(entry: &Entry) -> u32 {
    if matches!(entry.state.as_str(), "failed" | "timed_out") {
        ZORK_UI.palette.danger
    } else {
        [SEND_COLOR, MODEL_COLOR, RECEIVE_COLOR][entry.lane.min(2)]
    }
}
pub fn metrics(entry: &Entry, input_label: &str, output_label: &str, cache_label: &str) -> Div {
    div()
        .line_height(px(17.))
        .when_some(entry.model.as_ref(), |v, name| {
            v.child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .line_height(px(17.))
                    .mb(px(1.))
                    .truncate()
                    .child(name.clone()),
            )
        })
        .when_some(entry.usage.as_ref(), |v, usage| {
            let input = usage["input_tokens"].as_u64().unwrap_or(0);
            let output = usage["output_tokens"].as_u64().unwrap_or(0);
            v.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x(px(8.))
                    .gap_y(px(1.))
                    .text_size(px(10.))
                    .line_height(px(15.))
                    .text_color(rgb(ZORK_UI.palette.muted))
                    .child(format!("{input_label} {input}"))
                    .child(format!("{output_label} {output}"))
                    .when_some(
                        usage["cached_input_tokens"].as_u64().filter(|_| input > 0),
                        |v, cached| v.child(format!("{cache_label} {}%", cached * 100 / input)),
                    ),
            )
        })
}
pub fn activity_color(kind: Kind, state: &str) -> u32 {
    if matches!(state, "failed" | "timed_out") {
        return ACTIVITY_ERROR_COLOR;
    }
    match kind {
        Kind::Input => ACTIVITY_RECEIVE_COLOR,
        Kind::SendMessage | Kind::SendFile | Kind::Notify => ACTIVITY_SEND_COLOR,
        Kind::Wait => ACTIVITY_WAIT_COLOR,
        Kind::Error => ACTIVITY_ERROR_COLOR,
        // Output, thinking and ordinary operations read neutral, as the history page's
        // text-tertiary icon and secondary label do.
        _ => ZORK_UI.palette.subtle,
    }
}
pub fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Input => "history_item_input",
        Kind::Output => "history_model_message",
        Kind::Thinking => "history_item_thinking",
        Kind::SendMessage => "history_send_message",
        Kind::SendFile => "history_send_file",
        Kind::Notify => "history_notify",
        Kind::Assign => "history_assign",
        Kind::Rework => "history_rework",
        Kind::Workers => "history_workers",
        Kind::Tasks => "history_tasks",
        Kind::Read => "history_read_file",
        Kind::Write => "history_write_file",
        Kind::Edit => "history_edit_file",
        Kind::Shell => "history_shell_run",
        Kind::Browser => "history_browser",
        Kind::Wait => "history_wait",
        Kind::End => "history_end",
        Kind::Cancel => "history_cancel_tool",
        Kind::Help => "history_tool_help",
        Kind::History => "history_list_events",
        Kind::ChatHistory => "history_chat_history",
        Kind::Job => "history_job",
        Kind::Error => "history_execution_error",
        Kind::Notice | Kind::UnknownTool => "history_notice",
    }
}
pub fn kind_icon(kind: Kind) -> &'static str {
    match kind {
        Kind::Input => "history/receive.svg",
        Kind::Output | Kind::Thinking => "interface/sparkles.svg",
        Kind::SendMessage => "history/send.svg",
        Kind::SendFile => "history/attachment.svg",
        Kind::Notify => "history/notify.svg",
        Kind::Assign | Kind::Rework => "history/assign.svg",
        Kind::Workers | Kind::Tasks => "history/agents.svg",
        Kind::Read => "history/file-read.svg",
        Kind::Write => "history/file-write.svg",
        Kind::Edit => "history/file-edit.svg",
        Kind::Shell => "history/terminal.svg",
        Kind::Browser => "history/browser.svg",
        Kind::Wait => "history/clock.svg",
        Kind::End => "history/end.svg",
        Kind::Cancel | Kind::Error => "history/stop.svg",
        Kind::Help => "history/help.svg",
        Kind::History | Kind::ChatHistory => "history/history.svg",
        Kind::Job => "history/job.svg",
        Kind::Notice | Kind::UnknownTool => "history/generic-tool.svg",
    }
}

#[cfg(feature = "stories")]
pub use crate::history_page::stories::Story as HistoryStory;
