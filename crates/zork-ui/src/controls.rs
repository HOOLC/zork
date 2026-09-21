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
pub use zork_liquid::tokens::{
    BUTTON_RADIUS, CARD_RADIUS, COMPACT_CARD_RADIUS, FIELD_RADIUS, ICON_BUTTON_RADIUS,
};
pub const BUTTON_PADDING_X: f32 = 16.;
pub const MODAL_RADIUS: f32 = CARD_RADIUS;
pub const MENU_RADIUS: f32 = COMPACT_CARD_RADIUS;
pub const MENU_OUTSET: f32 = 4.;
pub const MENU_GAP: f32 = 6.;
pub const MENU_PADDING: f32 = 8.;
pub const FIELD_HOVER_BORDER: u32 = FORM.hover_border;
pub const FIELD_FOCUS_BORDER: u32 = FORM.focus_border;

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
use crate::components::liquid::controls::adaptive_action;
/// Shared actions preserve intrinsic layout and caller-provided icon/content slots.
pub use crate::components::liquid::controls::{Action, ActionStyle};

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
    use crate::components::liquid::controls::ButtonVariant;
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
) -> crate::components::liquid::controls::Field {
    crate::components::liquid::controls::adaptive_input(id, input, invalid, ZORK_UI.palette.canvas)
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
pub fn avatar(id: &str, name: &str, size: f32) -> Div {
    let hash = id
        .bytes()
        .fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize));
    let (fill, ink) =
        crate::design::LEADER_AVATAR_COLORS[hash % crate::design::LEADER_AVATAR_COLORS.len()];
    let initial = name
        .trim()
        .chars()
        .next()
        .unwrap_or('L')
        .to_uppercase()
        .collect::<String>();
    div()
        .size(px(size))
        .flex_shrink_0()
        .rounded_full()
        .bg(rgb(fill))
        .text_color(rgb(ink))
        .text_size(px(size * 0.38))
        .font_weight(FontWeight::SEMIBOLD)
        .flex()
        .items_center()
        .justify_center()
        .child(initial)
}

/// The stored key is shared by creation, settings and conversation views.
pub const AGENT_AVATARS: [(&str, &str, &str); 12] = [
    ("cat", "小猫", "avatars/cat.svg"),
    ("bunny", "小兔", "avatars/bunny.svg"),
    ("bear", "小熊", "avatars/bear.svg"),
    ("fox", "狐狸", "avatars/fox.svg"),
    ("panda", "熊猫", "avatars/panda.svg"),
    ("chick", "小鸡", "avatars/chick.svg"),
    ("dog", "小狗", "avatars/dog.svg"),
    ("owl", "猫头鹰", "avatars/owl.svg"),
    ("koala", "考拉", "avatars/koala.svg"),
    ("penguin", "企鹅", "avatars/penguin.svg"),
    ("deer", "小鹿", "avatars/deer.svg"),
    ("octopus", "章鱼", "avatars/octopus.svg"),
];
/// Portrait only, for members sitting directly on the composer material.
pub fn agent_portrait(avatar: Option<&str>, size: f32) -> Div {
    let (key, _, _) = AGENT_AVATARS
        .iter()
        .find(|(key, _, _)| Some(*key) == avatar)
        .unwrap_or(&AGENT_AVATARS[0]);
    div()
        .size(px(size))
        .flex_shrink_0()
        .child(gpui::img(format!("avatars/portraits/{key}.svg")).size(px(size)))
}

pub fn agent_avatar(avatar: Option<&str>, size: f32) -> Div {
    agent_portrait(avatar, size)
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
    use crate::components::liquid::{
        overlay::{Choice, Placement, Popover, Selection, Trigger},
        Material,
    };
    use std::{cell::RefCell, rc::Rc};
    let id = id.into();
    let state = window
        .use_keyed_state(format!("{id}-popover"), cx, |_, cx| {
            Rc::new(RefCell::new(Popover::new(cx)))
        })
        .read(cx)
        .clone();
    let measured = window.use_keyed_state(format!("{id}-width"), cx, |_, _| 0f32);
    let width = *measured.read(cx);
    let choices = options
        .into_iter()
        .map(|(id, label, checked)| Choice {
            id,
            label: label.into(),
            checked: Some(checked),
            disabled: !enabled,
        })
        .collect();
    let popover = state.borrow_mut().render_with_icons(
        id,
        label,
        choices,
        Selection::Single,
        Trigger::Field,
        open,
        enabled,
        Placement::Window {
            width: width.max(32.),
        },
        Material::ordinary(),
        leading,
        option_icons,
        window,
        cx,
        move |v, open, _, cx| set_open(v, open, cx),
        move |v, index, _, cx| choose(v, index, cx),
    );
    div()
        .relative()
        .w_full()
        .h(px(DROPDOWN_HEIGHT))
        .child(
            gpui::canvas(
                move |bounds, _, cx| {
                    measured.update(cx, |width, cx| {
                        let next = bounds.size.width.as_f32();
                        if (*width - next).abs() > 0.1 {
                            *width = next;
                            cx.notify();
                        }
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
        .child(popover)
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

#[derive(Clone)]
pub struct OpenAgent {
    pub id: String,
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
    crate::components::liquid::controls::deferred_toggle(
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

pub fn avatar_picker<V: 'static>(
    prefix: impl Into<gpui::SharedString>,
    selected: &str,
    enabled: bool,
    cx: &gpui::Context<V>,
    choose: impl Fn(&mut V, &'static str, &mut gpui::Context<V>) + 'static,
) -> Div {
    use crate::components::liquid::primitives::data::{portrait_choices, PortraitOption};
    let prefix = prefix.into();
    let selected = AGENT_AVATARS
        .iter()
        .position(|(key, _, _)| *key == selected)
        .unwrap_or(0);
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(label("头像"))
        .child(portrait_choices(
            prefix.clone(),
            AGENT_AVATARS
                .iter()
                .map(|(key, title, _)| PortraitOption {
                    id: format!("{prefix}-{key}"),
                    portrait: key,
                    label: (*title).into(),
                })
                .collect(),
            selected,
            enabled,
            6,
            32.,
            24.,
            cx.listener(move |v, index: &usize, _, cx| choose(v, AGENT_AVATARS[*index].0, cx)),
        ))
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
    use crate::components::liquid::primitives::{feedback, surface};
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
