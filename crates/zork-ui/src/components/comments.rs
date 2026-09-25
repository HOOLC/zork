//! Quoted comment presentation. Hosts decide when to persist or send a batch.
//!
//! Draft comments sit inside the composer in the shape of the sent message:
//! per draft a quote line (↩, source author, 「passage」, × to remove) and a
//! borderless auto-growing input "回复这段…". No cards or boxes.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    comments::DraftComment,
    components::{
        message_row::identity::{self, QuoteAuthor},
        text_input::ComposerInput,
    },
    controls as ui,
    design::{INTERACTION, ZORK_UI},
};
use gpui::{div, prelude::*, px, rgb, Context, Entity, Window};
use std::rc::Rc;
mod editor;
pub use editor::{Closed, Editor, EditorRequest, Submit};
fn id(prefix: &str, suffix: &str) -> String {
    if prefix.is_empty() {
        suffix.into()
    } else {
        format!("{prefix}-{suffix}")
    }
}

/// One draft as the composer shows it.
pub struct DraftView {
    pub comment: DraftComment,
    /// Source author as the transcript shows it (disc for agents).
    pub author: QuoteAuthor,
    /// The draft's own reply input (borderless, auto-growing).
    pub input: Entity<ComposerInput>,
}

const LINE: f32 = 20.;
const GAP: f32 = 10.;
/// Space under the last draft, before the main input.
const BOTTOM: f32 = 8.;
/// The drafts scroll inside the composer beyond this height.
pub const DRAFTS_MAX_HEIGHT: f32 = 240.;
const INPUT_MIN: f32 = 20.;
const INPUT_MAX: f32 = 120.;

/// Height of one draft's reply input from its content.
pub fn draft_input_height(input: &ComposerInput) -> f32 {
    input
        .content_height()
        .unwrap_or(INPUT_MIN)
        .clamp(INPUT_MIN, INPUT_MAX)
}

/// The composer band the drafts occupy, given each input's height.
pub fn drafts_height(inputs: impl IntoIterator<Item = f32>) -> f32 {
    let mut total = 0.;
    let mut count = 0;
    for input in inputs {
        total += LINE + 2. + input;
        count += 1;
    }
    if count == 0 {
        return 0.;
    }
    (total + GAP * (count - 1) as f32).min(DRAFTS_MAX_HEIGHT) + BOTTOM
}

/// The drafts band. Hosts create each input with the "回复这段…" placeholder
/// ([`DraftText::reply_placeholder`]). `jump` scrolls to the source passage; `remove` drops the
/// draft (its passage loses the dotted underline in the transcript).
pub fn drafts<V: 'static>(
    prefix: &str,
    drafts: Vec<DraftView>,
    remove_label: &str,
    cx: &mut Context<V>,
    jump: impl Fn(&mut V, DraftComment, &mut Window, &mut Context<V>) + 'static,
    remove: impl Fn(&mut V, String, &mut Context<V>) + 'static,
) -> gpui::AnyElement {
    let jump = Rc::new(jump);
    let remove = Rc::new(remove);
    let p = ZORK_UI.palette;
    let owner = cx.entity().downgrade();
    div()
        .id(id(prefix, "composer-comment-queue"))
        .w_full()
        .max_h(px(DRAFTS_MAX_HEIGHT))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(GAP))
        .children(drafts.into_iter().map(|draft| {
            let comment = draft.comment.clone();
            let key = comment.id.clone();
            let height = draft_input_height(draft.input.read(cx));
            let input = draft.input.clone();
            let jump = jump.clone();
            let jump_owner = owner.clone();
            let jump_comment = comment.clone();
            let remove = remove.clone();
            let remove_key = key.clone();
            let close = div()
                .id(id(prefix, &format!("comment-remove-{key}")))
                .flex_shrink_0()
                .ml_auto()
                .size(px(22.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(p.subtle))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(INTERACTION.neutral_hover)).text_color(rgb(p.text)))
                .child(ui::icon("icons/x.svg", 12.))
                .on_click(cx.listener(move |v, _, _, cx| {
                    cx.stop_propagation();
                    remove(v, remove_key.clone(), cx)
                }))
                .automation(AutomationRole::Button, remove_label.to_owned());
            div()
                .id(id(prefix, &format!("queued-comment-{key}")))
                .w_full()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(identity::passage_line(
                    &id(prefix, &format!("comment-quote-{key}")),
                    &draft.author,
                    &comment.source.quote,
                    Some(Rc::new(move |window, cx| {
                        let jump = jump.clone();
                        let comment = jump_comment.clone();
                        let _ = jump_owner.update(cx, |v, cx| jump(v, comment, window, cx));
                    })),
                    Some(close.into_any_element()),
                ))
                .child(
                    div()
                        .id(id(prefix, &format!("comment-input-{key}")))
                        .w_full()
                        .h(px(height))
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .text_color(rgb(p.text))
                        .overflow_hidden()
                        .child(input.clone())
                        .on_click(move |_, window, cx| {
                            window.focus(&input.read(cx).focus_handle(), cx)
                        })
                        .automation(AutomationRole::TextInput, comment.source.quote.clone()),
                )
        }))
        .automation(AutomationRole::Status, "引用草稿")
        .into_any_element()
}

/// Wording of the draft queue, injected so clients keep their catalogs.
#[derive(Clone, Debug)]
pub struct DraftText {
    pub reply_placeholder: String,
    pub remove: String,
    pub extra_placeholder: String,
    pub reply_required: String,
    pub duplicate: String,
}
impl Default for DraftText {
    fn default() -> Self {
        Self {
            reply_placeholder: "回复这段…".into(),
            remove: "移除这段引用".into(),
            extra_placeholder: "补充说明（可选）".into(),
            reply_required: "每段引用都写一句回复，或者移除它".into(),
            duplicate: "这段已经在引用里了".into(),
        }
    }
}

/// Whether every draft has its own reply (sending is refused otherwise).
pub fn drafts_complete(comments: &[DraftComment]) -> bool {
    comments
        .iter()
        .all(|comment| !comment.comment.trim().is_empty())
}

/// Whether `source` (same message and passage) is already drafted.
pub fn already_drafted(comments: &[DraftComment], source: &crate::comments::CommentSource) -> bool {
    let quote = source.quote.trim();
    comments.iter().any(|comment| {
        comment.source.message_id == source.message_id && comment.source.quote.trim() == quote
    })
}

#[cfg(feature = "stories")]
pub struct CommentsStory {
    prefix: String,
    comments: Vec<(DraftComment, Entity<ComposerInput>)>,
    editor: Entity<Editor>,
}
#[cfg(feature = "stories")]
impl CommentsStory {
    pub fn new(prefix: String, state: &str, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| Editor::new(prefix.clone(), cx));
        let draft = |id: &str, author: &str, quote: &str, text: &str, cx: &mut Context<Self>| {
            let input = cx.new(|cx| ComposerInput::new("回复这段…", cx).multiline());
            input.update(cx, |input, cx| input.reset_value(text.to_owned(), cx));
            cx.subscribe(
                &input,
                |_, _, _: &crate::components::text_input::ComposerLayoutChanged, cx| cx.notify(),
            )
            .detach();
            (
                DraftComment {
                    id: id.into(),
                    source: crate::comments::CommentSource {
                        session_id: "mock-session".into(),
                        message_id: Some(format!("{id}-source")),
                        author: Some(author.into()),
                        quote: quote.into(),
                        ..Default::default()
                    },
                    comment: text.into(),
                },
                input,
            )
        };
        let comments = match state {
            "queued" | "editing" => vec![
                draft(
                    "d1",
                    "Planner",
                    "键盘可以走完全部流程",
                    "菜单里的 Tab 顺序也要一起测。",
                    cx,
                ),
                draft("d2", "审阅助手", "触控区重叠", "", cx),
            ],
            _ => vec![],
        };
        Self {
            prefix,
            comments,
            editor,
        }
    }
}
#[cfg(feature = "stories")]
impl gpui::Render for CommentsStory {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let views = self
            .comments
            .iter()
            .map(|(comment, input)| DraftView {
                comment: comment.clone(),
                author: QuoteAuthor {
                    name: comment.source.author.clone().unwrap_or_default(),
                    disc: None,
                },
                input: input.clone(),
            })
            .collect();
        let queue = drafts(
            &self.prefix,
            views,
            "移除这段引用",
            cx,
            |_, _, _, _| {},
            |v, id, cx| {
                v.comments.retain(|(comment, _)| comment.id != id);
                cx.notify();
            },
        );
        div()
            .p_4()
            .w(px(520.))
            .rounded(px(24.))
            .bg(rgb(ZORK_UI.thread.user_fill))
            .flex()
            .flex_col()
            .gap_4()
            .child(queue)
            .child(self.editor.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drafts_band_grows_then_scrolls() {
        assert_eq!(drafts_height([]), 0.);
        assert_eq!(drafts_height([20.]), 20. + 2. + 20. + BOTTOM);
        assert_eq!(drafts_height([20.; 10]), DRAFTS_MAX_HEIGHT + BOTTOM);
    }

    #[test]
    fn duplicates_and_empty_replies_are_detected() {
        let source = crate::comments::CommentSource {
            message_id: Some("m1".into()),
            quote: "触控区重叠".into(),
            ..Default::default()
        };
        let draft = DraftComment {
            id: "d".into(),
            source: source.clone(),
            comment: " ".into(),
        };
        assert!(already_drafted(std::slice::from_ref(&draft), &source));
        assert!(!drafts_complete(std::slice::from_ref(&draft)));
        let other = crate::comments::CommentSource {
            message_id: Some("m2".into()),
            ..source
        };
        assert!(!already_drafted(&[draft], &other));
    }
}
