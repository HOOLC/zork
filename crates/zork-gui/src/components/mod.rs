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
    cx.set_reduce_motion(zork_client_core::desktop::reduced_motion());
}

pub mod selection;

mod transcript_cache;
