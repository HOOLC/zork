//! Application styling and draft events around gpui-base's unstyled editors.
//! Text, selection, layout, IME and undo history belong to the library state.

use gpui::{
    div, prelude::*, rgba, App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
    Focusable, SharedString, Subscription, Window,
};
use gpui_base::input::{
    self, InputBaseState, InputEvent, InputModeKind, InputState, TextareaState,
};

// Both facades use the same public editing API while retaining their own modes.
macro_rules! edit {
    ($this:expr, $cx:expr, |$state:ident, $state_cx:ident| $body:expr) => {
        match ($this)
            .editor
            .as_ref()
            .expect("editor prepared before input")
            .clone()
        {
            Editor::Single(entity) => entity.update($cx, |$state, $state_cx| $body),
            Editor::Multi(entity) => entity.update($cx, |$state, $state_cx| $body),
        }
    };
}

mod bridge;
mod shortcuts;
#[cfg(all(test, feature = "headless-bench"))]
mod tests;

pub use gpui_base::input::{
    Copy as CopyText, Cut as CutText, Paste as PasteText, Redo as RedoText,
    SelectAll as SelectAllText, Undo as UndoText,
};
pub use shortcuts::init;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComposerSubmit;
/// Platform clipboard payload. The host imports it through client core.
pub struct ComposerFilesPasted(pub ClipboardItem);
/// Committed text changed; cursor, selection and preedit remain local.
pub struct ComposerEdited;
pub struct ComposerLayoutChanged;

#[derive(Clone)]
enum Editor {
    Single(Entity<InputState>),
    Multi(Entity<TextareaState>),
}

impl Editor {
    fn value(&self, cx: &App) -> SharedString {
        match self {
            Self::Single(state) => state.read(cx).value(),
            Self::Multi(state) => state.read(cx).value(),
        }
    }

    fn focus(&self, cx: &App) -> FocusHandle {
        match self {
            Self::Single(state) => state.read(cx).focus_handle(cx),
            Self::Multi(state) => state.read(cx).focus_handle(cx),
        }
    }
}

pub struct ComposerInput {
    editor: Option<Editor>,
    focus_handle: FocusHandle,
    placeholder: SharedString,
    // Read-only text projection for the existing presentation/draft API. It is
    // refreshed on edits, never on caret blinks or selection notifications.
    value: SharedString,
    pending_reset: bool,
    pending_configuration: bool,
    single_line: bool,
    submit_on_enter: bool,
    secret: bool,
    disabled: bool,
    readonly: bool,
    code_length: Option<usize>,
    observed_selection: Option<std::ops::Range<usize>>,
    ime_active: bool,
    content_height: Option<f32>,
    subscriptions: Vec<Subscription>,
}

impl ComposerInput {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            editor: None,
            focus_handle: cx.focus_handle(),
            placeholder: placeholder.into(),
            value: SharedString::default(),
            pending_reset: true,
            pending_configuration: true,
            single_line: false,
            submit_on_enter: true,
            secret: false,
            disabled: false,
            readonly: false,
            code_length: None,
            observed_selection: None,
            ime_active: false,
            content_height: None,
            subscriptions: Vec::new(),
        }
    }

    pub fn single_line(mut self) -> Self {
        self.single_line = true;
        self
    }

    /// Standalone multiline fields insert a newline; chat composers retain
    /// their submit binding unless the caller explicitly selects this mode.
    pub fn multiline(mut self) -> Self {
        self.single_line = false;
        self.submit_on_enter = false;
        self
    }

    /// A digit-slot presentation of this editor. This only constrains the input
    /// alphabet and length; code verification remains the consuming core's job.
    pub fn numeric_code(mut self, length: usize) -> Self {
        self.single_line = true;
        self.code_length = Some(length.clamp(1, 12));
        self
    }

    pub fn selection(&self, cx: &App) -> std::ops::Range<usize> {
        match &self.editor {
            Some(Editor::Single(state)) => state.read(cx).selected_range(),
            Some(Editor::Multi(state)) => state.read(cx).selected_range(),
            None => self.value.len()..self.value.len(),
        }
    }

    pub fn select(
        &mut self,
        range: std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prepare(window, cx);
        let boundary = |offset: usize| {
            let mut end = offset.min(self.value.len());
            while !self.value.is_char_boundary(end) {
                end -= 1;
            }
            end
        };
        let start = boundary(range.start);
        let end = boundary(range.end).max(start);
        edit!(self, cx, |state, scx| state
            .set_selected_range(start..end, scx));
        cx.notify();
    }

    pub(crate) fn is_disabled(&self) -> bool {
        self.disabled
    }

    pub fn set_editable(&mut self, disabled: bool, readonly: bool, cx: &mut Context<Self>) {
        if (self.disabled, self.readonly) != (disabled, readonly) {
            self.disabled = disabled;
            self.readonly = readonly;
            self.pending_configuration = true;
            cx.notify();
        }
    }

    fn code_text(&self, text: &str, replaced: usize) -> Option<String> {
        let Some(length) = self.code_length else {
            return Some(text.to_owned());
        };
        let available = length.saturating_sub(self.value.len().saturating_sub(replaced));
        let normalized: String = text
            .chars()
            .filter_map(|c| {
                if c.is_ascii_digit() {
                    Some(c)
                } else {
                    (c as u32)
                        .checked_sub('０' as u32)
                        .and_then(|digit| char::from_digit(digit, 10))
                }
            })
            .take(available)
            .collect();
        (text.is_empty() || !normalized.is_empty()).then_some(normalized)
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    /// Last measured height using the library's actual font and wrapping width.
    /// Keep it until the next layout so typing never collapses a tall composer.
    pub fn content_height(&self) -> Option<f32> {
        self.content_height
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    pub fn set_placeholder(&mut self, value: impl Into<SharedString>, cx: &mut Context<Self>) {
        let value = value.into();
        if self.placeholder != value {
            self.placeholder = value;
            self.pending_configuration = true;
            cx.notify();
        }
    }

    pub fn set_secret(&mut self, secret: bool, cx: &mut Context<Self>) {
        // Passwords retain single-line editing when their contents are revealed.
        self.single_line |= secret;
        self.secret = secret;
        self.pending_configuration = true;
        cx.notify();
    }

    pub fn set_value(&mut self, value: impl Into<String>, cx: &mut Context<Self>) {
        let value = value.into();
        if self.value.as_ref() != value {
            self.reset_value(value, cx);
        }
    }

    /// Restoring another draft clears editing history even for identical text.
    pub fn reset_value(&mut self, value: impl Into<String>, cx: &mut Context<Self>) {
        self.value = value.into().into();
        self.pending_reset = true;
        self.ime_active = false;
        cx.emit(ComposerLayoutChanged);
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        let had_text = !self.value.is_empty();
        self.reset_value(String::new(), cx);
        if had_text {
            cx.emit(ComposerEdited);
        }
    }

    fn subscribe<M: InputModeKind>(
        &mut self,
        state: &Entity<InputBaseState<M>>,
        cx: &mut Context<Self>,
    ) {
        self.subscriptions
            .push(
                cx.subscribe(state, |this, state, event: &InputEvent, cx| match event {
                    InputEvent::Change => {
                        this.value = state.read(cx).value();
                        this.ime_active = false;
                        cx.emit(ComposerEdited);
                        cx.emit(ComposerLayoutChanged);
                        cx.notify();
                    }
                    InputEvent::PressEnter { shift: false, .. } if !this.ime_active => {
                        cx.emit(ComposerSubmit);
                    }
                    _ => {}
                }),
            );
    }

    // Existing views create their field entities before a Window is available.
    // Only initialization/configuration is deferred; platform input and action
    // capture also apply pending projections before touching the editing state.
    fn prepare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let needs_editor = self
            .editor
            .as_ref()
            .is_none_or(|editor| matches!(editor, Editor::Single(_)) != self.single_line);
        if needs_editor {
            let focused = self.focus_handle.is_focused(window);
            self.subscriptions.clear();
            let editor = if self.single_line {
                let state = cx.new(|cx| {
                    InputState::new(window, cx)
                        .context_menu(false)
                        .scrollbar(false)
                });
                self.subscribe(&state, cx);
                if self.code_length.is_some() {
                    self.subscriptions
                        .push(cx.observe(&state, |this, state, cx| {
                            let selection = state.read(cx).selected_range();
                            if this.observed_selection.as_ref() != Some(&selection) {
                                this.observed_selection = Some(selection);
                                cx.notify();
                            }
                        }));
                }
                Editor::Single(state)
            } else {
                let state = cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .rows(1)
                        .submit_on_enter(self.submit_on_enter)
                        .context_menu(false)
                        .scrollbar(false)
                        .scroll_beyond_last_line(Some(0))
                        .cursor_surrounding_lines(Some(0))
                });
                self.subscribe(&state, cx);
                Editor::Multi(state)
            };
            self.focus_handle = editor.focus(cx);
            self.editor = Some(editor);
            self.pending_configuration = true;
            self.pending_reset = true;
            if focused {
                window.focus(&self.focus_handle, cx);
            }
        }
        if self.pending_configuration {
            self.pending_configuration = false;
            let placeholder = self.placeholder.clone();
            edit!(self, cx, |state, scx| state.set_placeholder(
                placeholder,
                window,
                scx
            ));
            if let Some(Editor::Single(state)) = &self.editor {
                state.update(cx, |state, scx| {
                    state.set_masked(self.secret, window, scx);
                    if let Some(length) = self.code_length {
                        state.set_mask_pattern(
                            gpui_base::input::MaskPattern::new(&"9".repeat(length)),
                            window,
                            scx,
                        );
                    }
                });
            }
            edit!(self, cx, |state, scx| {
                state.set_disabled(self.disabled, scx);
                state.set_readonly(self.readonly, scx);
            });
            self.focus_handle = self.focus_handle.clone().tab_stop(!self.disabled);
            if self.disabled && self.focus_handle.is_focused(window) {
                window.blur();
            }
        }
        if self.pending_reset {
            self.pending_reset = false;
            let value = self.value.clone();
            edit!(self, cx, |state, scx| {
                state.set_value(value, window, scx);
                let end = state.text().len();
                state.set_selected_range(end..end, scx);
            });
            self.value = self.editor.as_ref().unwrap().value(cx);
        }
    }

    fn sync_preedit(&mut self, cx: &mut Context<Self>) {
        self.value = self.editor.as_ref().unwrap().value(cx);
        cx.emit(ComposerLayoutChanged);
        cx.notify();
    }

    fn paste_item(
        &mut self,
        item: ClipboardItem,
        plain: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prepare(window, cx);
        if self.disabled || self.readonly {
            return;
        }
        if !plain
            && item.entries().iter().any(|entry| {
                matches!(
                    entry,
                    gpui::ClipboardEntry::Image(_) | gpui::ClipboardEntry::ExternalPaths(_)
                )
            })
        {
            cx.emit(ComposerFilesPasted(item));
        } else if let Some(text) = item.text() {
            let text = if self.single_line {
                text.replace("\r\n", " ").replace(['\n', '\r'], " ")
            } else {
                text
            };
            let replaced = self.selection(cx).len();
            if let Some(text) = self.code_text(&text, replaced) {
                edit!(self, cx, |state, scx| state.replace(text, window, scx));
            }
        }
    }

    fn capture_paste(&mut self, _: &input::Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = cx.read_from_clipboard() {
            self.paste_item(item, false, window, cx);
        }
        cx.stop_propagation();
    }

    fn paste_plain(
        &mut self,
        _: &shortcuts::PastePlain,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(item) = cx.read_from_clipboard() {
            self.paste_item(item, true, window, cx);
        }
    }

    fn capture_edit<A: gpui::Action>(
        &mut self,
        _: &A,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prepare(window, cx);
        if self.ime_active {
            cx.stop_propagation();
        } else {
            cx.propagate();
        }
    }
}

impl EventEmitter<ComposerSubmit> for ComposerInput {}
impl EventEmitter<ComposerFilesPasted> for ComposerInput {}
impl EventEmitter<ComposerEdited> for ComposerInput {}
impl EventEmitter<ComposerLayoutChanged> for ComposerInput {}

impl Focusable for ComposerInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ComposerInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.prepare(window, cx);
        let color = window.text_style().color;
        let style = input::InputEditorStyle {
            foreground: color,
            caret: color,
            muted_foreground: gpui::rgb(crate::design::ZORK_UI.palette.subtle).into(),
            selection: rgba(0x339CFF30).into(),
            ..Default::default()
        };
        edit!(self, cx, |state, _scx| state.set_editor_style(style));
        let child = match self.editor.as_ref().unwrap() {
            Editor::Single(state) => input::Input::new(state).into_any_element(),
            Editor::Multi(state) => input::Textarea::new(state).into_any_element(),
        };
        let mut element = div().size_full().flex().relative().overflow_hidden()
            .key_context("ComposerInput")
            .capture_key_down(cx.listener(|view, _, window, cx| view.prepare(window, cx)))
            .capture_action(cx.listener(Self::capture_paste))
            // Submission is delivered through ComposerSubmit. Consume the
            // library's propagated Enter so another matching binding cannot
            // submit the same draft again.
            .on_action(cx.listener(|_, _: &input::Enter, _, _| {}))
            .on_action(cx.listener(Self::paste_plain));
        element = element
            .on_action(cx.listener(|view, _: &shortcuts::FocusNext, window, cx| {
                if !view.ime_active {
                    crate::navigation::move_focus(false, window, cx);
                }
            }))
            .on_action(
                cx.listener(|view, _: &shortcuts::FocusPrevious, window, cx| {
                    if !view.ime_active {
                        crate::navigation::move_focus(true, window, cx);
                    }
                }),
            );
        element = element.on_action(cx.listener(
            |view, action: &input::PlatformEdit, window, cx| {
                view.prepare(window, cx);
                if !view.ime_active {
                    edit!(view, cx, |state, scx| state
                        .platform_edit(action, window, scx));
                }
            },
        ));
        macro_rules! guard {
            ($($action:ty),* $(,)?) => { $(
                element = element.capture_action(cx.listener(Self::capture_edit::<$action>));
            )* };
        }
        guard!(
            input::Enter,
            input::Escape,
            input::MovePageUp,
            input::MovePageDown,
            input::Indent,
            input::Outdent,
            input::IndentInline,
            input::OutdentInline,
            input::Backspace,
            input::Delete,
            input::Cut,
            input::Undo,
            input::Redo,
            input::SelectAll,
            input::MoveLeft,
            input::MoveRight,
            input::MoveUp,
            input::MoveDown,
            input::MoveHome,
            input::MoveEnd,
            input::MoveToStart,
            input::MoveToEnd,
            input::MoveToPreviousWord,
            input::MoveToNextWord,
            input::SelectToStartOfLine,
            input::SelectToEndOfLine,
            input::SelectToStart,
            input::SelectToEnd,
            input::SelectToPreviousWordStart,
            input::SelectToNextWordEnd,
            input::DeleteToBeginningOfLine,
            input::DeleteToEndOfLine,
            input::DeleteToPreviousWordStart,
            input::DeleteToNextWordEnd,
            gpui_base::actions::SelectLeft,
            gpui_base::actions::SelectRight,
            gpui_base::actions::SelectUp,
            gpui_base::actions::SelectDown
        );
        element
            .child(child)
            .child(bridge::InputBridge { input: cx.entity() })
    }
}
