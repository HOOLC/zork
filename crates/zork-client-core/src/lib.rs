//! Client-owned identity, offline state and bounded Station operations.
//! The platform reports host visibility and service lifetime; core decides when
//! to retain or release connections according to the user's background preference.
pub mod activity;
pub mod adb;
pub mod api;
pub mod chat_files;
pub mod composer;
pub mod conversation;
pub mod data_reset;
pub use zork_client_types::{comments, files};
pub use zork_config::channel;
pub mod agent_edit;
mod catalog;
mod client_directory;
pub mod delivery;
#[cfg(feature = "desktop")]
pub mod desktop;
pub mod device_label;
mod device_metadata;
pub mod device_names;
pub mod device_status;
pub mod file_io;
pub mod interactions;
pub mod live;
pub mod local_device;
pub mod local_scripts;
pub mod locale;
pub mod mesh_enrollment;
pub mod message_presentation;
pub mod message_time;
pub mod model_catalog;
pub mod model_connections;
pub mod model_edit;
pub mod model_editor;
pub mod new_chat;
pub mod notifications;
pub mod pages;
pub mod preferences;
pub mod relay_account;
pub mod resources;
mod services;
mod settings;
pub mod settings_actions;
pub mod state;
pub use zork_observe as observe;
pub mod store;
pub mod subscriptions;
pub mod sync;
pub mod thinking;
pub mod transcript;
pub mod transport;

include!("client.rs");

pub mod device_edit;
