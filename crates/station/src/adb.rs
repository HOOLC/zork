//! Android-initiated Mesh leases expose ADB only on this host's loopback.
use crate::state::AppState;
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{mpsc, watch, OwnedSemaphorePermit, Semaphore},
    time::Instant,
};
use zork_client_types::adb::{Advertisement, HostState, LocalState};
use zork_mesh::bridge::{Peer, Reply};

#[derive(Default)]
pub struct Registry {
    entries: Mutex<HashMap<String, Entry>>,
    stopped: std::sync::atomic::AtomicBool,
}
struct Entry {
    advertisement: Advertisement,
    serial: Option<String>,
    state: HostState,
    cancel: watch::Sender<bool>,
    pending: HashMap<String, Pending>,
}
struct Pending {
    socket: TcpStream,
    permit: OwnedSemaphorePermit,
    deadline: Instant,
}
impl Registry {
    pub fn shutdown(&self) {
        self.stopped
            .store(true, std::sync::atomic::Ordering::SeqCst);
        for (_, entry) in self.entries.lock().expect("ADB registry").drain() {
            entry.cancel.send_replace(true);
        }
    }
    fn remove(&self, origin: &str, generation: &str) {
        let mut entries = self.entries.lock().expect("ADB registry");
        if entries
            .get(origin)
            .is_some_and(|e| e.advertisement.generation == generation)
        {
            if let Some(entry) = entries.remove(origin) {
                entry.cancel.send_replace(true);
            }
        }
    }
    fn take(&self, origin: &str, generation: &str, id: &str) -> Result<Reply> {
        let mut entries = self.entries.lock().expect("ADB registry");
        let entry = entries.get_mut(origin).context("adb_lease_closed")?;
        ensure!(
            entry.advertisement.generation == generation && !*entry.cancel.borrow(),
            "adb_lease_replaced"
        );
        let pending = entry.pending.remove(id).context("adb_stream_unknown")?;
        ensure!(pending.deadline > Instant::now(), "adb_stream_expired");
        Ok(Reply::Tunnel {
            upstream: pending.socket,
            cancelled: entry.cancel.subscribe(),
            guard: Some(Box::new(pending.permit)),
        })
    }
}

fn allowed(state: &AppState, origin: &str) -> bool {
    zork_config::load_config(&state.config.data_root)
        .ok()
        .is_some_and(|c| c.mesh.peers.iter().any(|p| p.origin == origin))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    v: u8,
    request: Request,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    AdbRegister { body: Advertisement },
    AdbStream { generation: String, id: String },
}

pub async fn handle(state: AppState, peer: Peer, value: Value) -> Result<Reply> {
    let request: Envelope = serde_json::from_value(value)?;
    ensure!(request.v == 1, "mesh_protocol_version");
    ensure!(allowed(&state, &peer.origin), "mesh_client_not_granted");
    let registry = state.mesh.get().context("mesh_not_ready")?.adb.clone();
    match request.request {
        Request::AdbStream { generation, id } => registry.take(&peer.origin, &generation, &id),
        Request::AdbRegister { body } => {
            ensure!(
                body.generation.parse::<ulid::Ulid>().is_ok(),
                "invalid_adb_generation"
            );
            ensure!(body.activation_port >= 1024, "invalid_adb_port");
            ensure!(
                body.bridge_application_id.len() <= 255
                    && body
                        .bridge_application_id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_'),
                "invalid_adb_application"
            );
            ensure!(
                body.name.len() <= 128 && !body.name.chars().any(char::is_control),
                "invalid_adb_name"
            );
            let listener = if body.local == LocalState::Ready {
                Some(TcpListener::bind("127.0.0.1:0").await?)
            } else {
                None
            };
            let serial = listener
                .as_ref()
                .map(|l| l.local_addr().map(|a| a.to_string()))
                .transpose()?;
            let (cancel, mut cancelled) = watch::channel(false);
            let origin = peer.origin;
            let generation = body.generation.clone();
            {
                let mut entries = registry.entries.lock().expect("ADB registry");
                ensure!(
                    !registry.stopped.load(std::sync::atomic::Ordering::SeqCst),
                    "adb_stopping"
                );
                ensure!(
                    entries.len() < 16 || entries.contains_key(&origin),
                    "adb_device_limit"
                );
                if let Some(old) = entries.insert(
                    origin.clone(),
                    Entry {
                        advertisement: body,
                        serial: serial.clone(),
                        state: HostState::Connecting,
                        cancel,
                        pending: HashMap::new(),
                    },
                ) {
                    old.cancel.send_replace(true);
                }
            }
            let (tx, rx) = mpsc::channel(16);
            let mut policy = state.db.realtime.listen(crate::realtime::MESH);
            tokio::spawn(async move {
                let (host, host_rx) = watch::channel(HostState::Connecting);
                tokio::select! {
                    _ = tx.closed() => {},
                    _ = cancelled.changed() => {},
                    _ = policy.until(|| (!allowed(&state, &origin)).then_some(())) => {},
                    _ = monitor_host(serial.as_deref(), host) => {},
                    _ = serve(&registry, &origin, &generation, listener.as_ref(), &tx, host_rx) => {},
                }
                // Close accepts before publishing removal or waiting for the
                // SDK server to disconnect this lease's transport.
                drop(listener);
                registry.remove(&origin, &generation);
                // Disconnect only this lease's serial, never the global ADB server.
                if let Some(serial) = serial {
                    let _ = adb(&["disconnect", &serial]).await;
                }
            });
            Ok(Reply::Subscription(rx))
        }
    }
}

async fn serve(
    registry: &Registry,
    origin: &str,
    generation: &str,
    listener: Option<&TcpListener>,
    tx: &mpsc::Sender<Value>,
    mut host: watch::Receiver<HostState>,
) -> Result<()> {
    let slots = Arc::new(Semaphore::new(16));
    let mut latest = Value::Null;
    let mut heartbeat = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_secs(3));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            accepted = async {
                match &listener {
                    Some(listener) => listener.accept().await,
                    None => std::future::pending().await,
                }
            } => {
                let (socket, _) = accepted?;
                let Ok(permit) = slots.clone().try_acquire_owned() else { continue };
                let id = ulid::Ulid::new().to_string();
                {
                    let mut entries = registry.entries.lock().expect("ADB registry");
                    let entry = entries.get_mut(origin).context("adb_lease_closed")?;
                    ensure!(entry.advertisement.generation == generation, "adb_lease_replaced");
                    entry.pending.insert(id.clone(), Pending { socket, permit, deadline: Instant::now() + Duration::from_secs(15) });
                }
                tokio::time::timeout(Duration::from_secs(5), tx.send(json!({"event":"open","generation":generation,"id":id}))).await??;
            },
            result = host.changed() => { result?; },
            _ = tick.tick() => {},
        }
        let value = {
            let mut entries = registry.entries.lock().expect("ADB registry");
            let entry = entries.get_mut(origin).context("adb_lease_closed")?;
            ensure!(
                entry.advertisement.generation == generation,
                "adb_lease_replaced"
            );
            entry
                .pending
                .retain(|_, pending| pending.deadline > Instant::now());
            entry.state = *host.borrow_and_update();
            json!({"event":"state","generation":generation,"state":entry.state,"serial":entry.serial})
        };
        if value != latest || heartbeat.elapsed() >= Duration::from_secs(15) {
            tokio::time::timeout(Duration::from_secs(5), tx.send(value.clone())).await??;
            latest = value;
            heartbeat = Instant::now();
        }
    }
}

// ADB command waits run independently of accepts: adb connect itself opens
// the listener and waits for the phone's reverse stream.
async fn monitor_host(serial: Option<&str>, host: watch::Sender<HostState>) {
    let Some(serial) = serial else {
        return std::future::pending().await;
    };
    let mut state = HostState::Connecting;
    let mut next_connect = Instant::now();
    loop {
        let connect = !matches!(state, HostState::Ready | HostState::Unauthorized)
            && Instant::now() >= next_connect;
        if connect {
            next_connect = Instant::now() + Duration::from_secs(15);
        }
        state = host_state(serial, connect).await;
        host.send_if_modified(|old| {
            if *old == state {
                false
            } else {
                *old = state;
                true
            }
        });
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

fn adb_path() -> PathBuf {
    for variable in ["ANDROID_SDK_ROOT", "ANDROID_HOME"] {
        if let Some(root) = std::env::var_os(variable) {
            let path = PathBuf::from(root)
                .join("platform-tools")
                .join(if cfg!(windows) { "adb.exe" } else { "adb" });
            if path.is_file() {
                return path;
            }
        }
    }
    if let Some(root) = std::env::var_os("LOCALAPPDATA") {
        let path = PathBuf::from(root).join("Android/Sdk/platform-tools/adb.exe");
        if path.is_file() {
            return path;
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        for relative in [
            "Library/Android/sdk/platform-tools/adb",
            "Android/Sdk/platform-tools/adb",
        ] {
            let path = PathBuf::from(&home).join(relative);
            if path.is_file() {
                return path;
            }
        }
    }
    "adb".into()
}
async fn adb(args: &[&str]) -> Result<std::process::Output> {
    Ok(tokio::time::timeout(
        Duration::from_secs(8),
        tokio::process::Command::new(adb_path())
            .args(args)
            .kill_on_drop(true)
            .output(),
    )
    .await??)
}
async fn host_state(serial: &str, connect: bool) -> HostState {
    if connect {
        if let Err(error) = adb(&["connect", serial]).await {
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            {
                return HostState::AdbMissing;
            }
        }
    }
    match adb(&["-s", serial, "get-state"]).await {
        Ok(output) => parse_state(&output.stdout, &output.stderr),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            HostState::AdbMissing
        }
        Err(_) => HostState::Offline,
    }
}
fn parse_state(stdout: &[u8], stderr: &[u8]) -> HostState {
    if String::from_utf8_lossy(stdout).trim() == "device" {
        HostState::Ready
    } else if String::from_utf8_lossy(stderr).contains("unauthorized") {
        HostState::Unauthorized
    } else {
        HostState::Offline
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn leases_reject_cross_phone_stale_and_replayed_streams() -> Result<()> {
        let registry = Registry::default();
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let socket = TcpStream::connect(listener.local_addr()?).await?;
        let (_client, _) = listener.accept().await?;
        let (cancel, _) = watch::channel(false);
        let slots = Arc::new(Semaphore::new(1));
        registry.entries.lock().unwrap().insert(
            "phone".into(),
            Entry {
                advertisement: Advertisement {
                    generation: "new".into(),
                    name: "phone".into(),
                    activation_port: 5555,
                    bridge_application_id: "fixture.phone".into(),
                    local: LocalState::Ready,
                },
                serial: None,
                state: HostState::Connecting,
                cancel,
                pending: HashMap::from([(
                    "stream".into(),
                    Pending {
                        socket,
                        permit: slots.clone().acquire_owned().await?,
                        deadline: Instant::now() + Duration::from_secs(3),
                    },
                )]),
            },
        );
        assert!(registry.take("other-phone", "new", "stream").is_err());
        assert!(registry.take("phone", "old", "stream").is_err());
        registry.remove("phone", "old");
        let reply = registry.take("phone", "new", "stream")?;
        assert!(registry.take("phone", "new", "stream").is_err());
        assert!(slots.clone().try_acquire_owned().is_err());
        let Reply::Tunnel { cancelled, .. } = &reply else {
            panic!()
        };
        registry.shutdown();
        assert!(*cancelled.borrow());
        drop(reply);
        assert!(slots.try_acquire_owned().is_ok());
        Ok(())
    }
    #[test]
    fn distinguishes_authorization_and_transport_state() {
        assert_eq!(parse_state(b"device\n", b""), HostState::Ready);
        assert_eq!(
            parse_state(b"", b"error: device unauthorized."),
            HostState::Unauthorized
        );
        assert_eq!(parse_state(b"offline", b""), HostState::Offline);
    }
}
