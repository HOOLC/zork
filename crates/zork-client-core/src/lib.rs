//! Client-owned identity, offline state and bounded Station operations.
//! The platform reports host visibility and service lifetime; core decides when
//! to retain or release connections according to the user's background preference.
#[cfg(not(target_family = "wasm"))]
pub mod activity;
#[cfg(not(target_family = "wasm"))]
pub mod adb;
#[cfg(not(target_family = "wasm"))]
pub mod api;
#[cfg(not(target_family = "wasm"))]
pub mod chat_files;
pub mod composer;
#[cfg(not(target_family = "wasm"))]
pub mod conversation;
#[cfg(not(target_family = "wasm"))]
pub mod data_reset;
pub use zork_client_types::{comments, files};
#[cfg(not(target_family = "wasm"))]
pub use zork_config::channel;
pub mod agent_edit;
#[cfg(not(target_family = "wasm"))]
mod catalog;
#[cfg(not(target_family = "wasm"))]
mod client_directory;
#[cfg(not(target_family = "wasm"))]
pub mod delivery;
#[cfg(feature = "desktop")]
#[cfg(not(target_family = "wasm"))]
pub mod desktop;
#[cfg(not(target_family = "wasm"))]
mod device_metadata;
#[cfg(not(target_family = "wasm"))]
mod enrollment;
#[cfg(not(target_family = "wasm"))]
pub mod file_io;
pub mod interactions;
#[cfg(not(target_family = "wasm"))]
pub mod live;
#[cfg(not(target_family = "wasm"))]
pub mod local_scripts;
pub mod locale;
#[cfg(not(target_family = "wasm"))]
pub mod mesh_enrollment;
pub mod model_edit;
pub mod new_chat;
#[cfg(not(target_family = "wasm"))]
pub mod notifications;
#[cfg(not(target_family = "wasm"))]
pub mod pages;
#[cfg(not(target_family = "wasm"))]
pub mod preferences;
#[cfg(not(target_family = "wasm"))]
pub mod relay_account;
#[cfg(not(target_family = "wasm"))]
pub mod resources;
#[cfg(not(target_family = "wasm"))]
mod services;
#[cfg(not(target_family = "wasm"))]
mod settings;
#[cfg(not(target_family = "wasm"))]
pub mod settings_actions;
#[cfg(not(target_family = "wasm"))]
pub mod shared_files;
pub mod state;
pub use zork_observe as observe;
#[cfg(not(target_family = "wasm"))]
pub mod store;
#[cfg(not(target_family = "wasm"))]
pub mod subscriptions;
#[cfg(not(target_family = "wasm"))]
pub mod sync;
#[cfg(not(target_family = "wasm"))]
pub mod transcript;
#[cfg(not(target_family = "wasm"))]
pub mod transport;

#[cfg(target_family = "wasm")]
#[path = "api/portable.rs"]
pub mod api;

#[cfg(not(target_family = "wasm"))]
include!("client.rs");

pub mod device_edit;
