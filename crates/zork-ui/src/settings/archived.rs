//! Archived Chats, opened from client settings. The host owns archive state;
//! this list only reads it and asks to open or restore a Chat.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
    design::ZORK_UI,
    resources::Text,
};
use gpui::{div, prelude::*, px, rgb, Context};
use std::rc::Rc;
use zork_client_types::navigation::NavigationChat;

#[derive(Clone)]
pub struct ArchivedChat {
    pub node: String,
    pub device: String,
    pub chat: NavigationChat,
}

pub enum Action {
    Open { node: String, chat: String },
    Restore {
        node: String,
        chat: String,
        expected_message_count: u64,
    },
}

/// Newest first; the same ordering as the Chat list.
pub fn sort(chats: &mut [ArchivedChat]) {
    chats.sort_by(|a, b| {
        b.chat
            .updated_at
            .cmp(&a.chat.updated_at)
            .then_with(|| a.chat.chat_id.cmp(&b.chat.chat_id))
            .then_with(|| a.node.cmp(&b.node))
    });
}

fn day(updated: &str) -> String {
    // `YYYY-MM-DDT…` → `MM-DD`; anything else stays out of the row.
    updated.get(5..10).unwrap_or_default().to_owned()
}

pub fn list<V: 'static>(
    chats: Vec<ArchivedChat>,
    text: &Text,
    cx: &mut Context<V>,
    action: impl Fn(&mut V, Action, &mut Context<V>) + 'static,
) -> gpui::AnyElement {
    let p = ZORK_UI.palette;
    let action = Rc::new(action);
    if chats.is_empty() {
        let empty = text.text("chat_archive_empty").to_string();
        return div()
            .id("archived-chats-empty")
            .text_size(px(13.))
            .line_height(px(20.))
            .text_color(rgb(p.muted))
            .child(empty.clone())
            .automation(AutomationRole::Status, empty)
            .into_any_element();
    }
    let restore_label = text.text("chat_unarchive").to_string();
    let untitled = text.text("device_untitled_task").to_string();
    div()
        .id("archived-chats")
        .flex()
        .flex_col()
        .gap(px(2.))
        .children(chats.into_iter().map(|item| {
            let title = if item.chat.title.is_empty() {
                untitled.to_string()
            } else {
                item.chat.title.clone()
            };
            let key = format!("{}-{}", item.node, item.chat.chat_id);
            let meta = [item.device.clone(), day(&item.chat.updated_at)]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            let (open, restore) = (action.clone(), action.clone());
            let (open_node, open_chat) = (item.node.clone(), item.chat.chat_id.clone());
            let (node, chat, count) = (
                item.node.clone(),
                item.chat.chat_id.clone(),
                item.chat.message_count,
            );
            let pending = item.chat.archive_pending;
            div()
                .id(format!("archived-{key}"))
                .w_full()
                .min_h(px(40.))
                .pl(px(10.))
                .pr(px(4.))
                .py(px(4.))
                .flex()
                .items_center()
                .gap(px(10.))
                .rounded(px(crate::design::RADIUS.control))
                .cursor_pointer()
                .hover(|row| row.bg(rgb(crate::design::INTERACTION.neutral_hover)))
                .on_click(cx.listener(move |v, _, _, cx| {
                    open(
                        v,
                        Action::Open {
                            node: open_node.clone(),
                            chat: open_chat.clone(),
                        },
                        cx,
                    )
                }))
                .child(crate::device_name::mark(&item.device, 18.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_baseline()
                        .gap(px(8.))
                        .child(
                            div()
                                .min_w_0()
                                .text_size(px(13.))
                                .line_height(px(20.))
                                .text_ellipsis()
                                .child(title.clone()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(12.))
                                .line_height(px(16.))
                                .text_color(rgb(p.subtle))
                                .child(meta.clone()),
                        ),
                )
                .when_some(item.chat.archive_error.clone(), |row, error| {
                    row.child(
                        div()
                            .flex_shrink_0()
                            .max_w(px(200.))
                            .text_ellipsis()
                            .text_size(px(12.))
                            .text_color(rgb(p.danger))
                            .child(error),
                    )
                })
                .child(
                    ui::quiet_button(
                        format!("unarchive-{key}"),
                        restore_label.clone(),
                        !pending,
                        ui::IconButtonSize::Compact,
                    )
                    .on_click(cx.listener(move |v, _, _, cx| {
                        cx.stop_propagation();
                        restore(
                            v,
                            Action::Restore {
                                node: node.clone(),
                                chat: chat.clone(),
                                expected_message_count: count,
                            },
                            cx,
                        )
                    }))
                    .automation_enabled(
                        !pending,
                        AutomationRole::Button,
                        format!("{restore_label} {title}"),
                    ),
                )
                .automation(AutomationRole::Button, format!("{title} · {meta}"))
        }))
        .into_any_element()
}
