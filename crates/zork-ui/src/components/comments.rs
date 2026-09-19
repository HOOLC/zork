//! Quoted comment presentation. Hosts decide when to persist or send a batch.
use super::liquid::controls::ControlElement;
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    comments::DraftComment,
    controls as ui,
    design::ZORK_UI,
};
use gpui::{div, prelude::*, px, rgb, ClickEvent, Context, Entity, Window};
use std::{cell::Cell, rc::Rc};
mod editor;
pub use editor::{Closed, Editor, EditorRequest, Submit};
fn id(prefix: &str, suffix: &str) -> String {
    if prefix.is_empty() {
        suffix.into()
    } else {
        format!("{prefix}-{suffix}")
    }
}
pub fn queue<V: 'static>(
    prefix: &str,
    comments: &[DraftComment],
    window: &mut Window,
    cx: &mut Context<V>,
    edit: impl Fn(
            &mut V,
            DraftComment,
            &ClickEvent,
            gpui::Bounds<gpui::Pixels>,
            &mut Window,
            &mut Context<V>,
        ) + 'static,
    remove: impl Fn(&mut V, String, &mut Context<V>) + 'static,
) -> gpui::AnyElement {
    let edit = Rc::new(edit);
    let remove = Rc::new(remove);
    let p = ZORK_UI.palette;
    div()
        .id(id(prefix, "composer-comment-queue"))
        .flex()
        .flex_col()
        .gap(px(2.))
        .max_h(px(140.))
        .overflow_y_scroll()
        .children(comments.iter().cloned().map(|comment| {
            let editing = comment.clone();
            let key = comment.id.clone();
            let edit = edit.clone();
            let remove = remove.clone();
            let edit_id = id(prefix, &format!("comment-edit-{}", comment.id));
            let focus = super::liquid::controls::action_focus(edit_id.clone(), window, cx);
            let bounds = Rc::new(Cell::new(gpui::Bounds::default()));
            let measured = bounds.clone();
            super::liquid::panel::inline(id(prefix, &format!("queued-comment-{}", comment.id)))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_2()
                .radius(6.)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(p.muted))
                                .truncate()
                                .child(format!(
                                    "{}：{}",
                                    comment.source.author.as_deref().unwrap_or("消息"),
                                    comment.source.quote
                                )),
                        )
                        .child(div().text_size(px(12.)).truncate().child(comment.comment)),
                )
                .child(
                    ui::button(edit_id, "编辑", false, true)
                        .control_focus(&focus)
                        .control_overlay(
                            gpui::canvas(move |bounds, _, _| measured.set(bounds), |_, _, _, _| {})
                                .absolute()
                                .inset_0()
                                .into_any_element(),
                        )
                        .on_click(cx.listener(move |v, event, w, cx| {
                            w.focus(&focus, cx);
                            edit(v, editing.clone(), event, bounds.get(), w, cx);
                        }))
                        .automation(AutomationRole::Button, "编辑评论"),
                )
                .child(
                    ui::icon_button(id(prefix, &format!("comment-remove-{}", comment.id)), true)
                        .w(px(ui::CONTROL_HEIGHT))
                        .h(px(ui::CONTROL_HEIGHT))
                        .px_0()
                        .border_0()
                        .child(ui::icon("icons/x.svg", 12.))
                        .on_click(cx.listener(move |v, _, _, cx| remove(v, key.clone(), cx)))
                        .automation(AutomationRole::Button, "移除评论"),
                )
        }))
        .into_any_element()
}
#[cfg(feature = "stories")]
pub struct CommentsStory {
    prefix: String,
    comments: Vec<DraftComment>,
    editor: Entity<Editor>,
    pending: Option<EditorRequest>,
    anchor: Rc<Cell<gpui::Bounds<gpui::Pixels>>>,
    next_id: usize,
}
#[cfg(feature = "stories")]
impl CommentsStory {
    pub fn new(prefix: String, state: &str, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| Editor::new(prefix.clone(), cx));
        cx.subscribe(&editor, |v, _, event: &Submit, cx| {
            let comment = DraftComment {
                id: event.editing.clone().unwrap_or_else(|| {
                    v.next_id += 1;
                    format!("demo-{}", v.next_id)
                }),
                source: event.source.clone(),
                comment: event.text.clone(),
            };
            if let Some(old) = v.comments.iter_mut().find(|old| old.id == comment.id) {
                *old = comment;
            } else {
                v.comments.push(comment);
            }
            v.editor.update(cx, |editor, cx| editor.dismiss(cx));
            cx.notify();
        })
        .detach();
        let request = EditorRequest {
            source: crate::comments::CommentSource {
                session_id: "mock-session".into(),
                author: Some("产品 Leader".into()),
                quote: "请先统一图标与头像。".into(),
                ..Default::default()
            },
            editing: (state == "editing").then(|| "demo".into()),
            text: if state == "editing" {
                "保持紧凑，文字需要清晰。".into()
            } else {
                String::new()
            },
            toolbar: false,
        };
        let comments = if matches!(state, "queued" | "editing") {
            vec![DraftComment {
                id: "demo".into(),
                source: request.source.clone(),
                comment: "保持紧凑，文字需要清晰。".into(),
            }]
        } else {
            vec![]
        };
        Self {
            prefix,
            comments,
            editor,
            pending: matches!(state, "compose" | "editing").then_some(request),
            anchor: Default::default(),
            next_id: 0,
        }
    }
}
#[cfg(feature = "stories")]
impl gpui::Render for CommentsStory {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds = self.anchor.get();
        if bounds.size.width > px(0.) {
            if let Some(request) = self.pending.take() {
                let focus = window.focused(cx);
                self.editor.update(cx, |editor, cx| {
                    editor.open_at(request, bounds, focus, window, cx)
                });
            }
        }
        let anchor = self.anchor.clone();
        let owner = cx.entity().downgrade();
        let trigger = ui::button(id(&self.prefix, "add-comment"), "添加评论", false, true)
            .control_overlay(
                gpui::canvas(
                    move |bounds, _, cx| {
                        if anchor.replace(bounds) != bounds {
                            let _ = owner.update(cx, |_, cx| cx.notify());
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
                .into_any_element(),
            )
            .on_click(cx.listener(|v, _, window, cx| {
                let bounds = v.anchor.get();
                let focus = window.focused(cx);
                v.editor.update(cx, |editor, cx| {
                    editor.open_at(
                        EditorRequest {
                            source: crate::comments::CommentSource {
                                session_id: "mock-session".into(),
                                author: Some("产品 Leader".into()),
                                quote: "请先统一图标与头像。".into(),
                                ..Default::default()
                            },
                            editing: None,
                            text: String::new(),
                            toolbar: false,
                        },
                        bounds,
                        focus,
                        window,
                        cx,
                    )
                });
            }))
            .automation(AutomationRole::Button, "添加评论");
        let queue = queue(
            &self.prefix,
            &self.comments,
            window,
            cx,
            |v, comment, _, bounds, window, cx| {
                let focus = window.focused(cx);
                v.editor.update(cx, |editor, cx| {
                    editor.open_at(
                        EditorRequest {
                            source: comment.source,
                            editing: Some(comment.id),
                            text: comment.comment,
                            toolbar: false,
                        },
                        bounds,
                        focus,
                        window,
                        cx,
                    )
                });
            },
            |v, id, cx| {
                v.comments.retain(|comment| comment.id != id);
                cx.notify();
            },
        );
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(trigger)
            .child(queue)
            .child(self.editor.clone())
    }
}
