//! Complete empty/loading transcript state; the host supplies its resolved reason and paging capability.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::loading,
    design::CUE_UI,
};
use gpui::{prelude::*, *};
pub struct Data {
    pub loading: bool,
    pub message: String,
    pub older: Option<(String, bool)>,
}
pub fn render<V: 'static>(
    data: Data,
    cx: &mut Context<V>,
    older: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    if data.loading {
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(13.))
            .text_color(rgb(CUE_UI.palette.muted))
            .child(loading::status("messages-loading", data.message))
            .into_any_element()
    } else {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .gap_3()
            .items_center()
            .justify_center()
            .px_6()
            .text_size(px(14.))
            .text_color(rgb(CUE_UI.palette.subtle))
            .child(data.message)
            .when_some(data.older, |view, (label, enabled)| {
                view.child(
                    crate::controls::button("load-older", label.clone(), false, enabled)
                        .on_click(cx.listener(move |view, _, _, cx| older(view, cx)))
                        .automation(AutomationRole::Button, label),
                )
            })
            .into_any_element()
    }
}
