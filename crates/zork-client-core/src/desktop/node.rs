use crate::store::SavedNode;
use anyhow::{bail, ensure, Context, Result};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};
use zork_config::service as manager;

/// A local client may own a supervisor lease or merely view an independent service.
pub struct LocalNode {
    root: PathBuf,
    process: Mutex<Option<Child>>,
    lease: Mutex<Option<std::os::unix::net::UnixStream>>,
    known_running: AtomicBool,
    background: AtomicBool,
    start_at_login: AtomicBool,
    stopping: AtomicBool,
    quitting: AtomicBool,
    wake: zork_notify::Notifier,
}
impl LocalNode {
    pub fn new(root: PathBuf) -> Self {
        let settings = manager::settings(&root).unwrap_or_default();
        Self {
            root,
            process: Mutex::new(None),
            lease: Mutex::new(None),
            known_running: AtomicBool::new(false),
            background: AtomicBool::new(settings.enabled),
            start_at_login: AtomicBool::new(settings.start_at_login),
            stopping: AtomicBool::new(false),
            quitting: AtomicBool::new(false),
            wake: Default::default(),
        }
    }
    pub fn shutdown(self: &std::sync::Arc<Self>) -> tokio::sync::oneshot::Receiver<()> {
        self.quitting.store(true, Ordering::Release);
        self.stopping.store(true, Ordering::Release);
        self.wake.notify();
        if let Ok(mut lease) = self.lease.try_lock() {
            lease.take();
        }
        if let Ok(mut process) = self.process.try_lock() {
            if let Some(child) = process.as_mut() {
                child.stdin.take();
            }
        }
        let owner = self.clone();
        let (done, completed) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            if let Ok(mut process) = owner.process.lock() {
                if let Some(mut child) = process.take() {
                    child.stdin.take();
                    // A quick relaunch must not race a still-draining owned
                    // supervisor. Background services outlive the client.
                    if !owner.background() {
                        if let Err(error) = child.wait() {
                            eprintln!("local node shutdown failed: {error}");
                        }
                    }
                }
            }
            let _ = done.send(());
        });
        completed
    }
    pub fn running(&self) -> bool {
        self.known_running.load(Ordering::Acquire)
    }
    pub fn background(&self) -> bool {
        self.background.load(Ordering::Acquire)
    }
    pub fn start_at_login(&self) -> bool {
        self.start_at_login.load(Ordering::Acquire)
    }
    /// Called on the background executor, never while GPUI is rendering.
    pub fn refresh_status(&self) -> bool {
        let running = manager::running(&self.root);
        let old = self.known_running.swap(running, Ordering::AcqRel);
        self.refresh_settings() || old != running
    }
    pub(super) fn refresh_settings(&self) -> bool {
        let settings = manager::settings(&self.root).unwrap_or_default();
        let old_background = self.background.swap(settings.enabled, Ordering::AcqRel);
        let old_login = self
            .start_at_login
            .swap(settings.start_at_login, Ordering::AcqRel);
        old_background != settings.enabled || old_login != settings.start_at_login
    }
    pub fn set_background(&self, enabled: bool, at_login: bool) -> Result<()> {
        ensure!(!self.quitting.load(Ordering::Acquire), "客户端正在退出");
        let _process = self.process.lock().expect("local node");
        if enabled {
            manager::install(&self.root, &node_binary()?, at_login)?;
        } else {
            // Acquire the client lease before unregistering the watcher. No task restart.
            if manager::running(&self.root) {
                let (lease, reply) = manager::connect(&self.root, "lease")?;
                ensure!(reply == "ok", "设备版本不支持客户端接管");
                *self.lease.lock().expect("local lease") = Some(lease);
            }
            manager::uninstall(&self.root)?;
        }
        self.background.store(enabled, Ordering::Release);
        self.start_at_login
            .store(enabled && at_login, Ordering::Release);
        Ok(())
    }
    pub fn start(&self) -> Result<SavedNode> {
        super::trace_startup("client.local_start_begin");
        self.stopping.store(false, Ordering::Release);
        let mut process = self.process.lock().expect("local node");
        ensure!(!self.quitting.load(Ordering::Acquire), "客户端正在退出");
        let binary = node_binary()?;
        let fresh = !zork_config::config_path(&self.root).exists();
        let mut config = zork_config::ensure_layout(&self.root)?;
        super::trace_startup("client.local_layout_ready");
        let already_running = self.await_supervisor()?;
        super::trace_startup("client.supervisor_checked");
        let previous_station = if already_running {
            None
        } else {
            zork_config::read_ready_pid(&self.root, "zork-station")?
        };
        let mut events = manager::Events::new(&self.root)?;
        // Readiness is an exact file source. macOS directory events can batch
        // delivery; vnode notifications announce the committed PID immediately.
        let readiness = zork_notify::files::Source::new([zork_config::ready_pid_path(
            &self.root,
            "zork-station",
        )])?;
        let mut changes = events
            .subscribe()
            .merge(readiness.subscribe())
            .merge(self.wake.subscribe());
        if !already_running {
            let services = super::load_services()?;
            services.apply_defaults(&mut config.mesh)?;
            if fresh {
                let mut listeners = vec![];
                for binding in [
                    &mut config.bind.station,
                    &mut config.bind.runtime,
                    &mut config.bind.control,
                    &mut config.bind.agent,
                ] {
                    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
                    *binding = listener.local_addr()?.to_string();
                    listeners.push(listener);
                }
                config.admin.token = zork_config::random_token();
            }
            zork_config::save_config(&self.root, &config)?;
        }
        let marker = self.root.join("desktop-node.json");
        let id = if marker.exists() {
            serde_json::from_slice::<String>(&std::fs::read(&marker)?)?
        } else {
            let id = ulid::Ulid::new().to_string();
            std::fs::write(&marker, serde_json::to_vec(&id)?)?;
            id
        };
        let token = if config.admin.token.is_empty() {
            std::fs::read(self.root.join("run/node-token.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<String>(&b).ok())
        } else {
            Some(config.admin.token.clone())
        };
        let mut node = SavedNode {
            machine_name: None,
            color_key: None,
            id,
            name: if config.mesh.name.is_empty() {
                "本机设备".into()
            } else {
                config.mesh.name.clone()
            },
            url: zork_config::loopback_base_url(&config.bind.runtime),
            token,
            local: true,
            mesh: None,
            group: None,
        };
        if !already_running {
            if manager::settings(&self.root)?.enabled {
                manager::install(&self.root, &binary, self.start_at_login())?;
            } else {
                let log = std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(self.root.join("desktop-node.log"))?;
                let mut command = Command::new(binary);
                command
                    .args(["start", "--data"])
                    .arg(&self.root)
                    .env("ZORK_PARENT_PIPE", "1")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::from(log.try_clone()?))
                    .stderr(Stdio::from(log));
                let inherited = std::env::var("PATH").unwrap_or_default();
                command.env(
                    "PATH",
                    format!("/opt/homebrew/bin:/usr/local/bin:{inherited}"),
                );
                if std::env::var_os("ZORK_DESKTOP_FAKE_AGENT").is_some() {
                    command.arg("--fake-agent");
                }
                #[cfg(unix)]
                {
                    use std::os::unix::process::CommandExt;
                    command.process_group(0);
                }
                manager::prepare_child(&mut command);
                super::trace_startup("client.before_supervisor_spawn");
                *process = Some(command.spawn().context("start local node")?);
                super::trace_startup("client.supervisor_spawned");
            }
        }
        let _child_exit = process
            .as_ref()
            .map(|child| events.watch_process(child.id()))
            .transpose()?;
        let mut retry = zork_notify::retry::Retry::default();
        let http = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(500))
            .build()?;
        // Local readiness is independent of background Mesh recovery. Retain a
        // bounded allowance for process launch and local session recovery.
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            changes.checkpoint();
            events.refresh()?;
            if self.stopping.load(Ordering::Acquire) || self.quitting.load(Ordering::Acquire) {
                break;
            }
            if let Some(child) = process.as_mut() {
                if let Some(status) = child.try_wait()? {
                    bail!("本机设备启动失败（{status}），请查看设备日志");
                }
            }
            let ready_pid = zork_config::read_ready_pid(&self.root, "zork-station")?;
            let announced =
                ready_pid.is_some() && (already_running || ready_pid != previous_station);
            if announced
                && http
                    .get(format!("{}/readyz", node.url))
                    .send()
                    .is_ok_and(|r| r.status().is_success())
            {
                if process.is_some() {
                    let (lease, reply) = manager::connect(&self.root, "lease")?;
                    ensure!(reply == "ok", "设备未能交接客户端控制权");
                    *self.lease.lock().expect("local lease") = Some(lease);
                }
                if node.token.is_none() {
                    node.token = Some(serde_json::from_slice(&std::fs::read(
                        self.root.join("run/node-token.json"),
                    )?)?);
                }
                self.known_running.store(true, Ordering::Release);
                super::trace_startup("client.local_ready");
                return Ok(node);
            }
            // A declared-ready server can still fail a request; retry that IO
            // failure. Before readiness only file/process/cancellation events wake us.
            let wake_at = if announced {
                (Instant::now() + retry.delay()).min(deadline)
            } else {
                deadline
            };
            changes.blocking_changed(wake_at)?;
        }
        self.lease.lock().expect("local lease").take();
        if let Some(child) = process.as_mut() {
            child.stdin.take();
        }
        bail!("本机设备未能就绪，请检查设备日志后重试")
    }
    pub fn stop(&self) -> Result<()> {
        self.stopping.store(true, Ordering::Release);
        self.wake.notify();
        let mut process = self.process.lock().expect("local node");
        let mut events = manager::Events::new(&self.root)?;
        let mut changes = events.subscribe();
        if manager::settings(&self.root)?.enabled {
            manager::uninstall(&self.root)?;
        }
        self.background.store(false, Ordering::Release);
        self.start_at_login.store(false, Ordering::Release);
        if manager::running(&self.root) {
            manager::connect(&self.root, "stop")?;
        }
        self.lease.lock().expect("local lease").take();
        if let Some(child) = process.as_mut() {
            child.stdin.take();
        }
        let until = Instant::now() + Duration::from_secs(12);
        loop {
            changes.checkpoint();
            events.refresh()?;
            if events.supervisor_exited()? && !manager::running(&self.root) {
                break;
            }
            ensure!(changes.blocking_changed(until)?, "Station 尚未确认停止");
        }
        if let Some(mut child) = process.take() {
            let _ = child.wait();
        }
        self.known_running.store(false, Ordering::Release);
        Ok(())
    }
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn await_supervisor(&self) -> Result<bool> {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut events = manager::Events::new(&self.root)?;
        let mut changes = events.subscribe().merge(self.wake.subscribe());
        let mut retry = zork_notify::retry::Retry::default();
        loop {
            changes.checkpoint();
            events.refresh()?;
            ensure!(
                !self.quitting.load(Ordering::Acquire) && !self.stopping.load(Ordering::Acquire),
                "客户端正在退出"
            );
            if let Ok((_, reply)) =
                manager::connect_with_timeout(&self.root, "status", Duration::from_millis(100))
            {
                let status: serde_json::Value = serde_json::from_str(&reply)?;
                ensure!(status["protocol"] == 1, "未知的 Station supervisor 协议");
                if status["client_owned"] == true && status["background"] != true {
                    // Take over a still-live desktop lease before its former
                    // client exits. Independent services keep their lifetime.
                    if let Ok((lease, reply)) = manager::connect_with_timeout(
                        &self.root,
                        "lease",
                        Duration::from_millis(100),
                    ) {
                        if reply == "ok" {
                            *self.lease.lock().expect("local lease") = Some(lease);
                            return Ok(true);
                        }
                    }
                } else {
                    return Ok(true);
                }
            }
            // GPUI may finish quitting before its supervisor finishes draining.
            // No status response is not proof that this lock has been released.
            match manager::supervisor_lock(&self.root) {
                Ok(_available) => return Ok(false),
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::WouldBlock) => {}
                Err(error) => return Err(error),
            }
            ensure!(
                Instant::now() < deadline,
                "上一个本机设备仍在退出，请稍后重试"
            );
            // The control connection failed while its owner still holds the
            // lock. Retry connection failures, also waking on startup/exit events.
            changes.blocking_changed((Instant::now() + retry.delay()).min(deadline))?;
        }
    }
}
impl Drop for LocalNode {
    fn drop(&mut self) {
        if let Ok(lease) = self.lease.get_mut() {
            lease.take();
        }
        if let Ok(process) = self.process.get_mut() {
            if let Some(child) = process.as_mut() {
                child.stdin.take();
            }
        }
    }
}
pub(super) fn node_binary() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("ZORK_NODE_BINARY") {
        let path = PathBuf::from(path);
        ensure!(path.is_file(), "ZORK_NODE_BINARY is not a file");
        return Ok(manager::launch_path(&path));
    }
    let current = std::env::current_exe()?;
    let sibling = current
        .parent()
        .context("application directory")?
        .join("zork");
    ensure!(sibling.is_file(), "此安装缺少本机设备组件，请更新客户端");
    Ok(manager::launch_path(&sibling))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn relaunch_waits_for_a_draining_supervisor_without_a_control_socket() {
        let root = tempfile::tempdir().unwrap();
        let lease = manager::supervisor_lock(root.path()).unwrap();
        let owner = Arc::new(LocalNode::new(root.path().to_owned()));
        let waiting = owner.clone();
        let (done, received) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || done.send(waiting.await_supervisor()).unwrap());
        assert!(
            received.recv_timeout(Duration::from_millis(100)).is_err(),
            "treated a draining supervisor as absent"
        );
        drop(lease);
        // Filesystem wakeups can advance the exponential retry past one second.
        // Allow the production 15-second drain deadline plus scheduling slack.
        assert!(!received
            .recv_timeout(Duration::from_secs(16))
            .unwrap()
            .unwrap());
        worker.join().unwrap();
    }

    #[test]
    fn relaunch_adopts_only_a_desktop_owned_supervisor() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;
        for client_owned in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let _lease = manager::supervisor_lock(root.path()).unwrap();
            let listener = UnixListener::bind(zork_config::zork_sock_path(root.path())).unwrap();
            let server = std::thread::spawn(move || {
                let (mut status, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(&status).read_line(&mut line).unwrap();
                assert_eq!(line.trim(), "status");
                writeln!(
                    status,
                    "{}",
                    serde_json::json!({"protocol":1,"client_owned":client_owned,"background":false})
                )
                .unwrap();
                if client_owned {
                    let (mut connection, _) = listener.accept().unwrap();
                    line.clear();
                    BufReader::new(&connection).read_line(&mut line).unwrap();
                    assert_eq!(line.trim(), "lease");
                    writeln!(connection, "ok").unwrap();
                    line.clear();
                    assert_eq!(BufReader::new(&connection).read_line(&mut line).unwrap(), 0);
                }
            });
            let owner = Arc::new(LocalNode::new(root.path().to_owned()));
            assert!(owner.await_supervisor().unwrap());
            assert_eq!(owner.lease.lock().unwrap().is_some(), client_owned);
            owner.shutdown().blocking_recv().unwrap();
            server.join().unwrap();
        }
    }

    #[test]
    fn quit_waits_for_the_owned_supervisor_to_exit() {
        let root = tempfile::tempdir().unwrap();
        let owner = Arc::new(LocalNode::new(root.path().to_owned()));
        let child = Command::new("/bin/sh")
            .args(["-c", "cat >/dev/null; sleep 0.1"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        *owner.process.lock().unwrap() = Some(child);
        owner.shutdown().blocking_recv().unwrap();
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "supervisor still owns the startup lock"
        );
    }

    #[test]
    fn quit_does_not_wait_for_a_background_supervisor() {
        let root = tempfile::tempdir().unwrap();
        let owner = Arc::new(LocalNode::new(root.path().to_owned()));
        owner.background.store(true, Ordering::Release);
        let child = Command::new("/bin/sleep").arg("2").spawn().unwrap();
        let pid = child.id() as i32;
        *owner.process.lock().unwrap() = Some(child);
        owner.shutdown().blocking_recv().unwrap();
        let running = unsafe { libc::kill(pid, 0) } == 0;
        unsafe {
            libc::kill(pid, libc::SIGTERM);
            libc::waitpid(pid, std::ptr::null_mut(), 0);
        }
        assert!(running, "background supervisor was awaited or stopped");
    }
}
