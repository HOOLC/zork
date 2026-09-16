//! Supplemental platform commands using the shared buffer and undo manager.
use super::{InputBaseState, InputModeKind, undo_manager::EditIntent};
use gpui::{Context, EntityInputHandler, Global, Window, px};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlatformCommand {
    ParagraphStart,
    ParagraphEnd,
    NextWordStart,
    DeleteNextWordStart,
    PageUp,
    PageDown,
    ScrollStart,
    ScrollEnd,
    ScrollPageUp,
    ScrollPageDown,
    KillLine,
    Yank,
    Transpose,
    OpenLine,
}

#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = input, no_json)]
pub struct PlatformEdit {
    pub command: PlatformCommand,
    pub select: bool,
}

struct KilledText(String);
impl Global for KilledText {}

impl<M: InputModeKind> InputBaseState<M> {
    pub fn platform_edit(
        &mut self,
        action: &PlatformEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use PlatformCommand::*;
        if self.disabled || self.ime_marked_range.is_some() {
            return;
        }
        match action.command {
            ScrollStart | ScrollEnd | ScrollPageUp | ScrollPageDown => {
                let mut offset = self.scroll_offset();
                let height = self.input_bounds().size.height;
                offset.y = match action.command {
                    ScrollStart => px(0.),
                    ScrollEnd => -(self.content_height().unwrap_or(height) - height).max(px(0.)),
                    ScrollPageUp => offset.y + height,
                    _ => offset.y - height,
                };
                self.set_scroll_offset(offset, cx);
                return;
            }
            PageUp | PageDown => {
                let rows = (self.input_bounds().size.height / self.line_height().unwrap_or(px(20.)))
                    .max(1.) as isize;
                let rows = if action.command == PageUp {
                    -rows
                } else {
                    rows
                };
                if action.select {
                    self.select_vertical(rows, window, cx);
                } else {
                    self.move_vertical(rows, window, cx);
                }
                return;
            }
            _ => {}
        }
        let text = self.text.to_string();
        let cursor = self.cursor();
        let line_start = |at: usize| text[..at].rfind('\n').map_or(0, |i| i + 1);
        let line_end = |at: usize| text[at..].find('\n').map_or(text.len(), |i| at + i);
        let next_word_start = || {
            text.split_word_bound_indices()
                .find(|(start, word)| *start > cursor && !word.trim().is_empty())
                .map_or(text.len(), |(start, _)| start)
        };
        let target = match action.command {
            ParagraphStart => {
                let start = line_start(cursor);
                Some(if start == cursor && start > 0 {
                    line_start(start - 1)
                } else {
                    start
                })
            }
            ParagraphEnd => {
                let end = line_end(cursor);
                Some(if end == cursor && end < text.len() {
                    line_end(end + 1)
                } else {
                    end
                })
            }
            NextWordStart => Some(next_word_start()),
            _ => None,
        };
        if let Some(target) = target {
            self.undo_manager.break_transaction_coalescing();
            if action.select {
                self.select_to(target, cx);
            } else {
                self.move_to(target, None, cx);
            }
            self.update_preferred_column();
            return;
        }
        if !self.is_editable() {
            return;
        }
        let selection = self.selected_range();
        let (range, replacement, stay) = match action.command {
            DeleteNextWordStart => (
                if selection.is_empty() {
                    cursor..next_word_start()
                } else {
                    selection
                },
                String::new(),
                false,
            ),
            KillLine => {
                let range = if selection.is_empty() {
                    let end = line_end(cursor);
                    cursor..if end == cursor {
                        self.next_boundary(cursor)
                    } else {
                        end
                    }
                } else {
                    selection
                };
                if range.is_empty() {
                    return;
                }
                cx.set_global(KilledText(text[range.clone()].to_owned()));
                (range, String::new(), false)
            }
            Yank => {
                let Some(value) = cx.try_global::<KilledText>().map(|v| v.0.clone()) else {
                    return;
                };
                (selection, value, false)
            }
            Transpose => {
                if !selection.is_empty() {
                    return;
                }
                let middle = if cursor == line_end(cursor) {
                    self.previous_boundary(cursor)
                } else {
                    cursor
                };
                let start = self.previous_boundary(middle);
                let end = self.next_boundary(middle);
                if start == middle || text[start..end].contains('\n') {
                    return;
                }
                (
                    start..end,
                    format!("{}{}", &text[middle..end], &text[start..middle]),
                    false,
                )
            }
            OpenLine if !self.is_single_line() => (selection, "\n".into(), true),
            _ => return,
        };
        if range.is_empty() && replacement.is_empty() {
            return;
        }
        self.undo_manager.break_transaction_coalescing();
        self.undo_manager.begin_transaction();
        self.undo_manager.pending_intent = Some(EditIntent::Atomic);
        let utf16 = self.range_to_utf16(&range);
        self.replace_text_in_range(Some(utf16), &replacement, window, cx);
        if stay {
            self.selected_range = (range.start..range.start).into();
            self.selection_reversed = false;
            self.undo_manager
                .set_pending_selection_after(self.selected_range);
        }
        self.undo_manager.commit_transaction();
    }
}
