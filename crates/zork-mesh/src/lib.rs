//! Zork's bounded interface to the pinned, embedded Synchronicity runtime.
//! Product state and command idempotency belong to the Station.
pub mod bridge;
mod clean_start;
pub mod enrollment;
pub mod feed;
mod local_discovery;
pub mod managed;
pub mod node;
mod relay_access;
pub mod route;

pub const SYNCH_VERSION: &str = "0.1.8";
pub const MAX_FRAME: usize = 256 * 1024;
pub const MAX_ARTIFACT: usize = 300 * 1024 * 1024;

pub fn content_root(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

pub mod services;
