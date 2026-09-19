//! Private CEF transport. DevTools commands stay on inherited process pipes;
//! rendered pixels come from CEF's paint callback, never Chrome screencasting.
use anyhow::{anyhow, bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};
const TIMEOUT: Duration = Duration::from_secs(15);
const MAX_MESSAGE: usize = 16 * 1024 * 1024;
type Reply = std::result::Result<Value, String>;

#[cfg(target_os = "macos")]
fn ensure_graphical_session() -> Result<()> {
    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SessionGetInfo(session: u32, id: *mut u32, attributes: *mut u32) -> i32;
    }
    let mut session = 0;
    let mut attributes = 0;
    // Security/AuthSession.h: callerSecuritySession and sessionHasGraphicAccess.
    let status = unsafe { SessionGetInfo(u32::MAX, &mut session, &mut attributes) };
    ensure!(status == 0, "无法读取浏览器安全会话（{status}）");
    ensure!(
        attributes & 0x0010 != 0,
        "浏览器需要从已登录的 Zork 图形会话启动，当前会话无法安全访问登录钥匙串"
    );
    Ok(())
}

#[derive(Clone)]
pub struct Cdp(Arc<Connection>);
struct Connection {
    writer: Mutex<std::process::ChildStdin>,
    pending: Mutex<HashMap<u64, mpsc::Sender<Reply>>>,
    next: AtomicU64,
    child: Mutex<Child>,
    navigation: zork_notify::Hub<String>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        if let Ok(mut writer) = self.writer.lock() {
            let _ = writeln!(writer, "{{\"method\":\"Browser.close\"}}");
        }
        if let Ok(mut child) = self.child.lock() {
            let end = std::time::Instant::now() + Duration::from_secs(3);
            let _ = zork_notify::process::wait(&mut child, end);
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}
fn runtime_binary() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("ZORK_BROWSER_RUNTIME") {
        let path = PathBuf::from(path);
        ensure!(path.is_file(), "内置浏览器运行文件不存在");
        return Ok(path);
    }
    let executable = std::env::current_exe()?;
    let directory = executable.parent().context("应用目录不可用")?;
    #[cfg(target_os = "macos")]
    let candidates = [
        directory.join("../Helpers/ZorkBrowser.app/Contents/MacOS/ZorkBrowser"),
        directory.join("ZorkBrowser.app/Contents/MacOS/ZorkBrowser"),
        directory.join("../ZorkBrowser.app/Contents/MacOS/ZorkBrowser"),
    ];
    #[cfg(not(target_os = "macos"))]
    let candidates = [directory.join(if cfg!(windows) {
        "zork-browser-runtime.exe"
    } else {
        "zork-browser-runtime"
    })];
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .context("内置浏览器组件缺失，请重新安装完整的 Zork 应用")
}
struct RuntimeProcess {
    child: Child,
    input: std::process::ChildStdin,
    output: std::process::ChildStdout,
    log: StartupLog,
}
struct StartupLog {
    path: PathBuf,
    start: u64,
}
impl StartupLog {
    fn detail(&self) -> std::io::Result<String> {
        use std::io::{Seek, SeekFrom};
        let mut log = std::fs::File::open(&self.path)?;
        let start = self.start.max(log.metadata()?.len().saturating_sub(4096));
        log.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        log.take(4096).read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).trim().to_owned())
    }
}
fn spawn_runtime(profile: &Path, executable: &Path) -> Result<RuntimeProcess> {
    let executable = executable.canonicalize()?;
    let log_path = profile.join("process.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let startup_log = StartupLog {
        path: log_path,
        start: log.metadata()?.len(),
    };
    // Own the actual CEF process on every platform. On macOS it inherits the
    // desktop's GUI security session; no Launch Services lookup is involved.
    let mut command = Command::new(&executable);
    #[cfg(target_os = "macos")]
    command.env("MallocNanoZone", "0");
    let mut child = command
        .arg(profile)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(log)
        .spawn()
        .with_context(|| format!("启动浏览器组件 {}", executable.display()))?;
    Ok(RuntimeProcess {
        input: child.stdin.take().unwrap(),
        output: child.stdout.take().unwrap(),
        child,
        log: startup_log,
    })
}
impl Cdp {
    pub fn navigation(&self, session: &str) -> zork_notify::Changes {
        self.0
            .navigation
            .subscribe([session.to_owned(), String::new()])
    }
    pub fn close(&self) {
        let _ = self.call("Browser.close", json!({}), None);
        let mut child = self.0.child.lock().unwrap();
        let end = std::time::Instant::now() + Duration::from_secs(5);
        let _ = zork_notify::process::wait(&mut child, end);
        if matches!(child.try_wait(), Ok(None)) {
            let _ = child.kill();
        }
        let _ = child.wait();
    }
    pub fn launch(profile: &Path, event: impl Fn(Value, Vec<u8>) + Send + 'static) -> Result<Self> {
        // Check before opening the profile: a remote/non-GUI session can
        // render pages but cannot safely access existing encrypted cookies.
        #[cfg(target_os = "macos")]
        ensure_graphical_session()?;
        Self::launch_runtime(profile, &runtime_binary()?, TIMEOUT, event)
    }
    fn launch_runtime(
        profile: &Path,
        executable: &Path,
        timeout: Duration,
        event: impl Fn(Value, Vec<u8>) + Send + 'static,
    ) -> Result<Self> {
        std::fs::create_dir_all(profile)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(profile, std::fs::Permissions::from_mode(0o700))?;
        }
        let profile = profile.canonicalize()?;
        let process = spawn_runtime(&profile, executable).context("启动内置浏览器")?;
        Self::connect(process, timeout, event)
    }
    fn connect(
        process: RuntimeProcess,
        timeout: Duration,
        event: impl Fn(Value, Vec<u8>) + Send + 'static,
    ) -> Result<Self> {
        let RuntimeProcess {
            child,
            input,
            output: mut stdout,
            log,
        } = process;
        let connection = Self(Arc::new(Connection {
            writer: Mutex::new(input),
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            child: Mutex::new(child),
            navigation: Default::default(),
        }));
        let weak = Arc::downgrade(&connection.0);
        let (ready, started) = mpsc::channel();
        std::thread::Builder::new()
            .name("zork-cef-events".into())
            .spawn(move || {
                let read = || -> Result<()> {
                    loop {
                        let mut lengths = [0u8; 8];
                        stdout.read_exact(&mut lengths)?;
                        let header_size =
                            u32::from_le_bytes(lengths[0..4].try_into().unwrap()) as usize;
                        let pixel_size =
                            u32::from_le_bytes(lengths[4..8].try_into().unwrap()) as usize;
                        ensure!(
                            header_size <= MAX_MESSAGE && pixel_size <= 128 * 1024 * 1024,
                            "invalid browser packet"
                        );
                        let mut header = vec![0; header_size];
                        stdout.read_exact(&mut header)?;
                        let value: Value = serde_json::from_slice(&header)?;
                        let mut pixels = vec![0; pixel_size];
                        stdout.read_exact(&mut pixels)?;
                        let Some(inner) = weak.upgrade() else { break };
                        if value["method"] == "Zork.ready" {
                            let _ = ready.send(Ok(()));
                            continue;
                        }
                        if let Some(id) = value["id"].as_u64() {
                            if let Some(tx) = inner.pending.lock().unwrap().remove(&id) {
                                let result = if value.get("error").is_some() {
                                    Err(value["error"].to_string())
                                } else {
                                    Ok(value["result"].clone())
                                };
                                let _ = tx.send(result);
                            }
                        } else {
                            if matches!(
                                value["method"].as_str(),
                                Some(
                                    "Page.frameStartedLoading"
                                        | "Page.frameStoppedLoading"
                                        | "Page.frameNavigated"
                                        | "Page.domContentEventFired"
                                        | "Page.loadEventFired"
                                )
                            ) {
                                inner.navigation.publish([value["sessionId"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_owned()]);
                            }
                            drop(inner);
                            event(value, pixels);
                        }
                    }
                    Ok(())
                };
                let mut read = read;
                if let Err(error) = read() {
                    let _ = ready.send(Err(error.to_string()));
                }
                if let Some(inner) = weak.upgrade() {
                    inner.navigation.publish([String::new()]);
                    for (_, tx) in inner.pending.lock().unwrap().drain() {
                        let _ = tx.send(Err("内置浏览器进程已退出".into()));
                    }
                }
                event(json!({"method":"Zork.disconnected"}), vec![]);
            })?;
        let error = match started.recv_timeout(timeout) {
            Ok(Ok(())) => return Ok(connection),
            Ok(Err(e)) => anyhow!("内置浏览器启动失败：{e}"),
            Err(_) => anyhow!("内置浏览器启动超时"),
        };
        // A failed handshake must not leave a detached browser owning the
        // profile. Reap the actual child before returning or allowing a retry.
        let status = {
            let mut child = connection.0.child.lock().unwrap();
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
            }
            child.wait().ok()
        };
        let error = if let Some(status) = status {
            error.context(format!("浏览器进程已退出（{status}）"))
        } else {
            error
        };
        let detail = log.detail().unwrap_or_default();
        Err(if detail.is_empty() {
            error
        } else {
            error.context(detail)
        })
    }
    pub fn call(&self, method: &str, params: Value, session: Option<&str>) -> Result<Value> {
        let id = self.0.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.0.pending.lock().unwrap().insert(id, tx);
        let message = json!({"id":id,"method":method,"params":params,"sessionId":session});
        let sent = (|| -> Result<()> {
            let mut writer = self.0.writer.lock().unwrap();
            serde_json::to_writer(&mut *writer, &message)?;
            writer.write_all(b"\n")?;
            writer.flush()?;
            Ok(())
        })();
        if let Err(e) = sent {
            self.0.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        let result = rx.recv_timeout(TIMEOUT);
        self.0.pending.lock().unwrap().remove(&id);
        match result {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(anyhow!("{method}: {e}")),
            Err(mpsc::RecvTimeoutError::Timeout) => bail!("浏览器操作超时；请先检查页面状态"),
            Err(_) => bail!("浏览器连接已关闭"),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn script(name: &str) -> PathBuf {
        // Keep executable fixtures immutable. A concurrent fork can inherit a
        // just-written script's open descriptor and make Linux reject exec
        // with ETXTBSY until that unrelated child closes it.
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[test]
    fn failed_child_reports_this_attempts_diagnostics_and_exit_status() {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("profile");
        std::fs::create_dir(&profile).unwrap();
        std::fs::write(profile.join("process.log"), "previous-attempt-secret\n").unwrap();
        let executable = script("runtime-failure.sh");
        let error = Cdp::launch_runtime(&profile, &executable, TIMEOUT, |_, _| {})
            .err()
            .expect("runtime must fail");
        let detail = format!("{error:#}");
        assert!(detail.contains("missing-framework"), "{detail}");
        assert!(detail.contains("42"), "{detail}");
        assert!(!detail.contains("previous-attempt-secret"), "{detail}");
    }

    #[test]
    fn handshake_timeout_reaps_the_actual_runtime_before_returning() {
        let root = tempfile::tempdir().unwrap();
        let executable = script("runtime-timeout.sh");
        let profile = root.path().join("profile");
        std::fs::create_dir(&profile).unwrap();
        let process = spawn_runtime(&profile, &executable).unwrap();
        let pid = process.child.id().to_string();
        let error = Cdp::connect(process, Duration::from_millis(250), |_, _| {})
            .err()
            .expect("runtime must time out");
        assert!(format!("{error:#}").contains("启动超时"));
        let alive = Command::new("/bin/kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(!alive.success(), "timed-out runtime still alive: {pid}");
    }
}
