use gpui::{App, Window};

pub use crate::components::liquid::navigation::{Group as TabGroup, GroupSurface as TabSurface};
pub const TAB_GAP: f32 = 2.;

gpui::actions!(focus_scope, [Next, Previous]);

/// Editors validate IME state before requesting traversal. A surrounding
/// scope can handle the request before falling back to window-wide tab order.
pub fn move_focus(backwards: bool, window: &mut Window, cx: &mut App) {
    let action: Box<dyn gpui::Action> = if backwards {
        Box::new(Previous)
    } else {
        Box::new(Next)
    };
    if window.is_action_available(action.as_ref(), cx) {
        if let Some(focused) = window.focused(cx) {
            focused.dispatch_action(action.as_ref(), window, cx);
            return;
        }
    }
    crate::modal::advance_focus(backwards, window, cx);
}

/// GPUI tab traversal is explicit; consume only Tab and preserve activation keys.
pub fn keyboard_navigation(
    event: &gpui::KeyDownEvent,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) {
    if event.keystroke.key == "tab"
        && !event.keystroke.modifiers.control
        && !event.keystroke.modifiers.platform
        && !event.keystroke.modifiers.alt
    {
        move_focus(event.keystroke.modifiers.shift, window, cx);
        cx.stop_propagation();
    }
}
