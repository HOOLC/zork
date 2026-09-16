//! Platform aliases for the library's actions. Editing semantics stay upstream.
use gpui::{actions, App, KeyBinding};
use gpui_base::input;

actions!(composer_input, [PastePlain, FocusNext, FocusPrevious]);

fn mac_keyboard() -> bool {
    #[cfg(not(target_family = "wasm"))]
    {
        cfg!(target_os = "macos")
    }
    #[cfg(target_family = "wasm")]
    {
        js_sys::Reflect::get(&js_sys::global(), &"navigator".into())
            .ok()
            .is_some_and(|navigator| {
                js_sys::Reflect::get(&navigator, &"platform".into())
                    .ok()
                    .and_then(|v| v.as_string())
                    .is_some_and(|p| ["Mac", "iPhone", "iPad"].iter().any(|os| p.contains(os)))
            })
    }
}

pub fn init(cx: &mut App) {
    gpui_base::init(cx);
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
    #[cfg(not(target_family = "wasm"))]
    cx.bind_keys([
        KeyBinding::new(&format!("{command}-v"), input::Paste, scope),
        KeyBinding::new(&format!("{command}-shift-v"), PastePlain, scope),
    ]);
    // The browser owns the paste gesture and delivers its payload to the input
    // handler. The library's synchronous clipboard binding would swallow it.
    #[cfg(target_family = "wasm")]
    for key in [
        "cmd-v",
        "ctrl-v",
        "cmd-shift-v",
        "ctrl-shift-v",
        "cmd-alt-shift-v",
    ] {
        cx.bind_keys([KeyBinding::new(key, gpui::NoAction, scope)]);
    }
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
        #[cfg(not(target_family = "wasm"))]
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
        #[cfg(not(target_family = "wasm"))]
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
