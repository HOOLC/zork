//! Preserve application IME/clipboard events while the library owns editing.
use super::*;
use gpui::{
    relative, size, Bounds, Element, ElementId, ElementInputHandler, EntityInputHandler,
    GlobalElementId, LayoutId, Pixels, Point, Position, Style, UTF16Selection,
};
use std::ops::Range;

impl EntityInputHandler for ComposerInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        self.prepare(window, cx);
        edit!(self, cx, |state, scx| state
            .text_for_range(range, actual, window, scx))
    }

    fn selected_text_range(
        &mut self,
        ignore: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        self.prepare(window, cx);
        edit!(self, cx, |state, scx| {
            let mut selection = state.selected_text_range(ignore, window, scx)?;
            let range = state.selected_range();
            selection.reversed = !range.is_empty() && state.cursor() == range.start;
            Some(selection)
        })
    }

    fn marked_text_range(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.editor.as_ref()?;
        edit!(self, cx, |state, scx| state.marked_text_range(window, scx))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prepare(window, cx);
        let marked = self.marked_text_range(window, cx).is_some();
        edit!(self, cx, |state, scx| {
            state.unmark_text(window, scx);
            scx.notify();
        });
        self.ime_active = false;
        self.sync_preedit(cx);
        // The library commits undo history on unmark without emitting Change.
        if marked {
            cx.emit(ComposerEdited);
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prepare(window, cx);
        if self.disabled || self.readonly {
            return;
        }
        let replaced = range
            .as_ref()
            .map_or_else(|| self.selection(cx).len(), |range| range.len());
        let Some(text) = self.code_text(text, replaced) else {
            return;
        };
        edit!(self, cx, |state, scx| state
            .replace_text_in_range(range, &text, window, scx));
        self.ime_active = false;
        self.value = self.editor.as_ref().unwrap().value(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prepare(window, cx);
        if self.disabled || self.readonly {
            return;
        }
        edit!(self, cx, |state, scx| state.replace_and_mark_text_in_range(
            range, text, selected, window, scx
        ));
        self.ime_active = self.marked_text_range(window, cx).is_some();
        self.sync_preedit(cx);
    }

    fn paste(&mut self, item: ClipboardItem, window: &mut Window, cx: &mut Context<Self>) {
        self.paste_item(item, false, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.prepare(window, cx);
        edit!(self, cx, |state, scx| state
            .bounds_for_range(range, bounds, window, scx))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        self.prepare(window, cx);
        edit!(self, cx, |state, scx| state
            .character_index_for_point(point, window, scx))
    }

    fn set_selected_text_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use input::RopeExt;
        self.prepare(window, cx);
        edit!(self, cx, |state, scx| {
            let range = state.text().offset_utf16_to_offset(range.start)
                ..state.text().offset_utf16_to_offset(range.end);
            state.set_selected_range(range, scx);
        });
    }

    fn text_length_utf16(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<usize> {
        self.prepare(window, cx);
        Some(edit!(self, cx, |state, _scx| state.text().len_utf16()))
    }
}

// Installed after the library's text element so native and browser input both
// pass through the same committed/preedit and attachment event adapter.
pub(super) struct InputBridge {
    pub input: Entity<ComposerInput>,
}

impl IntoElement for InputBridge {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for InputBridge {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.position = Position::Absolute;
        style.inset.left = gpui::px(0.).into();
        style.inset.top = gpui::px(0.).into();
        style.size = size(relative(1.).into(), relative(1.).into());
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus_handle();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        self.input.update(cx, |view, cx| {
            let height = edit!(view, cx, |state, _scx| state
                .content_height()
                .map(f32::from));
            if view.content_height != height {
                view.content_height = height;
                cx.emit(ComposerLayoutChanged);
                cx.notify();
            }
        });
    }
}
