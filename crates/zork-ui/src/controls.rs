//! Shared compact native controls and approved Zork identity assets.
pub use super::modal::{
    detail_modal, detail_modal_sized, detail_modal_with_title_action, modal, modal_sized, ModalState,
};
/// Anchored popover for menus that app crates build from controls.
pub use gpui_component::popover::Popover;
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::text_input::ComposerInput,
    design::{TextRole, FORM, INTERACTION, ZORK_UI},
};
use crate::motion::MotionExt;
use gpui::{div, prelude::*, px, rgb, svg, Div, Entity, FontWeight, Stateful};

pub fn icon(path: &'static str, size: f32) -> gpui::Svg {
    svg()
        .path(path)
        .size(px(size))
        .flex_shrink_0()
        .text_color(rgb(ZORK_UI.palette.muted))
}
/// Shared desktop geometry, also exercised by the offscreen visual checks.
/// Settings and detail content: a centred reading column, min(760, 100% − 64).
pub const SETTINGS_COLUMN_WIDTH: f32 = 760.;
pub const SETTINGS_GUTTER: f32 = 32.;
/// Dialog widths by content: a short confirmation, a form, rich details.
pub const DIALOG_CONFIRM_WIDTH: f32 = 400.;
pub const DIALOG_FORM_WIDTH: f32 = 480.;
/// Multi-step forms whose choices are laid out as cards.
pub const DIALOG_STEP_WIDTH: f32 = 520.;
pub const DIALOG_RICH_WIDTH: f32 = 560.;
pub const CONTROL_HEIGHT: f32 = 32.;
pub const FIELD_HEIGHT: f32 = CONTROL_HEIGHT;
pub const BUTTON_HEIGHT: f32 = CONTROL_HEIGHT;
#[allow(non_snake_case)]
pub fn BUTTON_FOCUS_BACKGROUND() -> u32 {
    INTERACTION.primary_hover
}
pub const DROPDOWN_HEIGHT: f32 = CONTROL_HEIGHT;
pub const BUTTON_RADIUS: f32 = 999.;
pub const CARD_RADIUS: f32 = crate::design::RADIUS.container;
pub const COMPACT_CARD_RADIUS: f32 = crate::design::RADIUS.block;
/// A capsule at the control height; taller multi-line fields keep a block corner.
pub const FIELD_RADIUS: f32 = crate::design::RADIUS.control;
pub const ICON_BUTTON_RADIUS: f32 = crate::design::RADIUS.control;
pub const BUTTON_PADDING_X: f32 = 16.;
pub const MODAL_RADIUS: f32 = crate::design::RADIUS.surface;
/// Confirmations and forms read as containers; rich dialogs keep the surface corner.
pub fn dialog_radius(width: f32) -> f32 {
    if width >= DIALOG_RICH_WIDTH {
        MODAL_RADIUS
    } else {
        crate::design::RADIUS.container
    }
}
pub const MENU_RADIUS: f32 = PLAIN_POPOVER_RADIUS;
/// Menus and popovers: MENU_PADDING inside keeps capsule items concentric.
pub const PLAIN_POPOVER_RADIUS: f32 = crate::design::RADIUS.container;
pub const MENU_OUTSET: f32 = 4.;
pub const MENU_GAP: f32 = 6.;
pub const MENU_PADDING: f32 = 8.;
#[allow(non_snake_case)]
pub fn FIELD_HOVER_BORDER() -> u32 {
    FORM.hover_border
}
#[allow(non_snake_case)]
pub fn FIELD_FOCUS_BORDER() -> u32 {
    FORM.focus_border
}
/// Optical padding: an icon carries its own inset, so its side of a button is narrower.
pub const ICON_SIDE_PADDING_X: f32 = BUTTON_PADDING_X - 4.;
/// The shared keyboard-focus ring, drawn outside the control outline.
pub fn focus_ring() -> Vec<gpui::BoxShadow> {
    let [r, g, b] = [16u32, 8, 0].map(|shift| ((crate::design::INTERACTION.focus_ring >> shift) & 0xFF) as f32 / 255.);
    vec![gpui::BoxShadow {
        color: gpui::Rgba { r, g, b, a: crate::design::FOCUS_RING_ALPHA }.into(),
        offset: gpui::point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(2.),
        inset: false,
    }]
}

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
        .max_w(px(SETTINGS_COLUMN_WIDTH + 2. * SETTINGS_GUTTER))
        .mx_auto()
        .px(px(SETTINGS_GUTTER))
        .pt(px(32.))
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
    .text_size(px(12.))
}

pub fn heading(
    title: impl Into<gpui::SharedString>,
    description: impl Into<gpui::SharedString>,
) -> Div {
    // An empty description adds no line.
    let description: gpui::SharedString = description.into();
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
        .when(!description.is_empty(), |v| v.child(
            div()
                .text_size(px(13.))
                .line_height(px(20.))
                .text_color(rgb(ZORK_UI.palette.muted))
                .child(description),
        ))
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
            Self::Compact => ICON_BUTTON_RADIUS,
            Self::Small => ICON_BUTTON_RADIUS,
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
            radius: Some(size.radius()),
            disabled: !enabled,
            ..Default::default()
        },
        ZORK_UI.palette.canvas,
    )
    .size(px(size.extent()))
    .min_h_0()
    .py_0()
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
    .min_h_0()
    .py_0()
    .w_auto()
    .px(px(12.))
    .gap_1()
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
            input_control(id, input, error.is_some(), cx).automation_enabled(
                !input.read(cx).is_disabled(),
                AutomationRole::TextInput,
                text,
            ),
        )
        .when_some(error, |v, error| {
            v.child(
                div()
                    .id(error_id)
                    .mt(px(-3.))
                    .flex()
                    .items_start()
                    .gap(px(5.))
                    .text_size(px(12.))
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
    crate::components::choice_menu::render(
        id,
        label,
        options,
        open,
        enabled,
        selection,
        quiet,
        max_width,
        leading,
        option_icons,
        window,
        cx,
        set_open,
        choose,
    )
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
        // Embedded provider marks are monochrome currentColor SVGs. Drawing them
        // directly keeps the first presentation independent of image-loader wakeups.
        .child(svg().path(path).size(px(size)).text_color(rgb(ZORK_UI.palette.text)))
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
    // A new notice drops in from its region's top edge; the same message
    // re-rendering keeps its animation state and does not replay.
    let enter = gpui::SharedString::from(format!("status-notice-enter-{message}"));
    div().w_full().child(
        // A banner is a container: its corner and insets grow together.
        surface("status-notice-surface", crate::design::RADIUS.container, fill, false)
            .w_full()
            .pl(px(18.))
            .pr(px(12.))
            .py(px(12.))
            .child(feedback::notice_content(
                "status-notice",
                message,
                kind,
                None,
            ))
            .appear(
                enter,
                crate::motion::SURFACE,
                -crate::motion::NOTICE_OFFSET,
            ),
    )
}
