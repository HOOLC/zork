pub mod channel;
pub mod distribution;
pub mod membership;
pub mod relay_account;
pub mod service;
pub mod services;
pub mod startup;
pub mod update;

/// Generate a bearer credential with 256 bits of OS randomness.
/// ULIDs are identifiers, not secrets: their timestamp bits are predictable.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS randomness unavailable");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn device_name() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| {
            s.trim()
                .trim_end_matches(".local")
                .chars()
                .take(64)
                .collect()
        })
        .filter(|s: &String| !s.is_empty())
        .unwrap_or_else(|| "Zork device".into())
}
use std::fs;
use std::path::{Path, PathBuf};

/// `sockaddr_un.sun_path` is 104 bytes on macOS (including NUL) and 108 on Linux.
const UNIX_SOCK_MAX_BYTES: usize = 103;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Station-owned files, including execution records and immutable attachments.
/// This directory belongs to Station, not to a user-published file share.
pub fn files_root(data_root: &Path) -> PathBuf {
    data_root.join("shared-files")
}

const DEFAULT_GATEWAY_BIND: &str = "127.0.0.1:18790";
const DEFAULT_RUNTIME_BIND: &str = "127.0.0.1:3000";
const DEFAULT_CONTROL_BIND: &str = "127.0.0.1:3001";
const DEFAULT_AGENT_BIND: &str = "127.0.0.1:3010";
const DEFAULT_SLACK_API: &str = "https://slack.com/api";

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileConfig {
    #[serde(default)]
    pub context: ContextConfig,
    #[serde(default)]
    pub im_connections: Vec<ImConnectionConfig>,
    #[serde(default)]
    pub bind: BindConfig,
    #[serde(default)]
    pub urls: UrlConfig,
    #[serde(default)]
    pub admin: AdminConfig,
    #[serde(default)]
    pub mesh: MeshConfig,
}

/// A started Station has Mesh on by default: it listens on its encrypted entry
/// and can mint invitations, but trusts nobody until a membership exists.
/// Whether a local node is started at all stays the client's explicit choice;
/// an operator can still turn Mesh off, and that choice is persisted.
/// Product grants remain separate from network trust.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct MeshConfig {
    pub channel: Option<channel::Channel>,
    pub name: String,
    pub group: Option<membership::MeshGroup>,
    pub enabled: bool,
    pub offline: bool,
    pub bind: Option<String>,
    pub relay_urls: Option<Vec<String>>,
    /// The UDP port relays answer QUIC address discovery on, when a
    /// deployment's relay is not on iroh's default 7842 — the case when the
    /// local network carries UDP only to particular destination ports.
    pub relay_quic_port: Option<u16>,
    pub discovery_url: Option<String>,
    /// Optional UDP-only QAD servers, separate from authenticated relay forwarding.
    pub quic_discovery_urls: Option<Vec<String>>,
    pub peers: Vec<MeshPeer>,
    /// Peers owned by account discovery, separate from installed/manual members.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub account_peers: Vec<String>,
    pub workspaces: Vec<MeshWorkspace>,
}
impl Default for MeshConfig {
    fn default() -> Self {
        Self {
            channel: None,
            name: String::new(),
            group: None,
            enabled: true,
            offline: false,
            bind: None,
            relay_urls: None,
            relay_quic_port: None,
            discovery_url: None,
            quic_discovery_urls: None,
            peers: Vec::new(),
            account_peers: Vec::new(),
            workspaces: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MeshPeer {
    pub origin: String,
    pub name: String,
    pub addr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routes: Option<membership::MeshRoutes>,
    /// Local workspace IDs this peer may ask this node to execute in.
    #[serde(default)]
    pub execute: Vec<String>,
    /// Explicit permission for this paired personal client to manage the node.
    #[serde(default)]
    pub client: bool,
    /// Full task collaboration, granted only by an explicit own-mesh invitation.
    #[serde(default)]
    pub collaborate: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MeshWorkspace {
    pub id: String,
    pub path: PathBuf,
    #[serde(default)]
    pub profile_id: String,
    pub model: String,
    #[serde(alias = "effort")]
    pub thinking: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextStrategy {
    #[default]
    Compaction,
    Handoff,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ContextConfig {
    pub strategy: ContextStrategy,
    /// Approximate token target; call/result groups are never split.
    pub keep_recent_tokens: u32,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            strategy: ContextStrategy::Compaction,
            keep_recent_tokens: 20_000,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImConnectionConfig {
    pub id: String,
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub mode: ImMode,
    #[serde(flatten)]
    pub provider: ImProviderConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum ImProviderConfig {
    Slack(SlackProviderConfig),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SlackProviderConfig {
    #[serde(default)]
    pub app_token: String,
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub api_base_url: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImMode {
    #[default]
    Normal,
    Proactive,
}

fn default_enabled() -> bool {
    true
}

impl ImConnectionConfig {
    pub fn provider_name(&self) -> &'static str {
        match &self.provider {
            ImProviderConfig::Slack(_) => "slack",
        }
    }

    pub fn slack(&self) -> Option<&SlackProviderConfig> {
        match &self.provider {
            ImProviderConfig::Slack(config) => Some(config),
        }
    }

    pub fn configured(&self) -> bool {
        match &self.provider {
            ImProviderConfig::Slack(config) => {
                !config.app_token.trim().is_empty() && !config.bot_token.trim().is_empty()
            }
        }
    }
}

impl SlackProviderConfig {
    pub fn api_base_url(&self) -> String {
        let value = self.api_base_url.trim();
        if value.is_empty() {
            DEFAULT_SLACK_API.into()
        } else {
            value.trim_end_matches('/').to_string()
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BindConfig {
    #[serde(default = "default_station_bind")]
    pub station: String,
    #[serde(default = "default_runtime_bind")]
    pub runtime: String,
    #[serde(default = "default_control_bind")]
    pub control: String,
    #[serde(default = "default_agent_bind")]
    pub agent: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UrlConfig {
    #[serde(default)]
    pub station: String,
    #[serde(default)]
    pub runtime: String,
    #[serde(default)]
    pub admin: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminConfig {
    #[serde(default)]
    pub token: String,
}

impl Default for BindConfig {
    fn default() -> Self {
        Self {
            station: default_station_bind(),
            runtime: default_runtime_bind(),
            control: default_control_bind(),
            agent: default_agent_bind(),
        }
    }
}

fn default_station_bind() -> String {
    DEFAULT_GATEWAY_BIND.into()
}
fn default_runtime_bind() -> String {
    DEFAULT_RUNTIME_BIND.into()
}
fn default_control_bind() -> String {
    DEFAULT_CONTROL_BIND.into()
}

fn default_agent_bind() -> String {
    DEFAULT_AGENT_BIND.into()
}

#[derive(Clone, Debug)]
pub struct ProcessArgs {
    pub data_root: PathBuf,
    pub listen_host: Option<String>,
    pub agent_token: Option<String>,
    pub fake_agent: bool,
    pub no_streaming: bool,
}

pub fn default_data_root() -> PathBuf {
    channel::default_root(channel::current().expect("valid installation channel"))
}

pub fn config_path(data_root: &Path) -> PathBuf {
    data_root.join("config.json")
}

pub fn parse_process_args() -> Result<ProcessArgs> {
    parse_process_args_from(std::env::args().skip(1))
}

pub fn parse_process_args_from<I, S>(args: I) -> Result<ProcessArgs>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut data_root = None;
    let mut listen_host = None;
    let mut agent_token = None;
    let mut fake_agent = false;
    let mut no_streaming = false;
    let mut args = args.into_iter().peekable();
    while let Some(raw) = args.next() {
        let arg = raw.as_ref();
        match arg {
            "--data" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing value for --data"))?;
                data_root = Some(PathBuf::from(value.as_ref()));
            }
            "--listen" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing value for --listen"))?;
                listen_host = Some(value.as_ref().to_string());
            }
            "--agent-token" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing value for --agent-token"))?;
                if value.as_ref().is_empty() {
                    anyhow::bail!("--agent-token must not be empty");
                }
                agent_token = Some(value.as_ref().to_owned());
            }
            "--fake-agent" => fake_agent = true,
            "--no-streaming" => no_streaming = true,
            "--help" | "-h" => {}
            other if other.starts_with('-') => {
                anyhow::bail!("unknown argument: {other}");
            }
            _ => {}
        }
    }
    Ok(ProcessArgs {
        data_root: data_root.unwrap_or_else(default_data_root),
        listen_host,
        agent_token,
        fake_agent,
        no_streaming,
    })
}

pub fn zork_sock_path(data_root: &Path) -> PathBuf {
    unix_socket_path(data_root, "sup")
}

pub fn zork_pid_path(data_root: &Path) -> PathBuf {
    data_root.join("zork.pid")
}

pub fn ready_pid_path(data_root: &Path, name: &str) -> PathBuf {
    // Preserve the readiness contract with already installed supervisors and
    // clients when the executable is renamed from Station to Station.
    let name = if name == "zork-station" {
        "zork-station"
    } else {
        name
    };
    data_root.join("run").join(format!("{name}.pid"))
}

pub fn write_ready_pid(data_root: &Path, name: &str) -> Result<()> {
    let dir = data_root.join("run");
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    fs::write(
        ready_pid_path(data_root, name),
        format!("{}\n", std::process::id()),
    )
    .with_context(|| format!("write {} ready pid", name))?;
    Ok(())
}

pub fn read_ready_pid(data_root: &Path, name: &str) -> Result<Option<u32>> {
    let path = ready_pid_path(data_root, name);
    match fs::read_to_string(&path) {
        Ok(body) => Ok(body.trim().parse().ok()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

pub fn clear_ready_pid(data_root: &Path, name: &str) {
    let path = ready_pid_path(data_root, name);
    let current = std::process::id();
    if fs::read_to_string(&path)
        .ok()
        .and_then(|body| body.trim().parse::<u32>().ok())
        == Some(current)
    {
        let _ = fs::remove_file(path);
    }
}

pub fn unix_socket_path(data_root: &Path, kind: &str) -> PathBuf {
    let run_dir = data_root.join("run");
    let _ = fs::create_dir_all(&run_dir);
    let preferred = run_dir.join(format!("{kind}.sock"));
    if unix_path_bytes(&preferred) <= UNIX_SOCK_MAX_BYTES {
        return preferred;
    }
    let root_token = fnv_hex(&path_bytes(data_root));
    let fallback = PathBuf::from("/tmp").join(format!("zork-{kind}-{root_token}.sock"));
    if unix_path_bytes(&fallback) <= UNIX_SOCK_MAX_BYTES {
        return fallback;
    }
    PathBuf::from("/tmp").join(format!("z{root_token}.sock"))
}

fn fnv_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn unix_path_bytes(path: &Path) -> usize {
    path_bytes(path).len()
}

fn path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        path.to_string_lossy().as_bytes().to_vec()
    }
}

pub fn ensure_layout(data_root: &Path) -> Result<FileConfig> {
    fs::create_dir_all(data_root).with_context(|| format!("create {}", data_root.display()))?;
    fs::create_dir_all(data_root.join("state"))?;
    fs::create_dir_all(files_root(data_root).join("sessions"))?;
    fs::create_dir_all(files_root(data_root).join("repos"))?;
    fs::create_dir_all(files_root(data_root).join("jobs"))?;
    fs::create_dir_all(data_root.join("logs"))?;
    fs::create_dir_all(data_root.join("bin"))?;
    fs::create_dir_all(data_root.join("run"))?;
    let path = config_path(data_root);
    if !path.exists() {
        save_config(data_root, &FileConfig::default())?;
    }
    load_config(data_root)
}

pub fn load_config(data_root: &Path) -> Result<FileConfig> {
    let path = config_path(data_root);
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let mut value: serde_json::Value =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    // The first relay prototype persisted bearer credentials in MeshConfig.
    // Discard that obsolete field; admission only reads origin-bound sessions.
    if let Some(mesh) = value
        .get_mut("mesh")
        .and_then(serde_json::Value::as_object_mut)
    {
        mesh.remove("relay_token");
    }
    let parsed: FileConfig =
        serde_json::from_value(value).with_context(|| format!("parse {}", path.display()))?;
    Ok(parsed)
}

pub fn save_config(data_root: &Path, config: &FileConfig) -> Result<()> {
    fs::create_dir_all(data_root)?;
    let path = config_path(data_root);
    let temporary_path = data_root.join(format!(".config-{}.tmp", random_token()));
    let body = serde_json::to_string_pretty(config)? + "\n";
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        use std::io::Write;
        let mut file = options.open(&temporary_path)?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary_path, &path)
            .with_context(|| format!("replace {}", path.display()))?;
        #[cfg(unix)]
        fs::File::open(data_root)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

/// Serialize read-modify-write operations across the Station, CLI and client.
pub fn update_config<T>(root: &Path, edit: impl FnOnce(&mut FileConfig) -> Result<T>) -> Result<T> {
    use fs2::FileExt;
    fs::create_dir_all(root)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("config.lock"))?;
    lock.lock_exclusive()?;
    let mut config = load_config(root)?;
    let result = edit(&mut config)?;
    save_config(root, &config)?;
    Ok(result)
}

pub fn apply_listen(config: &mut FileConfig, host: &str) {
    config.bind.station = replace_host(&config.bind.station, host);
    config.bind.runtime = replace_host(&config.bind.runtime, host);
    config.bind.control = replace_host(&config.bind.control, host);
}

pub fn has_configured_im_connection(config: &FileConfig) -> bool {
    config
        .im_connections
        .iter()
        .any(ImConnectionConfig::configured)
}

pub fn loopback_base_url(bind: &str) -> String {
    let port = bind.rsplit(':').next().unwrap_or("0");
    format!("http://127.0.0.1:{port}")
}

pub fn station_base(config: &FileConfig) -> String {
    if !config.urls.station.trim().is_empty() {
        return config.urls.station.trim_end_matches('/').to_string();
    }
    loopback_base_url(&config.bind.station)
}

pub fn parse_bind(bind: &str) -> Result<std::net::SocketAddr> {
    bind.parse()
        .with_context(|| format!("invalid bind address {bind}"))
}

pub fn missing_commands(names: &[&str]) -> Vec<String> {
    names
        .iter()
        .copied()
        .filter(|name| find_on_path(name).is_none())
        .map(ToOwned::to_owned)
        .collect()
}

pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path_value = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_value) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

pub fn find_bin(name: &str, data_root: &Path) -> Option<PathBuf> {
    let local = data_root.join("bin").join(name);
    if local.is_file() {
        return Some(local);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join(name);
            if sibling.is_file() {
                return Some(sibling);
            }
            let os = if cfg!(target_os = "macos") {
                "darwin"
            } else {
                std::env::consts::OS
            };
            let arch = match std::env::consts::ARCH {
                "aarch64" => "arm64",
                "x86_64" => "x64",
                other => other,
            };
            let packaged = dir.join(format!("{name}-{os}-{arch}"));
            if packaged.is_file() {
                return Some(packaged);
            }
        }
    }
    find_on_path(name)
}

fn replace_host(bind: &str, host: &str) -> String {
    match bind.rsplit_once(':') {
        Some((_, port)) => format!("{host}:{port}"),
        None => format!("{host}:{bind}"),
    }
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("USERPROFILE")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_config() {
        let dir = tempfile::tempdir().unwrap();
        let config = FileConfig {
            im_connections: vec![
                ImConnectionConfig {
                    id: "01J00000000000000000000001".into(),
                    name: "work".into(),
                    enabled: true,
                    mode: ImMode::Normal,
                    provider: ImProviderConfig::Slack(SlackProviderConfig {
                        app_token: "xapp-1".into(),
                        bot_token: "xoxb-1".into(),
                        api_base_url: String::new(),
                    }),
                },
                ImConnectionConfig {
                    id: "01J00000000000000000000002".into(),
                    name: "community".into(),
                    enabled: true,
                    mode: ImMode::Proactive,
                    provider: ImProviderConfig::Slack(SlackProviderConfig {
                        app_token: "xapp-2".into(),
                        bot_token: "xoxb-2".into(),
                        api_base_url: "https://slack.example/api/".into(),
                    }),
                },
            ],
            ..FileConfig::default()
        };
        save_config(dir.path(), &config).unwrap();
        let loaded = load_config(dir.path()).unwrap();
        assert_eq!(loaded.im_connections, config.im_connections);
        assert_eq!(loaded.im_connections[0].provider_name(), "slack");
        assert_eq!(
            loaded.im_connections[1].slack().unwrap().api_base_url(),
            "https://slack.example/api"
        );
        assert!(has_configured_im_connection(&loaded));
    }

    #[test]
    fn fresh_station_config_enables_mesh_but_keeps_an_explicit_off() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = ensure_layout(dir.path()).unwrap();
        assert!(
            fresh.mesh.enabled,
            "a started Station has Mesh on by default"
        );
        assert!(fresh.mesh.group.is_none() && fresh.mesh.peers.is_empty());
        // Hand-written configuration without the field follows the same default.
        fs::write(config_path(dir.path()), r#"{"mesh":{"name":"n"}}"#).unwrap();
        assert!(load_config(dir.path()).unwrap().mesh.enabled);
        // An operator's explicit choice survives a reload.
        update_config(dir.path(), |config| {
            config.mesh.enabled = false;
            Ok(())
        })
        .unwrap();
        assert!(!ensure_layout(dir.path()).unwrap().mesh.enabled);
    }

    #[test]
    fn listen_rewrites_hosts() {
        let mut config = FileConfig::default();
        apply_listen(&mut config, "0.0.0.0");
        assert_eq!(config.bind.station, "0.0.0.0:18790");
        assert_eq!(
            loopback_base_url(&config.bind.control),
            "http://127.0.0.1:3001"
        );
    }

    #[test]
    fn unix_sockets_fit_under_sun_len() {
        let dir = tempfile::tempdir().unwrap();
        let sock = zork_sock_path(dir.path());
        assert!(sock.starts_with(dir.path().join("run")));
        assert!(unix_path_bytes(&sock) <= UNIX_SOCK_MAX_BYTES);
    }

    #[test]
    fn long_data_root_falls_back_to_tmp() {
        let long = PathBuf::from(format!("/{}", "x".repeat(180)));
        let sock = unix_socket_path(&long, "sup");
        assert!(sock.starts_with("/tmp/"));
        assert!(unix_path_bytes(&sock) <= UNIX_SOCK_MAX_BYTES);
    }

    #[test]
    fn ready_pid_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        write_ready_pid(dir.path(), "zork-agent").unwrap();
        assert_eq!(
            read_ready_pid(dir.path(), "zork-agent").unwrap(),
            Some(std::process::id())
        );
        clear_ready_pid(dir.path(), "zork-agent");
        assert_eq!(read_ready_pid(dir.path(), "zork-agent").unwrap(), None);
    }

    #[test]
    fn station_readiness_remains_visible_to_existing_clients() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("run")).unwrap();
        let legacy = dir.path().join("run/zork-station.pid");
        fs::write(&legacy, std::process::id().to_string()).unwrap();
        assert_eq!(
            read_ready_pid(dir.path(), "zork-station").unwrap(),
            Some(std::process::id())
        );
        clear_ready_pid(dir.path(), "zork-station");
        assert!(!legacy.exists());
        write_ready_pid(dir.path(), "zork-station").unwrap();
        assert_eq!(
            read_ready_pid(dir.path(), "zork-station").unwrap(),
            Some(std::process::id())
        );
    }

    #[test]
    fn agent_token_is_an_optional_startup_argument() {
        let absent = parse_process_args_from(Vec::<String>::new()).unwrap();
        assert_eq!(absent.agent_token, None);

        let supplied = parse_process_args_from(["--agent-token", "exact secret"]).unwrap();
        assert_eq!(supplied.agent_token.as_deref(), Some("exact secret"));

        assert!(parse_process_args_from(["--agent-token"]).is_err());
        assert!(parse_process_args_from(["--agent-token", ""]).is_err());
    }

    #[test]
    fn no_streaming_is_an_optional_process_wide_flag() {
        let absent = parse_process_args_from(Vec::<String>::new()).unwrap();
        assert!(!absent.no_streaming);

        let supplied = parse_process_args_from(["--no-streaming"]).unwrap();
        assert!(supplied.no_streaming);
    }
}

#[cfg(test)]
mod atomic_save_tests {
    #[test]
    fn concurrent_saves_never_publish_partial_json() {
        let root = tempfile::tempdir().unwrap();
        super::save_config(root.path(), &super::FileConfig::default()).unwrap();
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let root = root.path();
                scope.spawn(move || {
                    for _ in 0..20 {
                        super::save_config(root, &super::FileConfig::default()).unwrap();
                        super::load_config(root).unwrap();
                    }
                });
            }
        });
        assert!(!std::fs::read_dir(root.path()).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
    }
}
