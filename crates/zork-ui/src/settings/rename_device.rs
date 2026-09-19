//! Complete device-name editor; the host supplies save progress and validation results.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::text_input::ComposerInput,
    controls as ui,
    design::ZORK_UI,
};
use gpui::{prelude::*, *};
use std::rc::Rc;
pub enum Action {
    Save,
    Cancel,
}
pub fn render<V: 'static>(
    input: &Entity<ComposerInput>,
    busy: bool,
    error: Option<String>,
    modal: &ui::ModalState,
    window: &mut Window,
    cx: &mut Context<V>,
    action: impl Fn(&mut V, Action, &mut Context<V>) + 'static,
) -> AnyElement {
    let action = Rc::new(action);
    let cancel = action.clone();
    let save = action.clone();
    ui::modal(
        "device-rename-dialog",
        "修改设备名称",
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(ui::field("device-name-input", "名称", input, cx))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(ZORK_UI.palette.muted))
                    .child("连接此设备的小伙伴都会看到新名称。"),
            ),
        div()
            .flex()
            .justify_end()
            .gap_2()
            .child(
                ui::button("device-name-cancel", "取消", false, !busy).on_click(cx.listener(
                    move |v, _, _, cx| {
                        if !busy {
                            cancel(v, Action::Cancel, cx);
                        }
                    },
                )),
            )
            .child(
                ui::busy_button(
                    "device-name-save",
                    if busy { "保存中…" } else { "保存" },
                    true,
                    !busy,
                    busy,
                )
                .on_click(cx.listener(move |v, _, _, cx| save(v, Action::Save, cx)))
                .automation_enabled(!busy, AutomationRole::Button, "保存设备名称"),
            ),
        error,
        modal,
        window,
        cx,
        !busy,
        move |v, _, cx| action(v, Action::Cancel, cx),
    )
    .into_any_element()
}
