//! Shared history record presentation; event collection and paging belong to the host.
use crate::history::activity::Kind;
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::CUE_UI,
    history::Entry,
};
use gpui::{div, prelude::*, px, rgb, Context, Div, FontWeight};

// Restrained semantic accents, shared by rows, destinations and the timeline.
pub const SEND_COLOR: u32 = 0x536779;
pub const RECEIVE_COLOR: u32 = 0x5A6D62;
pub const MODEL_COLOR: u32 = 0x92969D;

/// Shared two-line history geometry. The host resolves labels and destinations;
/// no network, identity lookup or argument parsing occurs during row rendering.
pub struct ActivityHeader {
    pub icon: &'static str,
    pub color: u32,
    pub action: String,
    pub connector: Option<String>,
    pub subject: Option<String>,
    pub clickable_subject: bool,
    pub summary: String,
    pub time: String,
    pub status: Option<String>,
    pub nested: bool,
    pub group: bool,
}

pub fn activity_header<V: 'static>(
    id: impl Into<gpui::ElementId>,
    header: ActivityHeader,
    cx: &Context<V>,
    open: impl Fn(&mut V, &mut gpui::Window, &mut Context<V>) + 'static,
    navigate: impl Fn(&mut V, &mut gpui::Window, &mut Context<V>) + 'static,
) -> impl IntoElement {
    activity_header_sources(id, header, (None, None), cx, open, navigate)
}

pub fn activity_header_sources<V: 'static>(
    id: impl Into<gpui::ElementId>,
    header: ActivityHeader,
    sources: (
        Option<crate::components::liquid::overlay::SourceBinding>,
        Option<crate::components::liquid::overlay::SourceBinding>,
    ),
    cx: &Context<V>,
    open: impl Fn(&mut V, &mut gpui::Window, &mut Context<V>) + 'static,
    navigate: impl Fn(&mut V, &mut gpui::Window, &mut Context<V>) + 'static,
) -> impl IntoElement {
    let face = header.action.clone();
    let icon = header.icon;
    let open = std::rc::Rc::new(open);
    let keyboard_open = open.clone();
    let navigate = std::rc::Rc::new(navigate);
    let keyboard_navigate = navigate.clone();
    let id = id.into();
    let accessible = format!(
        "{} {} {} {} {} {}",
        header.action,
        header.connector.as_deref().unwrap_or_default(),
        header.subject.as_deref().unwrap_or_default(),
        header.summary,
        header.status.as_deref().unwrap_or_default(),
        header.time,
    );
    let subject_id = format!("history-target-{id:?}");
    let subject_label = header.subject.clone().unwrap_or_default();
    div()
        .id(id)
        .focusable()
        .tab_stop(true)
        .flex()
        .items_start()
        .gap(px(8.))
        .px(px(8.))
        .pl(px(if header.nested { 32. } else { 8. }))
        .py(px(3.))
        .min_w_0()
        .cursor_pointer()
        .hover(|v| v.bg(rgb(CUE_UI.palette.sidebar_hover)))
        .focus_visible(|v| v.bg(rgb(CUE_UI.palette.sidebar_hover)))
        .on_click(cx.listener(move |v, _, window, cx| open(v, window, cx)))
        .on_key_down(
            cx.listener(move |v, event: &gpui::KeyDownEvent, window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    keyboard_open(v, window, cx);
                }
            }),
        )
        .child(
            div()
                .w(px(16.))
                .h(px(20.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .child(
                    crate::controls::icon(header.icon, 14.).text_color(rgb(CUE_UI.palette.muted)),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .h(px(20.))
                        .min_w_0()
                        .child(
                            div()
                                .min_w_0()
                                .text_size(px(if header.group { 11. } else { 12. }))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(header.color))
                                .truncate()
                                .child(header.action),
                        )
                        .when_some(header.connector, |v, text| {
                            v.child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(rgb(CUE_UI.palette.muted))
                                    .child(text),
                            )
                        })
                        .when_some(header.subject, |v, text| {
                            let subject = div()
                                .min_w_0()
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(if header.clickable_subject {
                                    SEND_COLOR
                                } else {
                                    CUE_UI.palette.text
                                }))
                                .truncate()
                                .child(text);
                            if !header.clickable_subject {
                                // Plain source labels have no fake disabled-button node or hitbox.
                                return v.child(subject.into_any_element());
                            }
                            v.child(
                                subject
                                    .id(subject_id)
                                    .focusable()
                                    .tab_stop(true)
                                    .cursor_pointer()
                                    .hover(|v| v.underline())
                                    .focus_visible(|v| v.underline())
                                    .on_click(cx.listener(move |v, _, window, cx| {
                                        cx.stop_propagation();
                                        navigate(v, window, cx);
                                    }))
                                    .on_key_down(cx.listener(
                                        move |v, event: &gpui::KeyDownEvent, window, cx| {
                                            if matches!(
                                                event.keystroke.key.as_str(),
                                                "enter" | "space"
                                            ) {
                                                cx.stop_propagation();
                                                keyboard_navigate(v, window, cx);
                                            }
                                        },
                                    ))
                                    .map(|subject| match sources.1 {
                                        Some(source) => source
                                            .bind(
                                                subject,
                                                subject_label.clone(),
                                                crate::controls::ActionStyle {
                                                    quiet: true,
                                                    ..Default::default()
                                                },
                                            )
                                            .automation(AutomationRole::Button, subject_label)
                                            .into_any_element(),
                                        None => subject
                                            .automation(AutomationRole::Button, subject_label)
                                            .into_any_element(),
                                    }),
                            )
                        })
                        .child(div().flex_1())
                        .when_some(header.status, |v, text| {
                            v.child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(10.))
                                    .text_color(rgb(header.color))
                                    .child(text),
                            )
                        })
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(11.))
                                .text_color(rgb(CUE_UI.palette.muted))
                                .child(header.time),
                        ),
                )
                .when(!header.summary.is_empty(), |v| {
                    v.child(
                        div()
                            .h(px(20.))
                            .line_height(px(20.))
                            .text_size(px(12.))
                            .text_color(rgb(CUE_UI.palette.muted))
                            .truncate()
                            .child(header.summary),
                    )
                }),
        )
        .map(|header| match sources.0 {
            Some(source) => source
                .bind(
                    header,
                    face,
                    crate::controls::ActionStyle {
                        quiet: true,
                        icon: Some(icon),
                        ..Default::default()
                    },
                )
                .automation(AutomationRole::Button, accessible)
                .into_any_element(),
            None => header
                .automation(AutomationRole::Button, accessible)
                .into_any_element(),
        })
}
pub fn color(entry: &Entry) -> u32 {
    if matches!(entry.state.as_str(), "failed" | "timed_out") {
        CUE_UI.palette.danger
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
                    .text_color(rgb(CUE_UI.palette.muted))
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
        return CUE_UI.palette.danger;
    }
    match kind {
        Kind::Received => RECEIVE_COLOR,
        Kind::Model => MODEL_COLOR,
        Kind::SendMessage | Kind::SendFile | Kind::Notify => SEND_COLOR,
        Kind::Assign | Kind::Rework | Kind::Wait | Kind::Cancel => CUE_UI.palette.muted,
        Kind::Error => CUE_UI.palette.danger,
        _ => CUE_UI.palette.text,
    }
}
pub fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Received => "history_receive_message",
        Kind::Model => "history_model_message",
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
        Kind::Received => "history/receive.svg",
        Kind::Model => "cue/sparkles.svg",
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
