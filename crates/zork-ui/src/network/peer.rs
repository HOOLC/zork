//! Full manual pairing dialog; hosts supply input entities and result state.
use super::*;
use crate::components::text_input::ComposerInput;
use crate::settings::row;
use gpui::{AnyElement, Entity, Window};
pub struct Fields<'a> {
    pub name: &'a Entity<ComposerInput>,
    pub origin: &'a Entity<ComposerInput>,
    pub address: &'a Entity<ComposerInput>,
    pub grant: bool,
    pub focus: &'a FocusHandle,
    pub busy: bool,
    pub notice: Option<String>,
}
pub enum Action {
    Grant(bool),
    Save,
    Cancel,
}
pub fn render<V: 'static>(
    fields: Fields<'_>,
    modal: &crate::modal::ModalState,
    window: &mut Window,
    cx: &mut Context<V>,
    action: impl Fn(&mut V, Action, &mut Context<V>) + 'static,
) -> AnyElement {
    let action = Rc::new(action);
    let grant = action.clone();
    let cancel = action.clone();
    let save = action.clone();
    let busy = fields.busy;
    ui::modal(
        "mesh-peer-dialog",
        "手动连接设备",
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(ui::field("mesh-peer-name", "设备名称", fields.name, cx))
            .child(ui::field("mesh-peer-origin", "设备身份", fields.origin, cx))
            .child(ui::field(
                "mesh-peer-addr",
                "局域网地址 · 可选",
                fields.address,
                cx,
            ))
            .child(row(
                "允许作为客户端管理",
                "可管理此设备的模型连接与对话。",
                ui::switch(
                    "mesh-client-grant",
                    "客户端权限",
                    fields.grant,
                    !busy,
                    fields.focus,
                    cx,
                    move |v, on, cx| grant(v, Action::Grant(on), cx),
                ),
            )),
        div()
            .flex()
            .justify_end()
            .gap_2()
            .child(
                ui::button("mesh-cancel-peer", "取消", false, !busy)
                    .on_click(cx.listener(move |v, _, _, cx| {
                        if !busy {
                            cancel(v, Action::Cancel, cx);
                        }
                    }))
                    .automation_enabled(!busy, AutomationRole::Button, "取消添加"),
            )
            .child(
                ui::busy_button(
                    "mesh-add-peer",
                    if busy {
                        "正在保存…"
                    } else {
                        "保存配对"
                    },
                    true,
                    !busy,
                    busy,
                )
                .on_click(cx.listener(move |v, _, _, cx| save(v, Action::Save, cx)))
                .automation_enabled(!busy, AutomationRole::Button, "保存配对"),
            ),
        fields.notice,
        modal,
        window,
        cx,
        !busy,
        move |v, _, cx| action(v, Action::Cancel, cx),
    )
    .into_any_element()
}
