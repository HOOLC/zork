//! Small GPUI components owned by zork-gui.

pub mod activity;
pub mod brand;
pub mod interaction;
pub mod message;
pub mod text_input;

use gpui::App;

/// Register component-scoped key bindings once during application startup.
pub fn init(cx: &mut App) {
    zork_ui::components::init(cx);
    #[cfg(target_os = "macos")]
    {
        // The user's preference turns motion into short fades; tests and
        // stills that force `reduce_motion` later get static end states.
        let reduced = zork_client_core::desktop::reduced_motion();
        zork_ui::motion::set_user_reduced(reduced);
        cx.set_reduce_motion(reduced);
    }
}

pub mod selection;

mod transcript_cache;
