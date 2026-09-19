//! User service registration and the local supervisor control protocol.
//! The GUI and the installer share this code; neither owns an independent service.
#[cfg(unix)]
mod events;
#[cfg(target_os = "macos")]
mod identity;
use anyhow::{ensure, Context, Result};
#[cfg(unix)]
pub use events::Events;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

/// Dedicated runtime workers must not inherit GPUI/libdispatch signal masks:
/// Tokio uses SIGCHLD and Synch's eBPF workers use SIGUSR1.
pub fn clear_signal_mask() -> std::io::Result<()> {
    #[cfg(unix)]
    unsafe {
        let mut mask: libc::sigset_t = std::mem::zeroed();
        if libc::sigemptyset(&mut mask) != 0
            || libc::sigprocmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut()) != 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Clear inherited worker signal masks before executing a child server.
pub fn prepare_child(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(clear_signal_mask);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = command;
    }
}

/// Enter packaged helpers through their bundle path so macOS identifies the
/// intended process immediately, without a compatibility re-exec.
pub fn launch_path(path: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    if let Ok(resolved) = path.canonicalize() {
        if resolved
            .file_name()
            .is_some_and(|name| name == "ZorkHelperLauncher")
            || identity::native_helper(&resolved)
        {
            return resolved;
        }
    }
    path.to_owned()
}

/// Canonical bundle entry has completed, so background storage preparation can
/// begin without a later compatibility re-exec interrupting it.
pub struct ProcessIdentity {
    #[cfg(target_os = "macos")]
    native: bool,
}

impl ProcessIdentity {
    /// Call on the main thread before publishing runtime readiness.
    pub fn register(self) -> Result<()> {
        #[cfg(target_os = "macos")]
        identity::register(self.native)?;
        Ok(())
    }
}

pub fn prepare_process_identity() -> Result<ProcessIdentity> {
    Ok(ProcessIdentity {
        #[cfg(target_os = "macos")]
        native: identity::enter_bundle()?,
    })
}

/// Call on the main thread before starting a packaged background runtime.
pub fn register_process_identity() -> Result<()> {
    prepare_process_identity()?.register()
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServiceSettings {
    pub enabled: bool,
    pub start_at_login: bool,
}

pub fn settings(root: &Path) -> Result<ServiceSettings> {
    match fs::read(root.join("service.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ServiceSettings::default()),
        Err(e) => Err(e.into()),
    }
}

pub fn save_settings(root: &Path, settings: &ServiceSettings) -> Result<()> {
    crate::ensure_layout(root)?;
    let temp = root.join("service.json.tmp");
    fs::write(&temp, serde_json::to_vec(settings)?)?;
    fs::rename(temp, root.join("service.json"))?;
    Ok(())
}

pub fn label(root: &Path) -> Result<String> {
    Ok(format!(
        "com.zork.node.{}",
        crate::fnv_hex(&crate::path_bytes(&root.canonicalize()?))
    ))
}

#[cfg(unix)]
pub fn connect(root: &Path, command: &str) -> Result<(std::os::unix::net::UnixStream, String)> {
    connect_with_timeout(root, command, Duration::from_secs(3))
}

#[cfg(unix)]
pub fn connect_with_timeout(
    root: &Path,
    command: &str,
    timeout: Duration,
) -> Result<(std::os::unix::net::UnixStream, String)> {
    let mut stream = std::os::unix::net::UnixStream::connect(crate::zork_sock_path(root))
        .context("Station supervisor is not running")?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    writeln!(stream, "{command}")?;
    // Read one byte at a time through BufReader to retain the connection as a lease.
    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply)?;
    ensure!(
        !reply.is_empty() && !reply.starts_with("error:"),
        "{}",
        reply.trim()
    );
    Ok((stream, reply.trim().to_owned()))
}

pub fn running(root: &Path) -> bool {
    #[cfg(unix)]
    {
        connect(root, "status").is_ok_and(|(_, reply)| {
            serde_json::from_str::<serde_json::Value>(&reply).is_ok_and(|v| v["protocol"] == 1)
        })
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        false
    }
}

fn checked(command: &mut Command) -> Result<()> {
    let output = command.output()?;
    ensure!(
        output.status.success(),
        "service manager failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn install(root: &Path, binary: &Path, start_at_login: bool) -> Result<()> {
    let root = root.canonicalize()?;
    let binary = binary.canonicalize()?;
    let prior = settings(&root)?;
    save_settings(
        &root,
        &ServiceSettings {
            enabled: true,
            start_at_login,
        },
    )?;
    let result = install_platform(&root, &binary, start_at_login);
    if result.is_err() {
        let _ = save_settings(&root, &prior);
    }
    result
}

#[cfg(target_os = "macos")]
fn install_platform(root: &Path, binary: &Path, start_at_login: bool) -> Result<()> {
    let label = label(root)?;
    let uid = String::from_utf8(Command::new("id").arg("-u").output()?.stdout)?
        .trim()
        .to_owned();
    let domain = format!("gui/{uid}");
    let target = format!("{domain}/{label}");
    let agents = crate::home_dir().join("Library/LaunchAgents");
    fs::create_dir_all(&agents)?;
    let installed = agents.join(format!("{label}.plist"));
    let transient = root.join("run/service.plist");
    let path = if start_at_login {
        &installed
    } else {
        &transient
    };
    let log = root.join("logs/service.log");
    let mut arguments = vec![
        binary.display().to_string(),
        "service-run".into(),
        "--data".into(),
        root.display().to_string(),
    ];
    if std::env::var_os("ZORK_DESKTOP_FAKE_AGENT").is_some() {
        arguments.push("--fake-agent".into());
    }
    let extra_env = std::env::var("ZORK_REGISTRY_DIR")
        .ok()
        .map(|v| format!("<key>ZORK_REGISTRY_DIR</key><string>{}</string>", xml(&v)))
        .unwrap_or_default();
    let args = arguments
        .iter()
        .map(|a| format!("<string>{}</string>", xml(a)))
        .collect::<String>();
    let content=format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>{label}</string><key>ProgramArguments</key><array>{args}</array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>3</integer><key>EnvironmentVariables</key><dict><key>PATH</key><string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>{extra_env}</dict><key>AbandonProcessGroup</key><true/><key>StandardOutPath</key><string>{log}</string><key>StandardErrorPath</key><string>{log}</string></dict></plist>",log=xml(&log.to_string_lossy()));
    fs::write(path, content)?;
    if !start_at_login {
        match fs::remove_file(&installed) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    if Command::new("launchctl")
        .args(["print", &target])
        .output()?
        .status
        .success()
    {
        // Re-register the watcher only; it never kills an adopted supervisor.
        checked(Command::new("launchctl").args(["bootout", &target]))?;
    }
    checked(
        Command::new("launchctl")
            .arg("bootstrap")
            .arg(domain)
            .arg(path),
    )
}

#[cfg(target_os = "linux")]
fn install_platform(root: &Path, binary: &Path, start_at_login: bool) -> Result<()> {
    let name = format!("{}.service", label(root)?);
    let dir = crate::home_dir().join(".config/systemd/user");
    fs::create_dir_all(&dir)?;
    let quote = |p: &Path| {
        format!(
            "\"{}\"",
            p.to_string_lossy()
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('%', "%%")
        )
    };
    fs::write(dir.join(&name),format!("[Unit]\nDescription=Zork Station\n[Service]\nExecStart={} service-run --data {}\nRestart=always\nRestartSec=3\nKillMode=process\nEnvironment=PATH=/usr/local/bin:/usr/bin:/bin\n[Install]\nWantedBy=default.target\n",quote(binary),quote(root)))?;
    checked(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
    checked(Command::new("systemctl").args([
        "--user",
        if start_at_login { "enable" } else { "disable" },
        &name,
    ]))?;
    checked(Command::new("systemctl").args(["--user", "restart", &name]))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn install_platform(_: &Path, _: &Path, _: bool) -> Result<()> {
    anyhow::bail!("Background Station requires macOS launchd or Linux user systemd")
}

/// Unregister only the watcher. The caller explicitly stops or leases the Station.
pub fn uninstall(root: &Path) -> Result<()> {
    let previous = settings(root)?;
    save_settings(
        root,
        &ServiceSettings {
            enabled: false,
            start_at_login: false,
        },
    )?;
    let result = uninstall_platform(root);
    if result.is_err() {
        let _ = save_settings(root, &previous);
    }
    result
}

#[cfg(target_os = "macos")]
fn uninstall_platform(root: &Path) -> Result<()> {
    let label = label(root)?;
    let uid = String::from_utf8(Command::new("id").arg("-u").output()?.stdout)?
        .trim()
        .to_owned();
    let target = format!("gui/{uid}/{label}");
    if Command::new("launchctl")
        .args(["print", &target])
        .output()?
        .status
        .success()
    {
        checked(Command::new("launchctl").args(["bootout", &target]))?;
    }
    let path = crate::home_dir()
        .join("Library/LaunchAgents")
        .join(format!("{label}.plist"));
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
#[cfg(target_os = "linux")]
fn uninstall_platform(root: &Path) -> Result<()> {
    let name = format!("{}.service", label(root)?);
    let path = crate::home_dir().join(".config/systemd/user").join(&name);
    if path.exists() {
        checked(Command::new("systemctl").args(["--user", "disable", "--now", &name]))?;
        fs::remove_file(path)?;
        checked(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
    }
    Ok(())
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn uninstall_platform(_: &Path) -> Result<()> {
    Ok(())
}

/// These are the supported data roots the one-command installer can discover.
pub fn known_roots() -> Vec<PathBuf> {
    let mut roots = vec![crate::default_data_root()];
    let client = std::env::var_os("ZORK_CLIENT_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::home_dir().join("Library/Application Support/Zork/client"));
    roots.push(client.join("node"));
    if let Ok(entries) = fs::read_dir(registry_dir()) {
        for entry in entries.flatten() {
            if let Ok(bytes) = fs::read(entry.path()) {
                if let Ok(root) = serde_json::from_slice::<PathBuf>(&bytes) {
                    if crate::config_path(&root).is_file() {
                        roots.push(root);
                    }
                }
            }
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

fn registry_dir() -> PathBuf {
    std::env::var_os("ZORK_REGISTRY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::home_dir().join(".local/state/zork/nodes"))
}

pub fn register(root: &Path) -> Result<()> {
    let root = root.canonicalize()?;
    let dir = registry_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", label(&root)?));
    fs::write(path, serde_json::to_vec(&root)?)?;
    Ok(())
}

pub fn supervisor_lock(root: &Path) -> Result<fs::File> {
    exclusive_lock(&root.join("run/supervisor.lock"))
}

pub fn exclusive_lock(path: &Path) -> Result<fs::File> {
    use fs2::FileExt;
    fs::create_dir_all(path.parent().context("lock directory")?)?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.try_lock_exclusive()
        .context("A Station supervisor already manages this data directory")?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    #[test]
    fn packaged_launcher_is_canonical_but_ordinary_aliases_are_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let launcher = temp.path().join("ZorkHelperLauncher");
        fs::write(&launcher, "fixture").unwrap();
        let alias = temp.path().join("zork");
        std::os::unix::fs::symlink(&launcher, &alias).unwrap();
        assert_eq!(launch_path(&alias), launcher.canonicalize().unwrap());
        let native = temp
            .path()
            .join("ZorkStation.app/Contents/MacOS/zork-station");
        fs::create_dir_all(native.parent().unwrap()).unwrap();
        fs::write(&native, "native fixture").unwrap();
        let native_alias = temp.path().join("zork-station");
        std::os::unix::fs::symlink(&native, &native_alias).unwrap();
        assert_eq!(launch_path(&native_alias), native.canonicalize().unwrap());
        let ordinary = temp.path().join("ordinary");
        std::os::unix::fs::symlink("/bin/sh", &ordinary).unwrap();
        assert_eq!(launch_path(&ordinary), ordinary);
    }
    #[test]
    fn service_identity_uses_canonical_data_root() {
        let temp = tempfile::tempdir().unwrap();
        crate::ensure_layout(temp.path()).unwrap();
        assert_eq!(
            label(temp.path()).unwrap(),
            label(&temp.path().join(".")).unwrap()
        );
        assert!(!settings(temp.path()).unwrap().enabled);
        save_settings(
            temp.path(),
            &ServiceSettings {
                enabled: true,
                start_at_login: false,
            },
        )
        .unwrap();
        assert!(settings(temp.path()).unwrap().enabled);
    }
    #[cfg(unix)]
    #[test]
    fn station_children_do_not_inherit_a_gui_worker_signal_mask() {
        use std::os::unix::process::ExitStatusExt;
        std::thread::spawn(|| {
            unsafe {
                let mut mask: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut mask);
                libc::sigaddset(&mut mask, libc::SIGUSR1);
                libc::sigaddset(&mut mask, libc::SIGCHLD);
                assert_eq!(
                    libc::pthread_sigmask(libc::SIG_BLOCK, &mask, std::ptr::null_mut()),
                    0
                );
            }
            let mut command = Command::new("/bin/sh");
            command.args(["-c", "kill -USR1 $$"]);
            prepare_child(&mut command);
            assert_eq!(command.status().unwrap().signal(), Some(libc::SIGUSR1));
        })
        .join()
        .unwrap();
    }
}
