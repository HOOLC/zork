//! Shared GPUI component implementation for the desktop app and native design examples.
//! Applications provide data and actions; this package does not own Station, persistence or account access.
pub mod assets;
pub mod automation;
pub mod comments;
pub mod components;
pub mod controls;
pub mod design;
pub mod history;
pub mod modal;
pub mod navigation;
pub mod network;
pub mod onboarding;
pub mod settings;

#[cfg(feature = "stories")]
mod form_story;
#[cfg(feature = "stories")]
mod interaction_story;
#[cfg(feature = "stories")]
pub mod liquid_story;
#[cfg(feature = "stories")]
pub mod stories;

pub mod resources;

pub mod shared_files;

pub mod chat_navigation;

pub mod conversation_contents;

pub mod attachment_viewer;

pub mod browser_chrome;

pub mod history_details;

pub mod member_activity;

pub mod history_page;

pub mod new_chat;
pub mod welcome;

pub mod device_name;
pub mod node_directory;

pub mod conversation_toolbar;
