//! zork-gui: a GPUI desktop client for zork-station's local IM entry.
//!
//! The transcript contains only station-delivered user/assistant messages.
//! Agent tools, waits, deltas, and transcript text remain internal; activity
//! reaches the UI separately through station-projected status events.

pub mod api;
pub mod assets;
pub mod automation;
pub mod browser;
pub mod comments;
pub mod components;
pub mod design;
pub mod desktop;
pub mod transcript;
pub mod views;
pub mod window_chrome;

pub mod shell;

pub mod i18n;

pub mod session_history;
