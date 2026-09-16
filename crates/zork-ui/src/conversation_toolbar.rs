//! Read-only conversation members and the complete file/page menu.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
    design::CUE_UI,
};
use gpui::{prelude::*, *};
use std::rc::Rc;
pub struct Member {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
}
pub fn render<V: 'static>(
    members: Vec<Member>,
    contents: Entity<crate::conversation_contents::Menu>,
    right: f32,
    history_label: String,
    cx: &Context<V>,
    open: impl Fn(&mut V, String, &mut Context<V>) + 'static,
) -> impl IntoElement {
    let open = Rc::new(open);
    div()
        .id("conversation-toolbar")
        .absolute()
        .occlude()
        .top(px(6.))
        .h(px(32.))
        .flex()
        .items_center()
        .gap_2()
        .right(px(right))
        .children(members.into_iter().map(|member| {
            let open = open.clone();
            div()
                .id(format!("header-member-{}", member.id))
                .size(px(26.))
                .rounded_full()
                .bg(rgb(CUE_UI.palette.canvas))
                .cursor_pointer()
                .child(ui::agent_avatar(member.avatar.as_deref(), 26.))
                .on_click(cx.listener(move |v, _, _, cx| open(v, member.id.clone(), cx)))
                .automation(
                    AutomationRole::Button,
                    format!("{} · {}", member.name, history_label),
                )
        }))
        .child(contents)
}
