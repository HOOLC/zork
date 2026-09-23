//! Reusable workbench composition: chrome, rails, sections, controls and previews.
//! Stories supply labels, examples and input callbacks, never a second visual system.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
    design::{FORM, TextRole, ZORK_UI},
};
use gpui::{
    div, prelude::*, px, rgb, Context, Div, ElementId, Font, SharedString, Stateful, Window,
};

#[allow(non_snake_case)]
pub fn CANVAS() -> u32 {
    ZORK_UI.palette.canvas
}

pub const GAP: f32 = 24.;
pub const PREVIEW_PADDING: f32 = 18.;

pub enum GridMode {
    Focused,
    Standard,
    Dense,
}
#[derive(Clone, Copy, PartialEq)]
pub struct Layout {
    pub navigation: Option<f32>,
    pub inspector: Option<f32>,
    pub gutter: f32,
    pub content_width: f32,
    pub item_width: f32,
}
impl Layout {
    pub fn new(width: f32, navigation: bool, mode: GridMode) -> Self {
        let navigation = (navigation && width >= 1000.).then_some(200.);
        let inspector = (width >= 1320.).then_some(240.);
        let gutter = if width < 480. { 16. } else { 24. };
        let dense = matches!(mode, GridMode::Dense);
        let content_width =
            (width - navigation.unwrap_or(0.) - inspector.unwrap_or(0.) - gutter * 2.)
                .min(if dense { 1600. } else { 1120. })
                .max(240.);
        let columns = if matches!(mode, GridMode::Focused) {
            1.
        } else if dense && content_width > 1300. {
            4.
        } else if content_width >= 880. {
            2.
        } else {
            1.
        };
        Self {
            navigation,
            inspector,
            gutter,
            content_width,
            item_width: ((content_width - GAP * (columns - 1.)) / columns).max(240.),
        }
    }
}
pub fn font() -> Font {
    gpui::font("Inter Variable")
}

pub fn specimen(
    id: impl Into<ElementId>,
    width: f32,
    title: Option<(&str, &str)>,
) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(width))
        .flex()
        .flex_col()
        .gap_3()
        .when_some(title, |v, (title, help)| {
            v.child(section_title(title.to_owned(), help.to_owned()))
        })
}

pub fn column(gap: f32) -> Div {
    div().flex().flex_col().gap(px(gap))
}
pub fn row(gap: f32) -> Div {
    div().flex().items_center().gap(px(gap))
}
pub fn wrap(gap: f32) -> Div {
    row(gap).flex_wrap()
}
pub fn slot(width: f32) -> Div {
    div().w(px(width)).min_w_0()
}
pub fn description(text: impl Into<SharedString>) -> Div {
    ui::text_role(text, TextRole::Description)
}
pub fn section_title(title: impl Into<SharedString>, help: impl Into<SharedString>) -> Div {
    column(4.)
        .child(ui::text_role(title, TextRole::SectionTitle))
        .child(description(help))
}
pub fn separated(content: impl IntoElement, spacing: f32) -> Div {
    div()
        .pt(px(spacing))
        .border_t(gpui::px(crate::design::BORDER_WIDTH))
        .border_color(rgb(ZORK_UI.palette.border))
        .child(content)
}
pub fn toolbar() -> Div {
    column(12.)
        .pb_5()
        .border_b(gpui::px(crate::design::BORDER_WIDTH))
        .border_color(rgb(ZORK_UI.palette.border))
}
pub fn setting(
    label: impl Into<SharedString>,
    help: impl Into<SharedString>,
    control: impl IntoElement,
) -> Div {
    column(4.)
        .child(
            row(8.)
                .justify_between()
                .child(ui::text_role(label, TextRole::Label).text_color(rgb(ZORK_UI.palette.text)))
                .child(control),
        )
        .child(ui::text_role(help, TextRole::Metadata))
}
pub fn switch_setting(
    label: impl Into<SharedString>,
    help: impl Into<SharedString>,
    control: impl IntoElement,
) -> Div {
    row(8.)
        .justify_between()
        .child(
            column(4.)
                .child(ui::text_role(label, TextRole::Label).text_color(rgb(ZORK_UI.palette.text)))
                .child(ui::text_role(help, TextRole::Metadata)),
        )
        .child(control)
}

/// A bounded value display with independent decrement/increment availability.
/// Bounds and value semantics stay with the caller; this component owns feedback.
pub fn stepper<V: 'static>(
    id: impl Into<SharedString>,
    label: &str,
    value: String,
    decrement: bool,
    increment: bool,
    window: &mut Window,
    cx: &mut Context<V>,
    change: impl Fn(&mut V, i32, &mut Context<V>) + 'static,
) -> Stateful<Div> {
    let id = id.into();
    let change = std::rc::Rc::new(change);
    let mut content = row(0.);
    for (direction, enabled) in [(-1, decrement), (1, increment)] {
        if direction > 0 {
            content = content.child(
                ui::text_role(value.clone(), TextRole::Body)
                    .id(format!("{id}-value"))
                    .w(px(36.))
                    .text_align(gpui::TextAlign::Center)
                    .automation(AutomationRole::Status, value.clone()),
            );
        }
        let suffix = if direction < 0 { "less" } else { "more" };
        let label = format!("{}{label}", if direction < 0 { "减少" } else { "增加" });
        let change = change.clone();
        content = content.child(
            super::widgets::controls::action(
                format!("{id}-{suffix}"),
                "",
                32.,
                32.,
                super::widgets::controls::ActionStyle {
                    quiet: true,
                    disabled: !enabled,
                    icon: Some(if direction < 0 {
                        "icons/minus.svg"
                    } else {
                        "icons/plus.svg"
                    }),
                    ..Default::default()
                },
                ZORK_UI.palette.canvas,
                window,
                cx,
            )
            .on_click(cx.listener(move |view, _, _, cx| {
                if enabled {
                    change(view, direction, cx);
                }
            }))
            .automation_enabled(enabled, AutomationRole::Button, label),
        );
    }
    super::widgets::skin(
        format!("{id}-stepper"),
        100.,
        32.,
        ui::FIELD_RADIUS,
        super::widgets::SurfaceColors::outlined(FORM.outline, ZORK_UI.palette.canvas),
        content,
        window,
        cx,
    )
}

pub fn header(
    title: impl Into<SharedString>,
    subtitle: Option<&str>,
    gutter: f32,
    actions: impl IntoElement,
) -> Div {
    row(12.)
        .h(px(52.))
        .flex_shrink_0()
        .px(px(gutter))
        .justify_between()
        .border_b(gpui::px(crate::design::BORDER_WIDTH))
        .border_color(rgb(ZORK_UI.palette.border))
        .child(
            row(8.)
                .child(gpui::img("brand/mark.svg").size(px(22.)).flex_shrink_0())
                .child(ui::text_role(title, TextRole::SectionTitle))
                .when_some(subtitle, |v, text| {
                    v.child(ui::text_role(text.to_owned(), TextRole::Metadata).ml_2())
                }),
        )
        .child(actions)
}

pub enum Rail {
    Navigation,
    Inspector,
}
pub fn rail(
    id: impl Into<ElementId>,
    width: f32,
    kind: Rail,
    content: impl IntoElement,
) -> Stateful<Div> {
    let is_navigation = matches!(kind, Rail::Navigation);
    div()
        .id(id)
        .w(px(width))
        .h_full()
        .flex_shrink_0()
        .overflow_y_scroll()
        .p(px(if is_navigation { 12. } else { 16. }))
        .border_color(rgb(ZORK_UI.palette.border))
        .when(is_navigation, |v| {
            v.bg(rgb(ZORK_UI.palette.sidebar))
                .border_r(gpui::px(crate::design::BORDER_WIDTH))
        })
        .when(!is_navigation, |v| {
            v.border_l(gpui::px(crate::design::BORDER_WIDTH))
        })
        .child(content)
}
pub fn body() -> Div {
    row(0.).items_stretch().flex_1().min_h_0().min_w_0()
}
pub fn content(width: f32) -> Div {
    column(GAP).w(px(width)).max_w_full().mx_auto()
}
pub fn page_heading(
    title: impl Into<SharedString>,
    help: impl Into<SharedString>,
    action: impl IntoElement,
) -> Div {
    row(16.)
        .items_start()
        .justify_between()
        .child(
            column(4.)
                .flex_1()
                .min_w_0()
                .child(ui::page_title(title))
                .child(description(help)),
        )
        .child(action)
}
pub fn grid() -> Div {
    wrap(GAP).items_start()
}
pub fn viewport(id: impl Into<ElementId>, gutter: f32, content: impl IntoElement) -> Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_y_scroll()
        .p(px(gutter))
        .bg(rgb(CANVAS()))
        .child(content)
}
pub fn shell(
    id: impl Into<ElementId>,
    font: Font,
    header: impl IntoElement,
    body: impl IntoElement,
    overlay: Option<gpui::AnyElement>,
) -> Stateful<Div> {
    div()
        .id(id)
        .size_full()
        .overflow_hidden()
        .flex()
        .flex_col()
        .bg(rgb(ZORK_UI.palette.canvas))
        .text_color(rgb(ZORK_UI.palette.text))
        .text_size(px(13.))
        .font(font)
        .child(header)
        .child(body)
        .when_some(overlay, |v, overlay| v.child(overlay))
        .on_key_down(crate::navigation::keyboard_navigation)
}

/// A content-sized, unframed viewport. Components own their visible surfaces.
pub fn preview(
    id: impl Into<SharedString>,
    width: f32,
    content: impl IntoElement,
) -> impl IntoElement {
    div()
        .id(id.into())
        .relative()
        .w(px(width))
        .overflow_hidden()
        .child(content)
        .automation(AutomationRole::Status, "组件预览")
}
