//! Zork's owned iroh transport and immutable attachment transfer.
//! Product state and command idempotency belong to the Station.
pub mod control;
pub mod enrollment;
pub mod feed;
mod identity;
mod lan_discovery;
mod local_discovery;
pub mod managed;
mod network;
pub mod node;
mod punch;
pub mod retry;
pub mod route;

pub const MAX_FRAME: usize = 256 * 1024;
pub const MAX_ARTIFACT: usize = 300 * 1024 * 1024;

pub fn content_root(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

pub mod services;
