//! Shared compact native controls and approved Zork identity assets.
pub use super::modal::{detail_modal, detail_modal_with_title_action, modal, ModalState};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::text_input::ComposerInput,
    design::{TextRole, FORM, INTERACTION, ZORK_UI},
};
use gpui::{div, prelude::*, px, rgb, svg, Div, Entity, FontWeight, Stateful};

pub fn icon(path: &'static str, size: f32) -> gpui::Svg {
    svg()
        .path(path)
        .size(px(size))
        .flex_shrink_0()
        .text_color(rgb(ZORK_UI.palette.muted))
}
/// Shared desktop geometry, also exercised by the offscreen visual checks.
pub const SETTINGS_COLUMN_WIDTH: f32 = 790.;
pub const SETTINGS_GUTTER: f32 = 34.;
pub const DIALOG_WIDTH: f32 = 540.;
pub const CONTROL_HEIGHT: f32 = 32.;
pub const FIELD_HEIGHT: f32 = CONTROL_HEIGHT;
pub const BUTTON_HEIGHT: f32 = CONTROL_HEIGHT;
pub const BUTTON_FOCUS_BACKGROUND: u32 = INTERACTION.primary_hover;
pub const DROPDOWN_HEIGHT: f32 = CONTROL_HEIGHT;
pub const BUTTON_RADIUS: f32 = 999.;
pub const CARD_RADIUS: f32 = 12.;
pub const COMPACT_CARD_RADIUS: f32 = 12.;
pub const FIELD_RADIUS: f32 = 10.;
pub const ICON_BUTTON_RADIUS: f32 = 10.;
pub const BUTTON_PADDING_X: f32 = 16.;
pub const MODAL_RADIUS: f32 = CARD_RADIUS;
pub const MENU_RADIUS: f32 = COMPACT_CARD_RADIUS;
/// Ordinary Codex-style popup geometry, independent of field and card radii.
pub const PLAIN_POPOVER_RADIUS: f32 = 16.;
pub const MENU_OUTSET: f32 = 4.;
pub const MENU_GAP: f32 = 6.;
pub const MENU_PADDING: f32 = 8.;
pub const FIELD_HOVER_BORDER: u32 = FORM.hover_border;
pub const FIELD_FOCUS_BORDER: u32 = FORM.focus_border;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Single,
    Multiple,
    Actions,
}

pub fn settings_content(content: impl IntoElement) -> impl IntoElement {
    div()
        .id("desktop-settings-column")
        .w_full()
        .max_w(px(SETTINGS_COLUMN_WIDTH))
        .mx_auto()
        .px(px(SETTINGS_GUTTER))
        .pt(px(30.))
        .pb_8()
        .child(content)
        .automation(AutomationRole::Status, "设置内容列")
}
pub fn text_role(text: impl Into<gpui::SharedString>, role: TextRole) -> Div {
    let (size, line_height, weight) = role.metrics();
    div()
        .text_size(px(size))
        .line_height(px(line_height))
        .font_weight(FontWeight(weight as f32))
        .text_color(rgb(match role {
            TextRole::Description | TextRole::Metadata | TextRole::Label => ZORK_UI.palette.muted,
            _ => ZORK_UI.palette.text,
        }))
        .child(text.into())
}
pub fn page_title(title: impl Into<gpui::SharedString>) -> Div {
    text_role(title, TextRole::PageTitle)
}
pub fn action_link(
    id: impl Into<gpui::ElementId>,
    text: impl Into<gpui::SharedString>,
    enabled: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .min_h(px(CONTROL_HEIGHT))
        .flex()
        .items_center()
        .text_size(px(12.))
        .text_color(rgb(if enabled {
            ZORK_UI.palette.muted
        } else {
            ZORK_UI.palette.subtle
        }))
        .when(enabled, |v| {
            v.focusable()
                .tab_stop(true)
                .cursor_pointer()
                .hover(|v| v.text_color(rgb(ZORK_UI.palette.text)).underline())
                .focus_visible(|v| v.text_color(rgb(ZORK_UI.palette.text)).underline())
                .active(|v| v.text_color(rgb(ZORK_UI.palette.text)))
        })
        .when(!enabled, |v| v.cursor_default())
        .child(text.into())
}
pub fn page_action(id: impl Into<gpui::ElementId>, text: impl Into<gpui::SharedString>) -> Action {
    adaptive_action(
        id,
        text,
        ActionStyle {
            icon: Some("icons/plus.svg"),
            icon_only: Some(false),
            ..Default::default()
        },
        ZORK_UI.palette.canvas,
    )
    .h(px(BUTTON_HEIGHT))
    .text_size(px(11.))
}

pub fn heading(
    title: impl Into<gpui::SharedString>,
    description: impl Into<gpui::SharedString>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .pb_6()
        .child(
            div()
                .text_size(px(22.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.into()),
        )
        .child(
            div()
                .text_size(px(13.))
                .line_height(px(20.))
                .text_color(rgb(ZORK_UI.palette.muted))
                .child(description.into()),
        )
}
pub fn label(text: impl Into<gpui::SharedString>) -> Div {
    text_role(text, TextRole::Label)
}
pub fn form_field(title: impl Into<gpui::SharedString>, control: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .w_full()
        .min_w_0()
        .child(label(title))
        .child(control)
}
pub fn section() -> Div {
    div()
        .flex()
        .flex_col()
        .gap_4()
        .py_5()
        .border_t(gpui::px(crate::design::BORDER_WIDTH))
        .border_color(rgb(ZORK_UI.palette.border))
}
use crate::components::widgets::controls::adaptive_action;
/// Shared actions preserve intrinsic layout and caller-provided icon/content slots.
pub use crate::components::widgets::controls::{Action, ActionStyle};

pub fn button(
    id: impl Into<gpui::ElementId>,
    text: impl Into<gpui::SharedString>,
    primary: bool,
    enabled: bool,
) -> Action {
    adaptive_action(
        id,
        text,
        ActionStyle {
            primary,
            icon_only: Some(false),
            disabled: !enabled,
            ..Default::default()
        },
        ZORK_UI.palette.canvas,
    )
}

/// A pending request stays scoped to the action that started it.
pub fn busy_button(
    id: impl Into<gpui::ElementId>,
    text: impl Into<gpui::SharedString>,
    primary: bool,
    enabled: bool,
    busy: bool,
) -> Action {
    adaptive_action(
        id,
        text,
        ActionStyle {
            primary,
            icon_only: Some(false),
            disabled: !enabled && !busy,
            busy,
            ..Default::default()
        },
        ZORK_UI.palette.canvas,
    )
}
/// Standard and compact actions share their outline with the feedback layer.
#[derive(Clone, Copy)]
pub enum IconButtonSize {
    Standard,
    Compact,
    Small,
}
impl IconButtonSize {
    pub const fn extent(self) -> f32 {
        match self {
            Self::Standard => 32.,
            Self::Compact => 28.,
            Self::Small => 24.,
        }
    }
    pub const fn radius(self) -> f32 {
        match self {
            Self::Standard => ICON_BUTTON_RADIUS,
            Self::Compact => 10.,
            Self::Small => 8.,
        }
    }
}

pub fn icon_button(id: impl Into<gpui::ElementId>, enabled: bool) -> Action {
    icon_button_sized(id, enabled, IconButtonSize::Standard)
}
pub fn icon_button_sized(
    id: impl Into<gpui::ElementId>,
    enabled: bool,
    size: IconButtonSize,
) -> Action {
    adaptive_action(
        id,
        "",
        ActionStyle {
            icon_only: Some(true),
            disabled: !enabled,
            ..Default::default()
        },
        ZORK_UI.palette.canvas,
    )
    .size(px(size.extent()))
    .gap_0()
    .px_0()
}
/// Compact text actions use the same adaptive action material.
pub fn quiet_button(
    id: impl Into<gpui::ElementId>,
    text: impl Into<gpui::SharedString>,
    enabled: bool,
    size: IconButtonSize,
) -> Action {
    adaptive_action(
        id,
        text,
        ActionStyle {
            quiet: true,
            disabled: !enabled,
            ..Default::default()
        },
        ZORK_UI.palette.canvas,
    )
    .h(px(size.extent()))
    .w_auto()
    .px_2()
    .gap_1()
}
pub fn choice(
    id: impl Into<gpui::ElementId>,
    text: impl Into<gpui::SharedString>,
    selected: bool,
    enabled: bool,
) -> Action {
    use crate::components::widgets::controls::ButtonVariant;
    adaptive_action(
        id,
        text,
        ActionStyle {
            selected,
            disabled: !enabled,
            icon_only: Some(false),
            variant: Some(if selected {
                ButtonVariant::Soft
            } else {
                ButtonVariant::Outline
            }),
            ..Default::default()
        },
        ZORK_UI.palette.canvas,
    )
    .aria_toggled(if selected {
        gpui::Toggled::True
    } else {
        gpui::Toggled::False
    })
}
pub fn field(
    id: impl Into<gpui::ElementId>,
    text: &'static str,
    input: &Entity<ComposerInput>,
    cx: &gpui::App,
) -> Div {
    field_with_error(id, text, input, None, cx)
}
/// Standard input surface, also usable in an inline title without an extra label.
/// Callers attach automation after adding their own keyboard interactions.
pub fn input_control(
    id: impl Into<gpui::ElementId>,
    input: &Entity<ComposerInput>,
    invalid: bool,
    _cx: &gpui::App,
) -> crate::components::widgets::controls::Field {
    crate::components::widgets::controls::adaptive_input(id, input, invalid, ZORK_UI.palette.canvas)
}

pub fn field_with_error(
    id: impl Into<gpui::ElementId>,
    text: &'static str,
    input: &Entity<ComposerInput>,
    error: Option<String>,
    cx: &gpui::App,
) -> Div {
    let id: gpui::ElementId = id.into();
    let error_id = format!("{id:?}-error");
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_2()
        .child(label(text))
        .child(
            input_control(id, input, error.is_some(), cx)
                .automation(AutomationRole::TextInput, text),
        )
        .when_some(error, |v, error| {
            v.child(
                div()
                    .id(error_id)
                    .mt(px(-3.))
                    .flex()
                    .items_start()
                    .gap(px(5.))
                    .text_size(px(11.))
                    .line_height(px(17.))
                    .text_color(rgb(ZORK_UI.palette.danger))
                    .child(
                        div()
                            .w(px(12.))
                            .h(px(17.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .child(
                                icon("icons/attention.svg", 12.)
                                    .text_color(rgb(ZORK_UI.palette.danger)),
                            ),
                    )
                    .child(error.clone())
                    .automation(AutomationRole::Status, error),
            )
        })
}
pub fn feedback(message: String) -> Div {
    status_notice(message, NoticeKind::Info)
}
/// A single-selection field with a window-clamped overlay; choices never reflow the form.
pub fn dropdown<V: 'static>(
    id: impl Into<gpui::SharedString>,
    label: String,
    options: Vec<(String, String, bool)>,
    open: bool,
    enabled: bool,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<V>,
    set_open: impl Fn(&mut V, bool, &mut gpui::Context<V>) + 'static,
    choose: impl Fn(&mut V, usize, &mut gpui::Context<V>) + 'static,
) -> gpui::AnyElement {
    dropdown_with_icons(
        id,
        label,
        options,
        open,
        enabled,
        None,
        vec![],
        window,
        cx,
        set_open,
        choose,
    )
}
pub fn dropdown_with_icons<V: 'static>(
    id: impl Into<gpui::SharedString>,
    label: String,
    options: Vec<(String, String, bool)>,
    open: bool,
    enabled: bool,
    leading: Option<&'static str>,
    option_icons: Vec<Option<&'static str>>,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<V>,
    set_open: impl Fn(&mut V, bool, &mut gpui::Context<V>) + 'static,
    choose: impl Fn(&mut V, usize, &mut gpui::Context<V>) + 'static,
) -> gpui::AnyElement {
    menu_dropdown(
        id,
        label,
        options,
        open,
        enabled,
        crate::controls::Selection::Single,
        false,
        None,
        leading,
        option_icons,
        window,
        cx,
        set_open,
        choose,
    )
}
/// Text and chevron sized to the label, for a row that already has the composer's insets.
pub fn quiet_dropdown<V: 'static>(
    id: impl Into<gpui::SharedString>,
    label: String,
    options: Vec<(String, String, bool)>,
    open: bool,
    enabled: bool,
    max_width: f32,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<V>,
    set_open: impl Fn(&mut V, bool, &mut gpui::Context<V>) + 'static,
    choose: impl Fn(&mut V, usize, &mut gpui::Context<V>) + 'static,
) -> gpui::AnyElement {
    menu_dropdown(
        id,
        label,
        options,
        open,
        enabled,
        crate::controls::Selection::Single,
        true,
        Some(max_width),
        None,
        vec![],
        window,
        cx,
        set_open,
        choose,
    )
}
/// A choice field may keep its menu open for multiple selections.
pub fn dropdown_with_selection<V: 'static>(
    id: impl Into<gpui::SharedString>,
    label: String,
    options: Vec<(String, String, bool)>,
    open: bool,
    enabled: bool,
    selection: crate::controls::Selection,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<V>,
    set_open: impl Fn(&mut V, bool, &mut gpui::Context<V>) + 'static,
    choose: impl Fn(&mut V, usize, &mut gpui::Context<V>) + 'static,
) -> gpui::AnyElement {
    menu_dropdown(
        id,
        label,
        options,
        open,
        enabled,
        selection,
        false,
        None,
        None,
        vec![],
        window,
        cx,
        set_open,
        choose,
    )
}
fn menu_dropdown<V: 'static>(
    id: impl Into<gpui::SharedString>,
    label: String,
    options: Vec<(String, String, bool)>,
    open: bool,
    enabled: bool,
    selection: crate::controls::Selection,
    quiet: bool,
    max_width: Option<f32>,
    leading: Option<&'static str>,
    option_icons: Vec<Option<&'static str>>,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<V>,
    set_open: impl Fn(&mut V, bool, &mut gpui::Context<V>) + 'static,
    choose: impl Fn(&mut V, usize, &mut gpui::Context<V>) + 'static,
) -> gpui::AnyElement {
    use crate::components::widgets::controls::{adaptive_action, ActionStyle};
    use gpui::{KeyDownEvent, Role};
    use std::rc::Rc;

    let id: gpui::SharedString = id.into();
    let measured = window.use_keyed_state(format!("{id}-bounds"), cx, |_, _| {
        gpui::Bounds::<gpui::Pixels>::default()
    });
    let bounds = *measured.read(cx);
    let width = bounds.size.width.as_f32();
    let control_height = if quiet { 24. } else { DROPDOWN_HEIGHT };
    let control_width = if quiet {
        crate::components::widgets::overlay::measure_label(&label, 12., window) + 28.
    } else {
        width.max(32.)
    }
    .min(max_width.unwrap_or(f32::MAX).max(48.));
    let owner = cx.entity().downgrade();
    let choose = Rc::new(choose);
    let set_open = Rc::new(set_open);
    let handles: Vec<_> = options
        .iter()
        .enumerate()
        .map(|(index, _)| {
            crate::components::widgets::controls::action_focus(
                format!("{id}-option-{index}"),
                window,
                cx,
            )
        })
        .collect();
    let active: Vec<usize> = (0..options.len()).collect();
    let entry = options
        .iter()
        .position(|(_, _, checked)| *checked)
        .unwrap_or(0);
    let focus = handles.get(entry).cloned().unwrap_or_else(|| {
        crate::components::widgets::controls::action_focus(format!("{id}-menu"), window, cx)
    });
    let trigger_focus =
        crate::components::widgets::controls::action_focus(format!("{id}-trigger"), window, cx);
    if open
        && !trigger_focus.is_focused(window)
        && !handles.iter().any(|handle| handle.is_focused(window))
    {
        let blur_owner = owner.clone();
        let blur_close = set_open.clone();
        cx.defer(move |app| {
            let action = blur_close.clone();
            let _ = blur_owner.update(app, |view, cx| action(view, false, cx));
        });
    }
    let pointer_owner = owner.clone();
    let pointer_open = set_open.clone();
    let pointer_focus = focus.clone();
    let pointer_trigger_focus = trigger_focus.clone();
    let trigger = adaptive_action(
        id.clone(),
        label.clone(),
        ActionStyle {
            field: !quiet,
            quiet,
            opens_panel: true,
            disabled: !enabled || options.is_empty(),
            icon: leading,
            trailing: Some("icons/chevron-down.svg"),
            ..Default::default()
        },
        ZORK_UI.palette.canvas,
    )
    .w(px(control_width))
    .h(px(control_height))
    .track_focus(&trigger_focus)
    .on_click(move |event, window, app| {
        if !matches!(event, gpui::ClickEvent::Keyboard(_)) && enabled {
            let action = pointer_open.clone();
            let _ = pointer_owner.update(app, |view, cx| action(view, !open, cx));
            window.focus(
                if open {
                    &pointer_trigger_focus
                } else {
                    &pointer_focus
                },
                app,
            );
        }
    })
    .automation_enabled(
        enabled && !options.is_empty(),
        AutomationRole::Button,
        label.clone(),
    );
    let choice_focus = trigger_focus.clone();
    let panel_id = id.clone();
    let panel_padding = 6.;
    let panel_height = 320.;
    let mut rows = div()
        .id(format!("{panel_id}-rows"))
        .w_full()
        .max_h(px(
            panel_height - 2. * (panel_padding + crate::design::BORDER_WIDTH)
        ))
        .overflow_y_scroll()
        .flex()
        .flex_col();
    for (index, (key, text, checked)) in options.iter().enumerate() {
        let callback = choose.clone();
        let close = set_open.clone();
        let owner = owner.clone();
        let focus = handles[index].clone();
        let trigger_focus = choice_focus.clone();
        let text: gpui::SharedString = text.clone().into();
        let automation_label = text.to_string();
        let checked = *checked;
        let icon = option_icons.get(index).copied().flatten();
        let role = match selection {
            crate::controls::Selection::Single => Role::RadioButton,
            crate::controls::Selection::Multiple => Role::CheckBox,
            crate::controls::Selection::Actions => Role::Button,
        };
        let item = gpui_base::Button::new(format!("{key}-control"))
            .disabled(!enabled)
            .selected(checked)
            .track_focus(&focus)
            .tab_stop(enabled && index == entry)
            .role(role)
            .accessibility_label(text.clone())
            .on_click(move |_, window, app| {
                let callback = callback.clone();
                let close = close.clone();
                let _ = owner.update(app, |view, cx| {
                    callback(view, index, cx);
                    if selection == crate::controls::Selection::Single {
                        close(view, false, cx);
                    }
                });
                if selection == crate::controls::Selection::Single {
                    window.focus(&trigger_focus, app);
                }
            })
            .w_full()
            .min_h(px(32.))
            .rounded(px(10.))
            .when(checked, |v| v.bg(rgb(ZORK_UI.palette.selected)))
            .hover(|v| v.bg(rgb(INTERACTION.neutral_hover)))
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .text_size(px(12.))
            .when_some(icon, |v, path| v.child(self::icon(path, 14.)))
            .child(div().flex_1().min_w_0().child(text))
            .when(checked, |v| v.child(self::icon("icons/check.svg", 12.)));
        rows = rows.child(
            div()
                .id(key.clone())
                .w_full()
                .child(item)
                .automation_enabled(enabled, AutomationRole::Option, automation_label),
        );
    }
    let mut panel = div()
        .id(format!("{panel_id}-menu"))
        .occlude()
        .w(px(control_width.max(160.)))
        .max_h(px(panel_height))
        .rounded(px(PLAIN_POPOVER_RADIUS))
        .bg(rgb(ZORK_UI.palette.canvas))
        .border(px(crate::design::BORDER_WIDTH))
        .border_color(rgb(crate::design::UI_OUTLINE))
        .p(px(panel_padding))
        .flex()
        .flex_col()
        .child(crate::components::smooth::rounded_viewport(
            format!("{panel_id}-viewport"),
            PLAIN_POPOVER_RADIUS - panel_padding,
            rows,
        ));
    let focus_for_keys = handles.clone();
    let outside_owner = owner.clone();
    let outside_close = set_open.clone();
    let outside_focus = trigger_focus.clone();
    panel = panel
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            if active.is_empty() {
                return;
            }
            let current = active
                .iter()
                .position(|index| focus_for_keys[*index].is_focused(window))
                .unwrap_or(0);
            let next = match event.keystroke.key.as_str() {
                "up" => (current + active.len() - 1) % active.len(),
                "down" => (current + 1) % active.len(),
                "home" => 0,
                "end" => active.len() - 1,
                _ => return,
            };
            window.focus(&focus_for_keys[active[next]], cx);
            window.prevent_default();
            cx.stop_propagation();
        })
        .on_mouse_down_out(move |_, window, app| {
            let action = outside_close.clone();
            let _ = outside_owner.update(app, |view, cx| action(view, false, cx));
            window.focus(&outside_focus, app);
        });
    let open_owner = owner.clone();
    let open_action = set_open.clone();
    let mut select = gpui_base::Select::new(format!("{id}-select"))
        .open(open)
        .disabled(!enabled || options.is_empty())
        .focus_handle(&trigger_focus)
        .content_focus_handle(&focus)
        .accessibility_label(label.clone())
        .on_open_change(move |next, _, app| {
            let action = open_action.clone();
            let _ = open_owner.update(app, |view, cx| action(view, next, cx));
        })
        .w_full()
        .h_full()
        .child(trigger);
    if open {
        select = select.child(
            gpui::deferred(
                gpui_base::Positioner::side(bounds)
                    .placement(gpui_base::Placement::Bottom)
                    .align(gpui_base::Align::Start)
                    .offset(px(MENU_GAP))
                    .margin(px(8.))
                    .child(panel.automation(AutomationRole::ScrollArea, label.clone())),
            )
            .with_priority(350),
        );
    }
    div()
        .relative()
        .w(px(if quiet { control_width } else { width.max(32.) }))
        .when(!quiet, |v| v.w_full())
        .flex_shrink_0()
        .h(px(control_height))
        .child(
            gpui::canvas(
                move |bounds, _, cx| {
                    measured.update(cx, |current, cx| {
                        if *current != bounds {
                            *current = bounds;
                            cx.notify();
                        }
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
        .child(select)
        .into_any_element()
}

pub fn provider_path(provider: &str) -> &'static str {
    match provider {
        "openai" => "providers/openai.svg",
        "anthropic" => "providers/anthropic.svg",
        "github-copilot" => "providers/githubcopilot.svg",
        "kimi" | "kimi-coding" => "providers/kimi.svg",
        "openrouter" => "providers/openrouter.svg",
        "opencode-go" => "providers/opencode.svg",
        "xai" => "providers/xai.svg",
        _ => "providers/compatible.svg",
    }
}
pub fn provider_icon(provider: &str, size: f32) -> gpui::Div {
    let path = provider_path(provider);
    gpui::div()
        .size(px(size))
        .flex_shrink_0()
        .child(gpui::img(path).size(px(size)))
}

/// Controlled switch with the same compact geometry in native and Web renderers.
pub fn switch<V: 'static>(
    id: impl Into<gpui::SharedString>,
    label: impl Into<gpui::SharedString>,
    checked: bool,
    enabled: bool,
    focus: &gpui::FocusHandle,
    cx: &gpui::Context<V>,
    on_change: impl Fn(&mut V, bool, &mut gpui::Context<V>) + 'static,
) -> impl IntoElement {
    let label = label.into();
    crate::components::widgets::controls::deferred_toggle(
        id,
        checked,
        enabled,
        ZORK_UI.palette.canvas,
        focus.clone(),
        cx.listener(move |view, checked: &bool, _, cx| on_change(view, *checked, cx)),
    )
    .automation_enabled(
        enabled,
        AutomationRole::Option,
        format!("{}：{}", label, if checked { "开启" } else { "关闭" }),
    )
}

#[derive(Clone, Copy)]
pub enum NoticeKind {
    Info,
    Success,
    Error,
    Warning,
    Loading,
}
pub fn status_notice(message: String, kind: NoticeKind) -> Div {
    use crate::components::widgets::primitives::{feedback, surface};
    let (_, fill) = feedback::colors(kind);
    div().w_full().child(
        surface("status-notice-surface", FIELD_RADIUS, fill, false)
            .w_full()
            .px_3()
            .py_2()
            .child(feedback::notice_content(
                "status-notice",
                message,
                kind,
                None,
            )),
    )
}
