//! A small parent-lifetime guard reaps the process group even if Station crashes.
use super::{logs, Record};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    process::{ChildStdin, Command},
    task::JoinHandle,
};

#[derive(Serialize, Deserialize)]
struct Launch {
    command: Vec<String>,
    cwd: PathBuf,
    status: PathBuf,
    run_id: String,
}
#[derive(Default, Serialize, Deserialize)]
pub struct Status {
    pub run_id: String,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

pub struct Live {
    waiter: JoinHandle<std::io::Result<std::process::ExitStatus>>,
    force_stop: Option<tokio::sync::oneshot::Sender<()>>,
    exited: tokio::sync::watch::Receiver<bool>,
    lifetime: Option<ChildStdin>,
    output: Vec<JoinHandle<Result<()>>>,
    pub generation: u64,
    status: PathBuf,
    run_id: String,
    log_error: tokio::sync::watch::Receiver<Option<String>>,
}
impl Live {
    pub async fn spawn(root: &Path, record: &Record, wake: zork_notify::Notifier) -> Result<Self> {
        let directory = root.join("services").join(&record.id);
        let stdout = logs::Writer::open(&directory.join("logs/stdout.log")).await?;
        let stderr = logs::Writer::open(&directory.join("logs/stderr.log")).await?;
        let run_id = ulid::Ulid::new().to_string();
        let mut previous = std::fs::read_dir(&directory)?
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with("status-") && name.ends_with(".json")
            })
            .collect::<Vec<_>>();
        previous.sort_by_key(|entry| {
            entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });
        let remove = previous.len().saturating_sub(7);
        for entry in previous.into_iter().take(remove) {
            std::fs::remove_file(entry.path())?;
        }
        let launch = Launch {
            command: record.command.clone().context("external_service")?,
            cwd: record.cwd.clone().context("missing_service_cwd")?,
            status: directory.join(format!("status-{run_id}.json")),
            run_id,
        };
        let executable = std::env::current_exe()?;
        #[cfg(target_os = "macos")]
        let executable = {
            let watch = executable.with_file_name("zork-service-watch");
            // The desktop bundle registers a distinct app identity for guards.
            // Standalone node distributions keep the direct re-exec entry.
            if watch.is_file() {
                watch
            } else {
                executable
            }
        };
        let mut child = Command::new(executable)
            .arg("--service-process")
            .arg(serde_json::to_string(&launch)?)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false)
            .spawn()
            .context("start_service_guard")?;
        let lifetime = child.stdin.take();
        let out = child.stdout.take().context("service_stdout_missing")?;
        let err = child.stderr.take().context("service_stderr_missing")?;
        let (errors, log_error) = tokio::sync::watch::channel(None);
        let (force_stop, mut forced) = tokio::sync::oneshot::channel();
        let (exit, exited) = tokio::sync::watch::channel(false);
        let waiter = tokio::spawn(async move {
            let result = tokio::select! {
                result = child.wait() => result,
                forced = &mut forced => {
                    // Dropping Live closes stdin so the guard can reap its
                    // process group. Only an explicit deadline forces a kill.
                    if forced.is_ok() { let _ = child.kill().await; }
                    child.wait().await
                }
            };
            exit.send_replace(true);
            wake.notify();
            result
        });
        Ok(Self {
            waiter,
            force_stop: Some(force_stop),
            exited,
            lifetime,
            output: vec![
                tokio::spawn(logs::capture(stdout, out, errors.clone())),
                tokio::spawn(logs::capture(stderr, err, errors)),
            ],
            log_error,
            generation: record.generation,
            status: launch.status,
            run_id: launch.run_id,
        })
    }
    pub fn is_running(&self) -> bool {
        !*self.exited.borrow()
    }
    pub fn status(&self) -> Status {
        let result = std::fs::read(&self.status)
            .ok()
            .filter(|bytes| bytes.len() <= 65536)
            .and_then(|bytes| serde_json::from_slice::<Status>(&bytes).ok())
            .filter(|status| status.run_id == self.run_id);
        let mut status = result.unwrap_or_default();
        if status.error.is_none() {
            status.error = self.log_error.borrow().clone();
        }
        status
    }
    pub async fn finish(mut self, stop: bool) -> Status {
        if stop {
            self.lifetime.take();
        }
        let exit = match tokio::time::timeout(Duration::from_secs(4), &mut self.waiter).await {
            Ok(Ok(Ok(exit))) => Some(exit),
            Ok(_) => None,
            Err(_) => {
                if let Some(force_stop) = self.force_stop.take() {
                    let _ = force_stop.send(());
                }
                if tokio::time::timeout(Duration::from_secs(2), &mut self.waiter)
                    .await
                    .is_err()
                {
                    self.waiter.abort();
                }
                None
            }
        };
        self.lifetime.take();
        let mut status = self.status();
        if status.error.is_none() {
            status.error = match &exit {
                Some(exit) if !exit.success() => Some(format!("service_process_exited: {exit}")),
                None => Some("service_guard_shutdown_timeout".into()),
                _ => None,
            };
        }
        if status.exit_code.is_none() {
            status.exit_code = exit.and_then(|v| v.code());
        }
        for mut task in self.output.drain(..) {
            match tokio::time::timeout(Duration::from_secs(2), &mut task).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(error))) => {
                    status.error = Some(format!("service_log_capture_failed: {error}"))
                }
                _ => {
                    task.abort();
                    status.error = Some("service_log_capture_interrupted".into());
                }
            }
        }
        status
    }
}

/// This path executes before constructing Station's Tokio runtime or opening state.
pub fn entry(json: &str) -> Result<i32> {
    let launch: Launch = serde_json::from_str(json)?;
    ensure!(!launch.command.is_empty(), "empty_service_command");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let save = |status: &Status| -> Result<()> {
                let temp = launch
                    .status
                    .with_extension(format!("{}.tmp", launch.run_id));
                std::fs::write(&temp, serde_json::to_vec(status)?)?;
                std::fs::rename(temp, &launch.status)?;
                Ok(())
            };
            let mut command = Command::new(&launch.command[0]);
            command
                .args(&launch.command[1..])
                .current_dir(&launch.cwd)
                .stdin(Stdio::null());
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                command.as_std_mut().process_group(0);
            }
            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(error) => {
                    save(&Status {
                        run_id: launch.run_id.clone(),
                        error: Some(format!("service_spawn_failed: {error}")),
                        exit_code: Some(127),
                        ..Default::default()
                    })?;
                    return Ok(127);
                }
            };
            let pid = child.id().context("service_pid_missing")?;
            let signal_group = |signal| {
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(pid as i32), signal);
                }
            };
            // OS-delivered EOF and child-exit readiness are the only wakeups.
            let (closed, lifetime) = tokio::sync::oneshot::channel();
            std::thread::spawn(move || {
                let mut bytes = [0; 64];
                while matches!(std::io::stdin().read(&mut bytes), Ok(n) if n > 0) {}
                let _ = closed.send(());
            });
            if let Err(error) = save(&Status {
                run_id: launch.run_id.clone(),
                pid: Some(pid),
                ..Default::default()
            }) {
                signal_group(libc::SIGKILL);
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(error);
            }
            let exit = tokio::select! {
                exit = child.wait() => exit?,
                _ = lifetime => {
                    signal_group(libc::SIGTERM);
                    let graceful = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
                    signal_group(libc::SIGKILL);
                    match graceful {
                        Ok(exit) => exit?,
                        Err(_) => {
                            let _ = child.kill().await;
                            child.wait().await?
                        }
                    }
                }
            };
            // A foreground command must not leave descendants holding log pipes.
            signal_group(libc::SIGKILL);
            let code = exit.code().unwrap_or(128);
            save(&Status {
                run_id: launch.run_id.clone(),
                pid: Some(pid),
                exit_code: Some(code),
                error: (!exit.success()).then(|| format!("service_exited: {exit}")),
            })?;
            Ok(code)
        })
}
