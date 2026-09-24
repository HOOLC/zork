//! Progressive disclosure for panels: a "更多" menu for rare actions, an
//! expander for secondary facts and an ⓘ for explanations.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::standard_menu::{Item, Menu},
    controls as ui,
    design::ZORK_UI,
};
use gpui::{div, prelude::*, px, rgb, AnimationExt, AnyElement, Context, SharedString, Window};

/// A quiet "更多" icon button with its menu. Items that are unavailable are
/// passed disabled; an empty list renders nothing.
pub fn more_menu<V: 'static>(
    id: impl Into<SharedString>,
    items: Vec<Item>,
    enabled: bool,
    window: &mut Window,
    cx: &mut Context<V>,
    choose: impl Fn(&mut V, String, &mut Window, &mut Context<V>) + 'static,
) -> AnyElement {
    if items.is_empty() {
        return gpui::Empty.into_any_element();
    }
    let id: SharedString = id.into();
    let menu = window
        .use_keyed_state(format!("{id}-menu-state"), cx, |_, _| Menu::default())
        .read(cx)
        .clone();
    let focus = crate::components::widgets::controls::action_focus(id.clone(), window, cx);
    let trigger = menu
        .trigger_element(
            ui::icon_button(id.clone(), enabled)
                .child(ui::icon("icons/more-horizontal.svg", 16.)),
            &focus,
            enabled,
            cx,
        )
        .aria_label("更多")
        .automation_enabled(enabled, AutomationRole::Button, "更多");
    div()
        .flex_shrink_0()
        .child(trigger)
        .children(menu.render(format!("{id}-menu"), items, window, cx, choose))
        .into_any_element()
}

/// A quiet "label ⌄" toggle; the content appears below it while open. The open
/// state is presentation only and survives re-renders of the same id.
pub fn expander<V: 'static>(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    content: impl IntoElement,
    window: &mut Window,
    cx: &mut Context<V>,
) -> AnyElement {
    let id: SharedString = id.into();
    let label: SharedString = label.into();
    let state = window.use_keyed_state(format!("{id}-open"), cx, |_, _| false);
    let open = *state.read(cx);
    let p = ZORK_UI.palette;
    let toggle = ui::quiet_button(id.clone(), "", true, ui::IconButtonSize::Compact)
        .ml(px(-12.))
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(p.muted))
                .child(label.clone()),
        )
        .child(
            // The chevron turns with the same move curve as the disclosure.
            ui::icon("interface/chevron-down.svg", 12.)
                .text_color(rgb(p.muted))
                .with_spring(
                    SharedString::from(format!("{id}-chevron")),
                    crate::motion::toggle(crate::motion::BASE, open),
                    |icon, phase| {
                        icon.with_transformation(gpui::Transformation::rotate(gpui::radians(
                            std::f32::consts::PI * phase.0.clamp(0., 1.),
                        )))
                    },
                ),
        )
        .aria_expanded(open)
        .on_click(move |_, _, cx| {
            state.update(cx, |open, cx| {
                *open = !*open;
                cx.notify();
            });
        })
        .automation(AutomationRole::Button, label.to_string());
    div()
        .flex()
        .flex_col()
        .items_start()
        .child(toggle)
        .when(open, |v| {
            // Content appears after the toggle: fade in while settling 4 px.
            // GPUI lays out auto heights in one pass, so the height itself snaps.
            v.child(
                div().w_full().pt_1().child(content).with_animation(
                    SharedString::from(format!("{id}-content")),
                    crate::motion::enter(crate::motion::BASE),
                    |body, t| body.opacity(t).relative().top(px(-4. * (1. - t))),
                ),
            )
        })
        .into_any_element()
}

/// An ⓘ that shows `text` on hover or keyboard focus.
pub fn info(id: impl Into<SharedString>, text: impl Into<String>) -> AnyElement {
    let id: SharedString = id.into();
    let text = text.into();
    crate::components::tooltip::hint(
        ui::icon_button_sized(id.clone(), true, ui::IconButtonSize::Small)
            .child(ui::icon("icons/info.svg", 14.).text_color(rgb(ZORK_UI.palette.subtle)))
            .aria_label(text.clone())
            .automation(AutomationRole::Button, text.clone()),
        id.to_string(),
        text,
    )
    .into_any_element()
}

/// One muted line for secondary facts.
pub fn meta(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(px(12.))
        .text_color(rgb(ZORK_UI.palette.muted))
        .child(text.into())
}
