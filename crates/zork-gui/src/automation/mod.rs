//! Loopback-only development automation for the native GPUI window.
//!
//! The API observes rendered geometry and can only mutate the app by dispatching
//! mouse and keyboard input. It intentionally has no access to `RootView` or to
//! station/business methods.

mod driver;
mod element;
pub mod protocol;
mod server;

use std::net::SocketAddr;
use std::thread::JoinHandle;

use gpui::{AnyWindowHandle, App};
use tokio::sync::mpsc;

use driver::{attach_driver, DriverEnvelope};
use element::{AutomationRegistry, AutomationRegistryGlobal};

pub use element::{AutomationElementExt, AutomationRoot};
pub use protocol::AutomationRole;

pub const DEFAULT_DEV_PORT: u16 = 8765;

/// In-process access to the same observed geometry and physical input driver.
/// Wrap the tested view in `AutomationRoot`; no HTTP listener or server thread
/// is created, and no application-state mutation commands are exposed.
#[cfg(feature = "headless-bench")]
#[derive(Clone)]
pub struct HeadlessAutomation {
    registry: AutomationRegistry,
}

#[cfg(feature = "headless-bench")]
impl HeadlessAutomation {
    pub fn install(cx: &mut App) -> Self {
        let registry = AutomationRegistry::new();
        cx.set_global(AutomationRegistryGlobal(registry.clone()));
        Self { registry }
    }

    pub fn snapshot(&self, include_hidden: bool) -> protocol::UiSnapshot {
        self.registry.snapshot(include_hidden)
    }

    pub fn dispatch(
        &self,
        action: protocol::UserAction,
        window: &mut gpui::Window,
        cx: &mut App,
    ) -> anyhow::Result<protocol::ActionResult> {
        driver::headless_action(action, window, cx, &self.registry)
            .map_err(|error| anyhow::anyhow!("{}: {}", error.code, error.message))
    }
}

pub struct DevAutomation {
    address: SocketAddr,
    token: String,
    registry: AutomationRegistry,
    receiver: mpsc::UnboundedReceiver<DriverEnvelope>,
    server_thread: JoinHandle<()>,
}

impl DevAutomation {
    #[cfg(feature = "headless-bench")]
    pub fn in_process_driver(&self) -> HeadlessAutomation {
        HeadlessAutomation {
            registry: self.registry.clone(),
        }
    }
    pub fn bind(port: u16, token: Option<String>) -> std::io::Result<Self> {
        let token = match token {
            Some(token) if !token.is_empty() => token,
            Some(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "dev automation token must not be empty",
                ));
            }
            None => zork_client_core::desktop::automation_token(),
        };
        let binding = server::bind(port, token.clone())?;
        Ok(Self {
            address: binding.address,
            token,
            registry: binding.registry,
            receiver: binding.receiver,
            server_thread: binding.thread,
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn install(&self, cx: &mut App) {
        cx.set_global(AutomationRegistryGlobal(self.registry.clone()));
    }

    pub fn attach(self, window: AnyWindowHandle, cx: &App) {
        let Self {
            registry,
            receiver,
            server_thread,
            ..
        } = self;
        attach_driver(window, registry, receiver, cx);
        // Dropping a JoinHandle detaches the loopback server. The process owns
        // its lifetime, so no business view needs to retain an automation task.
        drop(server_thread);
    }
}
