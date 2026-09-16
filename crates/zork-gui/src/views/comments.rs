use super::*;
use crate::comments::{CommentSource, DraftComment};
use zork_ui::components::comments::{EditorRequest, Submit};

#[derive(Clone)]
pub(super) struct CommentPopover {
    pub source: CommentSource,
}
impl RootView {
    pub(super) fn dismiss_selection(&mut self, cx: &mut Context<Self>) {
        self.comment_popover = None;
        self.comment_editor.update(cx, |editor, cx| editor.dismiss(cx));
        self.transcript_selection.borrow_mut().clear();
        self.message_reader.update(cx, |reader, cx| reader.clear_selection(cx));
        zork_ui::components::region::invalidate(
            cx,
            &["composer", "transcript", "message-reader", "overlays"],
        );
    }
    pub(super) fn current_comments(&self) -> &[DraftComment] {
        &self.draft_state.comments
    }
    pub(super) fn finish_text_selection(
        &mut self,
        event: &gpui::MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.transcript_selection.borrow().dragging { return; }
        let selection_bounds = self.transcript_selection.borrow().selected_bounds();
        let source = self.transcript_selection.borrow_mut().finish();
        if let Some(source) =
            source.filter(|s| Some(&s.session_id) == self.selected_session.as_ref())
        {
            let bounds = selection_bounds.unwrap_or_else(|| gpui::Bounds::new(event.position, gpui::size(px(1.), px(1.))));
            let focus = window.focused(cx);
            self.comment_editor.update(cx, |editor, cx| editor.open_at(EditorRequest {
                source: source.clone(), editing: None, text: String::new(), toolbar: true,
            }, bounds, focus, window, cx));
            self.comment_popover = Some(CommentPopover { source });
            zork_ui::components::region::invalidate(cx, &["composer", "transcript", "overlays"]);
        }
    }
    pub(super) fn add_comment(&mut self, event: &Submit, cx: &mut Context<Self>) {
        let comment = event.text.clone();
        if comment.is_empty() {
            return;
        }
        let Some(popover) = self.comment_popover.take() else {
            return;
        };
        if Some(&event.source.session_id) != self.selected_session.as_ref()
            || popover.source != event.source {
            return;
        }
        let session = event.source.session_id.clone();
        let comment = DraftComment {
            id: event
                .editing.clone()
                .unwrap_or_else(|| ulid::Ulid::new().to_string()),
            source: event.source.clone(),
            comment,
        };
        if let Err(error) = self.core_device.put_comment(&session, comment) {
            self.error = Some(error.to_string());
        }
        self.draft_state = self.core_device.draft(&session);
        self.transcript_selection.borrow_mut().clear();
        self.message_reader.update(cx, |reader, cx| reader.clear_selection(cx));
        self.comment_editor.update(cx, |editor, cx| editor.dismiss(cx));
        self.save_draft(cx);
        zork_ui::components::region::invalidate(cx, &["composer", "transcript", "overlays"]);
    }
    pub(super) fn render_comment_queue(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        zork_ui::components::comments::queue(
            "",
            self.current_comments(),
            window,
            cx,
            |v, comment, _, bounds, w, cx| {
                let focus = w.focused(cx);
                v.comment_editor.update(cx, |editor, cx| editor.open_at(EditorRequest {
                    source: comment.source.clone(), editing: Some(comment.id), text: comment.comment, toolbar: false,
                }, bounds, focus, w, cx));
                v.comment_popover = Some(CommentPopover { source: comment.source });
                zork_ui::components::region::invalidate(
                    cx,
                    &["composer", "transcript", "overlays"],
                );
            },
            |v, id, cx| {
                if let Some(session) = &v.selected_session {
                    if let Err(error) = v.core_device.remove_comment(session, &id) {
                        v.error = Some(error.to_string());
                    }
                    v.draft_state = v.core_device.draft(session);
                }
                v.save_draft(cx);
                zork_ui::components::region::invalidate(
                    cx,
                    &["composer", "transcript", "overlays"],
                );
            },
        )
    }
    pub(super) fn render_comment_popover(
        &mut self, _: &mut Window, _: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.comment_editor.clone().into_any_element()
    }
}
