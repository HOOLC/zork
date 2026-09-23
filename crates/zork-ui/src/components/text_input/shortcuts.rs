//! Platform aliases for the library's actions. Editing semantics stay upstream.
use gpui::{actions, App, KeyBinding};
use gpui_base::input;

actions!(composer_input, [PastePlain, FocusNext, FocusPrevious]);

fn mac_keyboard() -> bool {
    cfg!(target_os = "macos")
}

pub fn init(cx: &mut App) {
    gpui_component::init(cx);
    crate::design::init_component_theme(cx);
    bind_keys(cx, mac_keyboard());
}

pub(super) fn bind_keys(cx: &mut App, mac: bool) {
    let scope = Some("ComposerInput > Input");
    let command = if mac { "cmd" } else { "ctrl" };
    cx.bind_keys([
        KeyBinding::new("tab", FocusNext, scope),
        KeyBinding::new("shift-tab", FocusPrevious, scope),
        KeyBinding::new(&format!("{command}-a"), input::SelectAll, scope),
        KeyBinding::new(&format!("{command}-c"), input::Copy, scope),
        KeyBinding::new(&format!("{command}-x"), input::Cut, scope),
        KeyBinding::new(&format!("{command}-z"), input::Undo, scope),
        KeyBinding::new(&format!("{command}-shift-z"), input::Redo, scope),
        KeyBinding::new(
            &format!("{command}-enter"),
            input::Enter {
                secondary: true,
                shift: false,
            },
            scope,
        ),
    ]);
    cx.bind_keys([
        KeyBinding::new(&format!("{command}-v"), input::Paste, scope),
        KeyBinding::new(&format!("{command}-shift-v"), PastePlain, scope),
    ]);
    if mac {
        cx.bind_keys([
            KeyBinding::new("ctrl-cmd-space", input::ShowCharacterPalette, scope),
            KeyBinding::new("cmd-left", input::MoveHome, scope),
            KeyBinding::new("cmd-right", input::MoveEnd, scope),
            KeyBinding::new("cmd-up", input::MoveToStart, scope),
            KeyBinding::new("cmd-down", input::MoveToEnd, scope),
            KeyBinding::new("cmd-shift-left", input::SelectToStartOfLine, scope),
            KeyBinding::new("cmd-shift-right", input::SelectToEndOfLine, scope),
            KeyBinding::new("cmd-shift-up", input::SelectToStart, scope),
            KeyBinding::new("cmd-shift-down", input::SelectToEnd, scope),
            KeyBinding::new("alt-left", input::MoveToPreviousWord, scope),
            KeyBinding::new("alt-right", input::MoveToNextWord, scope),
            KeyBinding::new("alt-shift-left", input::SelectToPreviousWordStart, scope),
            KeyBinding::new("alt-shift-right", input::SelectToNextWordEnd, scope),
            KeyBinding::new("alt-backspace", input::DeleteToPreviousWordStart, scope),
            KeyBinding::new("alt-delete", input::DeleteToNextWordEnd, scope),
            KeyBinding::new("cmd-backspace", input::DeleteToBeginningOfLine, scope),
            KeyBinding::new("cmd-delete", input::DeleteToEndOfLine, scope),
            KeyBinding::new("ctrl-a", input::MoveHome, scope),
            KeyBinding::new("ctrl-e", input::MoveEnd, scope),
            KeyBinding::new("ctrl-b", input::MoveLeft, scope),
            KeyBinding::new("ctrl-f", input::MoveRight, scope),
            KeyBinding::new("ctrl-p", input::MoveUp, scope),
            KeyBinding::new("ctrl-n", input::MoveDown, scope),
            KeyBinding::new("ctrl-h", input::Backspace, scope),
            KeyBinding::new("ctrl-d", input::Delete, scope),
            KeyBinding::new("ctrl-shift-a", input::SelectToStartOfLine, scope),
            KeyBinding::new("ctrl-shift-e", input::SelectToEndOfLine, scope),
            KeyBinding::new("ctrl-shift-b", gpui_base::actions::SelectLeft, scope),
            KeyBinding::new("ctrl-shift-f", gpui_base::actions::SelectRight, scope),
            KeyBinding::new("ctrl-shift-p", gpui_base::actions::SelectUp, scope),
            KeyBinding::new("ctrl-shift-n", gpui_base::actions::SelectDown, scope),
            KeyBinding::new(
                "alt-enter",
                input::Enter {
                    secondary: false,
                    shift: true,
                },
                scope,
            ),
        ]);
        cx.bind_keys([KeyBinding::new("cmd-alt-shift-v", PastePlain, scope)]);
    } else {
        cx.bind_keys([
            KeyBinding::new("ctrl-insert", input::Copy, scope),
            KeyBinding::new("shift-delete", input::Cut, scope),
            KeyBinding::new("ctrl-home", input::MoveToStart, scope),
            KeyBinding::new("ctrl-end", input::MoveToEnd, scope),
            KeyBinding::new("ctrl-shift-home", input::SelectToStart, scope),
            KeyBinding::new("ctrl-shift-end", input::SelectToEnd, scope),
            KeyBinding::new("ctrl-left", input::MoveToPreviousWord, scope),
            KeyBinding::new("ctrl-right", input::MoveToNextWord, scope),
            KeyBinding::new("ctrl-shift-left", input::SelectToPreviousWordStart, scope),
            KeyBinding::new("ctrl-shift-right", input::SelectToNextWordEnd, scope),
            KeyBinding::new("ctrl-backspace", input::DeleteToPreviousWordStart, scope),
            KeyBinding::new("ctrl-delete", input::DeleteToNextWordEnd, scope),
            KeyBinding::new("ctrl-y", input::Redo, scope),
        ]);
        cx.bind_keys([KeyBinding::new("shift-insert", input::Paste, scope)]);
    }
    use input::PlatformCommand::*;
    let paragraph = if mac { "alt" } else { "ctrl" };
    for (key, command) in [
        (format!("{paragraph}-up"), ParagraphStart),
        (format!("{paragraph}-down"), ParagraphEnd),
    ] {
        for select in [false, true] {
            let key = if select {
                format!("shift-{key}")
            } else {
                key.clone()
            };
            cx.bind_keys([KeyBinding::new(
                &key,
                input::PlatformEdit { command, select },
                scope,
            )]);
        }
    }
    for (key, command, select) in if mac {
        vec![
            ("home", ScrollStart, false),
            ("end", ScrollEnd, false),
            ("pageup", ScrollPageUp, false),
            ("pagedown", ScrollPageDown, false),
            ("shift-pageup", PageUp, true),
            ("shift-pagedown", PageDown, true),
            ("ctrl-k", KillLine, false),
            ("ctrl-y", Yank, false),
            ("ctrl-t", Transpose, false),
            ("ctrl-o", OpenLine, false),
        ]
    } else {
        vec![
            ("ctrl-right", NextWordStart, false),
            ("ctrl-shift-right", NextWordStart, true),
            ("ctrl-delete", DeleteNextWordStart, false),
            ("shift-pageup", PageUp, true),
            ("shift-pagedown", PageDown, true),
        ]
    } {
        cx.bind_keys([KeyBinding::new(
            key,
            input::PlatformEdit { command, select },
            scope,
        )]);
    }
    if mac {
        cx.bind_keys([
            KeyBinding::new("shift-home", input::SelectToStart, scope),
            KeyBinding::new("shift-end", input::SelectToEnd, scope),
        ]);
    }
}
