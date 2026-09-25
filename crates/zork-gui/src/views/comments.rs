//! User replies to message passages (docs/design/interface.md 用户回复).
//!
//! Selecting text in a message offers a floating dark "引用回复" pill; each
//! click adds one draft comment (source message + passage). Drafts live in the
//! composer in the shape of the sent message: a quote line and the draft's own
//! reply input. Passages in the draft get a dotted underline in the transcript.
//! Payloads keep the `<zork-message-comments version="1">` format.
use super::*;
use crate::comments::{CommentSource, DraftComment};
use zork_ui::components::comments::{self as ui_comments, DraftView};
use zork_ui::components::message_row::QuoteAuthor;

/// A finished transcript selection waiting for "引用回复".
#[derive(Clone)]
pub(super) struct CommentPopover {
    pub source: CommentSource,
    pub bounds: gpui::Bounds<gpui::Pixels>,
}
impl RootView {
    pub(super) fn dismiss_selection(&mut self, cx: &mut Context<Self>) {
        self.comment_popover = None;
        self.transcript_selection.borrow_mut().clear();
        zork_ui::components::region::invalidate(cx, &["composer", "transcript", "overlays"]);
    }
    pub(super) fn current_comments(&self) -> &[DraftComment] {
        &self.draft_state.comments
    }
    pub(super) fn finish_text_selection(
        &mut self,
        event: &gpui::MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.transcript_selection.borrow().dragging {
            return;
        }
        let selection_bounds = self.transcript_selection.borrow().selected_bounds();
        let source = self.transcript_selection.borrow_mut().finish();
        self.comment_popover = source
            .filter(|s| Some(&s.session_id) == self.selected_session.as_ref())
            .map(|source| CommentPopover {
                source,
                bounds: selection_bounds.unwrap_or_else(|| {
                    gpui::Bounds::new(event.position, gpui::size(px(1.), px(1.)))
                }),
            });
        zork_ui::components::region::invalidate(cx, &["composer", "transcript", "overlays"]);
    }

    /// Adds one draft quote unless the same passage of the same message is
    /// already drafted, then focuses its reply input.
    pub(super) fn add_draft(&mut self, mut source: CommentSource, cx: &mut Context<Self>) {
        let Some(session) = self.selected_session.clone() else {
            return;
        };
        source.quote = source
            .quote
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if source.quote.is_empty() || source.session_id != session {
            return;
        }
        if ui_comments::already_drafted(self.current_comments(), &source) {
            self.show_hint(self.locale.text("draft_duplicate").into(), cx);
            return;
        }
        let comment = DraftComment {
            id: ulid::Ulid::new().to_string(),
            source,
            comment: String::new(),
        };
        self.focus_draft = Some(comment.id.clone());
        if let Err(error) = self.core_device.put_comment(&session, comment) {
            self.error = Some(error.to_string());
        }
        self.draft_state = self.core_device.draft(&session);
        self.save_draft(cx);
        zork_ui::components::region::invalidate(cx, &["composer", "transcript", "overlays"]);
    }

    /// "引用回复" on the pending selection.
    pub(super) fn quote_selection(&mut self, cx: &mut Context<Self>) {
        let Some(popover) = self.comment_popover.take() else {
            return;
        };
        self.transcript_selection.borrow_mut().clear();
        self.add_draft(popover.source, cx);
    }

    /// The hover action "引用": the whole message (its first ~60 characters).
    pub(super) fn quote_whole_message(
        &mut self,
        message_id: &str,
        quote: String,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.selected_session.clone() else {
            return;
        };
        let Some(TranscriptLine::Message { metadata, .. }) = self
            .transcript_lookup
            .index_of(message_id)
            .and_then(|index| self.lines.get(index))
        else {
            return;
        };
        let source = CommentSource {
            session_id: session,
            message_id: metadata.id.clone(),
            author: metadata.author_name.clone(),
            author_agent_id: metadata.author_agent_id.clone(),
            quote,
        };
        self.add_draft(source, cx);
    }

    /// One reply input per draft, created on first sight and dropped with it.
    fn sync_draft_inputs(&mut self, cx: &mut Context<Self>) {
        let comments = self.draft_state.comments.clone();
        self.draft_inputs
            .retain(|id, _| comments.iter().any(|comment| &comment.id == id));
        for comment in comments.iter() {
            if self.draft_inputs.contains_key(&comment.id) {
                continue;
            }
            let placeholder = self.locale.text("draft_reply_placeholder").to_owned();
            let input = cx.new(|cx| ComposerInput::new(placeholder, cx).multiline());
            let text = comment.comment.clone();
            input.update(cx, |input, cx| input.reset_value(text, cx));
            let id = comment.id.clone();
            cx.subscribe(&input, move |view, input, _: &ComposerEdited, cx| {
                let text = input.read(cx).value().to_owned();
                view.edit_draft_reply(&id, text, cx);
            })
            .detach();
            cx.subscribe(&input, |view, _, _: &ComposerSubmit, cx| {
                if view.selected_session.is_some() {
                    view.send_composer(cx);
                }
            })
            .detach();
            cx.subscribe(&input, |_, _, _: &ComposerLayoutChanged, cx| {
                zork_ui::components::region::invalidate(cx, &["composer"]);
            })
            .detach();
            self.draft_inputs.insert(comment.id.clone(), input);
        }
    }

    fn edit_draft_reply(&mut self, id: &str, text: String, cx: &mut Context<Self>) {
        let Some(session) = self.selected_session.clone() else {
            return;
        };
        let Some(mut comment) = self
            .draft_state
            .comments
            .iter()
            .find(|comment| comment.id == id)
            .cloned()
        else {
            return;
        };
        if comment.comment == text {
            return;
        }
        comment.comment = text;
        if let Err(error) = self.core_device.put_comment(&session, comment) {
            self.error = Some(error.to_string());
        }
        self.draft_state = self.core_device.draft(&session);
        cx.notify();
    }

    fn remove_draft(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(session) = &self.selected_session {
            if let Err(error) = self.core_device.remove_comment(session, id) {
                self.error = Some(error.to_string());
            }
            self.draft_state = self.core_device.draft(session);
        }
        self.draft_inputs.remove(id);
        self.save_draft(cx);
        zork_ui::components::region::invalidate(cx, &["composer", "transcript", "overlays"]);
    }

    /// Core refused the send: a draft quote has no reply of its own. Say so
    /// and put the cursor in the first empty reply.
    pub(super) fn refuse_empty_draft_reply(&mut self, cx: &mut Context<Self>) {
        self.show_hint(self.locale.text("draft_reply_required").into(), cx);
        if let Some(empty) = self
            .current_comments()
            .iter()
            .find(|comment| comment.comment.trim().is_empty())
        {
            self.focus_draft = Some(empty.id.clone());
        }
    }

    /// Draft passages by source message id (dotted underlines in place).
    pub(super) fn draft_passages(&self) -> HashMap<String, Vec<String>> {
        let mut passages: HashMap<String, Vec<String>> = HashMap::new();
        for comment in self.current_comments() {
            if let Some(id) = &comment.source.message_id {
                passages
                    .entry(id.clone())
                    .or_default()
                    .push(comment.source.quote.clone());
            }
        }
        passages
    }

    /// The source author as the transcript shows it: the agent's disc and
    /// name from core's presentation, else the recorded name with a disc in
    /// the agent's own tint (the same slot it has everywhere).
    fn draft_author(&self, comment: &DraftComment) -> QuoteAuthor {
        let presented = &self.multi_agent.presented;
        let source_row = comment
            .source
            .message_id
            .as_deref()
            .and_then(|id| self.transcript_lookup.index_of(id))
            .and_then(|index| presented.rows.get(index));
        if source_row.is_some_and(|row| row.user) {
            return QuoteAuthor {
                name: if self.locale == Locale::ZhCn {
                    "你"
                } else {
                    "You"
                }
                .into(),
                disc: None,
            };
        }
        presented
            .rows
            .iter()
            .filter_map(|row| row.identity.as_ref())
            .find(|identity| {
                comment.source.author_agent_id.is_some()
                    && identity.author.agent_id == comment.source.author_agent_id
            })
            .map(|identity| identity.author.quote_author())
            .unwrap_or_else(|| {
                let name = comment
                    .source
                    .author
                    .clone()
                    .unwrap_or_else(|| "消息".into());
                QuoteAuthor {
                    disc: comment.source.author_agent_id.as_deref().map(|agent| {
                        zork_ui::components::message_row::Disc {
                            tint: zork_ui::design::agent_tint_slot(agent),
                            maker: None,
                            initial: zork_client_core::message_presentation::initial(&name),
                        }
                    }),
                    name,
                }
            })
    }

    /// The drafts band for the composer and its height.
    pub(super) fn render_drafts(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(gpui::AnyElement, f32)> {
        self.sync_draft_inputs(cx);
        if self.draft_state.comments.is_empty() {
            return None;
        }
        if let Some(id) = self.focus_draft.take() {
            if let Some(input) = self.draft_inputs.get(&id) {
                let handle = input.read(cx).focus_handle();
                window.focus(&handle, cx);
            }
        }
        let comments = self.draft_state.comments.clone();
        let views: Vec<DraftView> = comments
            .iter()
            .filter_map(|comment| {
                Some(DraftView {
                    comment: comment.clone(),
                    author: self.draft_author(comment),
                    input: self.draft_inputs.get(&comment.id)?.clone(),
                })
            })
            .collect();
        let height = ui_comments::drafts_height(
            views
                .iter()
                .map(|view| ui_comments::draft_input_height(view.input.read(cx))),
        );
        let element = ui_comments::drafts(
            "",
            views,
            self.locale.text("draft_remove"),
            cx,
            |v, comment, _, cx| {
                let target = comment
                    .source
                    .message_id
                    .as_deref()
                    .and_then(|id| v.transcript_lookup.index_of(id));
                if let Some(target) = target {
                    v.jump_to_message(target, Some(comment.source.quote.clone()), cx);
                }
            },
            |v, id, cx| v.remove_draft(&id, cx),
        );
        Some((element, height))
    }

    /// The floating dark "引用回复" pill above the selection's end.
    pub(super) fn render_comment_popover(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(popover) = &self.comment_popover else {
            return div().into_any_element();
        };
        let label = self.locale.text("quote_reply").to_owned();
        // Screen readers hear the passage with the action.
        let accessible = format!("{label}：{}", popover.source.quote);
        let bounds = popover.bounds;
        let anchor = gpui::point(
            (bounds.right() - px(40.)).max(bounds.left()),
            bounds.top() - px(34.),
        );
        gpui::deferred(
            gpui::anchored().position(anchor).snap_to_window().child(
                zork_ui::components::message_row::identity::dark_pill(
                    "selection-quote-reply",
                    label.clone(),
                )
                .occlude()
                .cursor_pointer()
                .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(cx.listener(|v, _, _, cx| {
                    cx.stop_propagation();
                    v.quote_selection(cx)
                }))
                .automation(AutomationRole::Button, accessible),
            ),
        )
        .with_priority(2)
        .into_any_element()
    }
}
