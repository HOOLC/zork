//! Accessible component contracts composed from the shared material and editors.
//! Values are controlled by the host; only focus, pointer capture and disclosure
//! animation are retained here. Business validation and persistence stay in core.
pub mod data;
pub mod dialog;
pub mod disclosure;
pub mod feedback;
pub mod input;
pub mod menu;
pub mod range;
pub mod selection;

use crate::{
    components::smooth,
    design::{LIQUID_OUTLINE, ZORK_UI},
};
use gpui::{prelude::*, *};

/// Intrinsic content determines the height; the background follows that layout.
pub fn surface(
    id: impl Into<SharedString>,
    radius: f32,
    fill: u32,
    outlined: bool,
) -> Stateful<Div> {
    let id = id.into();
    div().id(id.clone()).relative().child(
        smooth::fill(format!("{id}-fill"), radius)
            .bg(rgb(fill))
            .when(outlined, |v| {
                v.border(px(crate::design::BORDER_WIDTH))
                    .border_color(rgb(LIQUID_OUTLINE))
            }),
    )
}

pub fn disabled_node(node: &mut accesskit::Node, disabled: bool) {
    if disabled {
        node.set_disabled();
    }
}

pub fn label<V: Focusable + 'static>(
    id: impl Into<SharedString>,
    text: impl Into<SharedString>,
    target: Entity<V>,
    disabled: bool,
) -> impl IntoElement {
    use crate::automation::{AutomationElementExt, AutomationRole};
    let text = text.into();
    div()
        .id(id.into())
        .role(Role::Label)
        .aria_label(text.clone())
        .text_size(px(12.))
        .line_height(px(20.))
        .text_color(rgb(ZORK_UI.palette.muted))
        .child(text.clone())
        .on_click(move |_, window, cx| {
            if !disabled {
                window.focus(&target.read(cx).focus_handle(cx), cx);
            }
        })
        .automation_enabled(!disabled, AutomationRole::Button, format!("聚焦{text}"))
}
