use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{info, warn};
use ulid::Ulid;

use crate::config::{now_rfc3339, RuntimeConfig};
use crate::db::{JobRow, StationDb};

const MAX_RUNTIME_MS: u64 = 12 * 60 * 60 * 1000;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct JobEvent {
    pub session_key: String,
    pub job_id: String,
    pub kind: String,
    pub event_kind: String,
    pub summary: String,
}

impl JobEvent {
    fn for_job(job: &JobRow, event_kind: &str, summary: String) -> Self {
        Self {
            session_key: job.session_key.clone(),
            job_id: job.id.clone(),
            kind: job.kind.clone(),
            event_kind: event_kind.into(),
            summary,
        }
    }
}

pub struct JobSupervisor {
    db: Arc<StationDb>,
    config: RuntimeConfig,
    agent: zork_agent::Agent,
    running: Mutex<HashMap<String, Child>>,
}

impl JobSupervisor {
    pub fn new(db: Arc<StationDb>, config: RuntimeConfig, agent: zork_agent::Agent) -> Self {
        Self {
            db,
            config,
            agent,
            running: Mutex::new(HashMap::new()),
        }
    }

    pub async fn restore(self: &Arc<Self>) -> Result<()> {
        for job in self.db.list_jobs()? {
            if !matches!(job.status.as_str(), "registered" | "running") {
                continue;
            }
            if job.restart_on_boot {
                if let Err(error) = self.spawn_job(&job).await {
                    warn!(job_id = %job.id, error = %error, "restore job failed");
                    self.db.finish_job(
                        &JobEvent::for_job(
                            &job,
                            "job_failed",
                            format!("Background job could not restart: {error}"),
                        ),
                        false,
                    )?;
                } else {
                    tokio::spawn(watch_job_exit(self.clone(), job.id.clone()));
                }
            } else {
                self.db.finish_job(&JobEvent::for_job(&job, "job_interrupted",
                    "Station restarted before recording this background job's outcome. Its effects are unknown; inspect them before retrying. The job was not restarted.".into()), false)?;
            }
        }
        Ok(())
    }

    pub fn start_delivery(&self) -> zork_notify::Task<()> {
        let db = self.db.clone();
        let agent = self.agent.clone();
        let mut changes = db.realtime.listen(crate::realtime::JOB_EVENTS);
        zork_notify::Task(tokio::spawn(async move {
            let mut retry = zork_notify::retry::Retry::default();
            loop {
                changes.checkpoint();
                let mut failed = false;
                let mut progress = false;
                match db.pending_job_events() {
                    Ok(events) => {
                        for (sequence, event) in events {
                            let delivered =
                                crate::delivery::handle_job_event(&agent, &db, event, sequence)
                                    .await;
                            match delivered.and_then(|()| db.acknowledge_job_event(sequence)) {
                                Ok(()) => progress = true,
                                Err(error) => {
                                    failed = true;
                                    warn!(sequence, %error, "background result awaits mailbox retry");
                                }
                            }
                        }
                    }
                    Err(error) => {
                        failed = true;
                        warn!(%error, "background result outbox unavailable");
                    }
                }
                if progress {
                    retry.reset();
                    tokio::task::yield_now().await;
                    continue;
                }
                if failed {
                    tokio::select! { _ = retry.wait() => {}, changed = changes.changed() => if changed.is_err() { return; } }
                } else if changes.changed().await.is_err() {
                    return;
                }
            }
        }))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn register(
        self: &Arc<Self>,
        session_key: &str,
        kind: &str,
        script: &str,
        cwd: Option<&str>,
        shell: Option<&str>,
        restart_on_boot: bool,
    ) -> Result<JobRow> {
        let binding = self
            .db
            .get_binding(session_key)?
            .with_context(|| format!("Unknown session: {session_key}"))?;
        let id = Ulid::new().to_string();
        let token = zork_config::random_token();
        let now = now_rfc3339();
        let job_dir = self.config.jobs_root.join(&id);
        std::fs::create_dir_all(&job_dir)?;
        let script_path = job_dir.join("run.sh");
        let shell = shell
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("sh");
        std::fs::write(&script_path, normalize_script(script, shell))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))?;
        }
        let cwd = resolve_cwd(binding.workspace_path(), cwd);
        let job = JobRow {
            id: id.clone(),
            token,
            session_key: binding.key().to_owned(),
            kind: kind.trim().to_string(),
            shell: shell.to_string(),
            cwd: cwd.display().to_string(),
            script_path: script_path.display().to_string(),
            restart_on_boot,
            status: "registered".into(),
            created_at: now.clone(),
            updated_at: now,
        };
        self.db.insert_job(&job)?;
        if let Err(error) = self.spawn_job(&job).await {
            self.db.finish_job(
                &JobEvent::for_job(
                    &job,
                    "job_failed",
                    format!("Background job could not start: {error}"),
                ),
                false,
            )?;
            return Err(error);
        }
        tokio::spawn(watch_job_exit(self.clone(), id.clone()));
        self.db.get_job(&id)?.context("job missing after register")
    }

    pub async fn cancel(&self, job_id: &str, session_key: Option<&str>) -> Result<JobRow> {
        let job = self.db.get_job(job_id)?.context("job_not_found")?;
        if let Some(session_key) = session_key {
            if job.session_key != session_key {
                anyhow::bail!("job_session_mismatch");
            }
        }
        if !matches!(job.status.as_str(), "registered" | "running") {
            anyhow::bail!("job_not_cancellable:{}", job.status);
        }
        self.db
            .update_job_status(&job.id, "cancelled", None, None)?;
        self.stop_child(&job.id).await;
        self.db
            .get_job(&job.id)?
            .context("job missing after cancel")
    }

    pub async fn notify(
        &self,
        job_id: Option<&str>,
        summary: &str,
        session_key: &str,
    ) -> Result<&'static str> {
        if let Some(job_id) = job_id {
            let job = self.db.get_job(job_id)?.context("job_not_found")?;
            anyhow::ensure!(job.session_key == session_key, "job_session_mismatch");
        }
        let binding = self
            .db
            .get_binding(session_key)?
            .context("session_not_found")?;
        self.db.queue_job_event(&JobEvent {
            session_key: binding.key().to_owned(),
            job_id: job_id.unwrap_or("notify").to_string(),
            kind: "notify".into(),
            event_kind: "notify".into(),
            summary: summary.to_string(),
        })?;
        Ok("queued")
    }

    pub fn job_json(job: &JobRow) -> Value {
        json!({
            "id": job.id,
            "token": job.token,
            "status": job.status,
            "kind": job.kind,
            "cwd": job.cwd,
            "shell": job.shell,
            "scriptPath": job.script_path,
            "restartOnBoot": job.restart_on_boot,
            "sessionKey": job.session_key,
            "createdAt": job.created_at,
        })
    }

    async fn spawn_job(self: &Arc<Self>, job: &JobRow) -> Result<()> {
        let mut running = self.running.lock().await;
        if running.contains_key(&job.id) {
            return Ok(());
        }
        let binding = self
            .db
            .get_binding(&job.session_key)?
            .context("job session missing")?;
        let path_value = prepend_path(&self.config.zork_bin_dir);
        let mut command = Command::new(&job.script_path);
        command
            .current_dir(&job.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env("PATH", path_value)
            .env("BROKER_JOB_ID", &job.id)
            .env("BROKER_API_BASE", &self.config.broker_http_base_url)
            .env("CHAT_PLATFORM", binding.platform())
            .env("CHAT_CONNECTION_ID", binding.connection_id())
            .env("SESSION_KEY", &job.session_key)
            .env("SESSION_WORKSPACE", &job.cwd)
            .env("REPOS_ROOT", &self.config.repos_root)
            .env("WORKTREE_PATH", &job.cwd)
            .env("BACKGROUND_JOB_KIND", &job.kind);
        if let Some(conversation_id) = binding.conversation_id() {
            command.env("CHAT_CONVERSATION_ID", conversation_id);
        }
        if let Some(root_message_id) = binding.root_message_id() {
            command.env("CHAT_ROOT_MESSAGE_ID", root_message_id);
        }
        if let Some(path) = &self.config.real_gh_path {
            command.env("BROKER_REAL_GH_PATH", path);
        }
        let mut child = command.spawn().context("spawn job")?;
        if let Some(stdout) = child.stdout.take() {
            let job_id = job.id.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(job_id = %job_id, "{line}");
                }
            });
        }
        if let Some(stderr) = child.stderr.take() {
            let job_id = job.id.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    warn!(job_id = %job_id, "{line}");
                }
            });
        }
        self.db.update_job_status(&job.id, "running", None, None)?;
        let job_id = job.id.clone();
        running.insert(job.id.clone(), child);
        drop(running);
        // Explicitly restartable service jobs are persistent processes, not
        // bounded batch work. Their lifetime ends on cancellation or process exit.
        if !(job.kind == "service" && job.restart_on_boot) {
            let supervisor = Arc::downgrade(self);
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(MAX_RUNTIME_MS)).await;
                let Some(supervisor) = supervisor.upgrade() else {
                    return;
                };
                if let Ok(Some(job)) = supervisor.db.get_job(&job_id) {
                    match supervisor.db.finish_job(
                        &JobEvent::for_job(
                            &job,
                            "job_timeout",
                            "Background job timed out after 12h.".into(),
                        ),
                        false,
                    ) {
                        Ok(true) => supervisor.stop_child(&job_id).await,
                        Ok(false) => {}
                        Err(error) => {
                            warn!(job_id = %job_id, %error, "job timeout persistence failed")
                        }
                    }
                }
            });
        }
        info!(job_id = %job.id, "background job started");
        Ok(())
    }

    async fn stop_child(&self, job_id: &str) {
        if let Some(mut child) = self.running.lock().await.remove(job_id) {
            let _ = child.kill().await;
        }
    }
}

pub async fn watch_job_exit(supervisor: Arc<JobSupervisor>, job_id: String) {
    let mut exits = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::child()) {
        Ok(exits) => exits,
        Err(error) => {
            warn!(%error,"job exit notification unavailable");
            return;
        }
    };
    let mut retry = zork_notify::retry::Retry::default();
    loop {
        let mut running = supervisor.running.lock().await;
        let Some(child) = running.get_mut(&job_id) else {
            return;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                let ok = status.success();
                let summary = if ok {
                    format!("Background job {job_id} finished.")
                } else {
                    format!("Background job {job_id} failed ({status}).")
                };
                let committed = supervisor.db.get_job(&job_id).and_then(|job| {
                    let job = job.context("job_not_found")?;
                    supervisor.db.finish_job(
                        &JobEvent::for_job(
                            &job,
                            if ok { "job_exit" } else { "job_failed" },
                            summary,
                        ),
                        ok,
                    )
                });
                if let Err(error) = committed {
                    // try_wait retains the observed status. Do not discard it
                    // before both the terminal row and notification are durable.
                    drop(running);
                    warn!(job_id = %job_id, %error, "job completion persistence awaits retry");
                    retry.wait().await;
                    continue;
                }
                running.remove(&job_id);
                return;
            }
            Ok(None) => {
                drop(running);
                if exits.recv().await.is_none() {
                    return;
                }
            }
            Err(error) => {
                warn!(job_id = %job_id, error = %error, "job wait failed");
                return;
            }
        }
    }
}

fn normalize_script(script: &str, shell: &str) -> String {
    if script.starts_with("#!") {
        return if script.ends_with('\n') {
            script.to_string()
        } else {
            format!("{script}\n")
        };
    }
    format!("#!/usr/bin/env {shell}\nset -euo pipefail\n{script}\n")
}

fn resolve_cwd(workspace: &str, cwd: Option<&str>) -> PathBuf {
    match cwd.map(str::trim).filter(|value| !value.is_empty()) {
        Some(path) if Path::new(path).is_absolute() => PathBuf::from(path),
        Some(path) => Path::new(workspace).join(path),
        None => PathBuf::from(workspace),
    }
}

fn prepend_path(bin_dir: &Path) -> std::ffi::OsString {
    let mut entries = vec![bin_dir.to_path_buf()];
    if let Some(path) = std::env::var_os("PATH") {
        entries.extend(std::env::split_paths(&path));
    }
    std::env::join_paths(entries).unwrap_or_else(|_| bin_dir.as_os_str().to_os_string())
}
