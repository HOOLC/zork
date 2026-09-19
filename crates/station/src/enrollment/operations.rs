//! Durable enrollment operation ownership and explicit local Mesh transitions.
use super::*;
use zork_client_core::mesh_enrollment::{JoinProgress, JoinRequest};
use zork_config::membership::MeshVersion;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct JoinRecord {
    request: JoinRequest,
    progress: JoinProgress,
    #[serde(default = "enrollment::now")]
    started_at: u64,
}
pub(super) fn load(root: &Path) -> Result<Option<JoinRecord>> {
    match std::fs::read(root.join("mesh/join-operation.json")) {
        Ok(bytes) => {
            ensure!(bytes.len() <= 64 * 1024, "join_record_too_large");
            Ok(Some(serde_json::from_slice(&bytes)?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

impl EnrollmentService {
    pub fn join_progress(&self, id: Option<&str>) -> Result<Option<JoinProgress>> {
        let record = self.join_state.lock().unwrap();
        if let Some(id) = id {
            ensure!(
                record.as_ref().is_some_and(|r| r.progress.id == id),
                "join_operation_not_found"
            );
        }
        Ok(record.as_ref().map(|record| record.progress.clone()))
    }
    pub fn start_join(
        self: &Arc<Self>,
        state: AppState,
        request: JoinRequest,
    ) -> Result<JoinProgress> {
        ensure!(
            request.invitation.len() <= 32 * 1024 && !request.invitation.trim().is_empty(),
            "invalid_mesh_invitation"
        );
        if let Some(name) = &request.name {
            zork_config::membership::validate_device_name(name)?;
        }
        let mut job = self.join_job.lock().unwrap();
        let mut record = self.join_state.lock().unwrap();
        if job.as_ref().is_some_and(|job| !job.is_finished()) {
            let active = record.as_ref().context("join_operation_missing")?;
            ensure!(active.request == request, "another_join_is_in_progress");
            return Ok(active.progress.clone());
        }
        let next = if let Some(previous) = record.as_ref().filter(|r| !r.progress.finished) {
            ensure!(previous.request == request, "another_join_is_in_progress");
            previous.clone()
        } else {
            JoinRecord {
                request: request.clone(),
                started_at: enrollment::now(),
                progress: JoinProgress {
                    id: ulid::Ulid::new().to_string(),
                    phase: "resolving".into(),
                    label: "正在读取邀请".into(),
                    attempt: 0,
                    retry_in_seconds: None,
                    finished: false,
                    result: None,
                    error: None,
                },
            }
        };
        let progress = next.progress.clone();
        let id = progress.id.clone();
        private_json(&self.root.join("mesh/join-operation.json"), &next)?;
        *record = Some(next);
        let service = self.clone();
        *job = Some(zork_notify::Task(tokio::spawn(async move {
            service.run_join(state, request, id).await;
        })));
        Ok(progress)
    }
    pub(crate) fn resume_join(self: &Arc<Self>, state: AppState) {
        let request = self
            .join_state
            .lock()
            .unwrap()
            .as_ref()
            .filter(|record| !record.progress.finished)
            .map(|record| record.request.clone());
        if let Some(request) = request {
            if let Err(error) = self.start_join(state, request) {
                tracing::warn!(%error, "pending Mesh join could not resume");
            }
        }
    }
    pub async fn stop_join(&self) {
        let job = self.join_job.lock().unwrap().take();
        if let Some(mut job) = job {
            job.abort();
            let _ = (&mut job).await;
        }
    }
    pub(super) fn join_phase(
        &self,
        state: &AppState,
        id: &str,
        phase: &str,
        label: &str,
    ) -> Result<()> {
        self.update_join(state, id, |progress| {
            progress.phase = phase.into();
            progress.label = label.into();
            progress.retry_in_seconds = None;
            progress.error = None;
        })
    }
    fn update_join(
        &self,
        state: &AppState,
        id: &str,
        change: impl FnOnce(&mut JoinProgress),
    ) -> Result<()> {
        let mut guard = self.join_state.lock().unwrap();
        let record = guard
            .as_mut()
            .filter(|record| record.progress.id == id)
            .context("join_operation_replaced")?;
        change(&mut record.progress);
        private_json(&self.root.join("mesh/join-operation.json"), record)?;
        drop(guard);
        state.db.realtime.notify(crate::realtime::MESH);
        Ok(())
    }
    async fn run_join(self: Arc<Self>, state: AppState, request: JoinRequest, id: String) {
        let (started_at, attempts) = {
            let record = self.join_state.lock().unwrap();
            let Some(record) = record.as_ref().filter(|record| record.progress.id == id) else {
                return;
            };
            (record.started_at, record.progress.attempt)
        };
        let mut retry = zork_mesh::retry::DiscoveryBackoff::after_failures(attempts);
        let deadline = started_at.saturating_add(enrollment::INVITE_SECONDS);
        loop {
            if self
                .update_join(&state, &id, |p| {
                    p.attempt += 1;
                    p.retry_in_seconds = None;
                })
                .is_err()
            {
                return;
            }
            let result = self
                .join_once(
                    &state,
                    &request.invitation,
                    request.name.as_deref(),
                    request.switch_from.as_ref(),
                    &id,
                )
                .await;
            match result {
                Ok(value) => {
                    let _ = self.update_join(&state, &id, |p| {
                        p.phase = "joined".into();
                        p.label = "已加入 Mesh".into();
                        p.finished = true;
                        p.result = Some(value);
                        p.error = None;
                    });
                    return;
                }
                Err(error) => {
                    let reason = error.to_string();
                    let terminal = terminal_error(&reason) || enrollment::now() >= deadline;
                    let delay = retry.next_delay();
                    let _ = self.update_join(&state, &id, |p| {
                        p.finished = terminal;
                        p.phase = if terminal { "failed" } else { "retrying" }.into();
                        p.label = if terminal {
                            zork_client_core::mesh_enrollment::failure_label(&reason)
                        } else {
                            "暂时无法连接，正在重试".into()
                        };
                        p.error = Some(reason);
                        p.retry_in_seconds = (!terminal).then_some(delay.as_secs());
                    });
                    if terminal {
                        return;
                    }
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
    pub async fn leave_mesh(&self, state: &AppState, expected: &MeshVersion) -> Result<Value> {
        let _guard = self
            .join_transaction
            .try_lock()
            .context("another_join_is_in_progress")?;
        ensure!(
            !self
                .join_job
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|job| !job.is_finished()),
            "another_join_is_in_progress"
        );
        let before = zork_config::load_config(&self.root)?.mesh;
        let mut detached = before.clone();
        zork_config::membership::detach(&mut detached, &self.origin, expected)?;
        self.archive_membership(&before)?;
        zork_config::update_config(&self.root, |config| {
            zork_config::membership::detach(&mut config.mesh, &self.origin, expected)?;
            Ok(())
        })?;
        state
            .mesh
            .get()
            .context("mesh_not_ready")?
            .refresh(state)
            .await?;
        Ok(json!({"left":true,"origin":self.origin,"data_preserved":true}))
    }
    pub(super) fn archive_membership(&self, config: &MeshConfig) -> Result<()> {
        if let Some(group) = &config.group {
            let dir = self.root.join("mesh/detached");
            std::fs::create_dir_all(&dir)?;
            private_json(&dir.join(format!("{}.json", &group.authority[4..])), config)?;
        }
        Ok(())
    }
}
fn terminal_error(error: &str) -> bool {
    error.starts_with("invalid_")
        || error.starts_with("unsupported_")
        || error.starts_with("invite_")
        || error.contains("另一环境")
        || error.contains("管理这个 Mesh")
        || matches!(
            error,
            "already_in_another_mesh"
                | "mesh_membership_changed_refresh_required"
                | "mesh_device_limit"
                | "cannot_join_this_device_to_itself"
                | "device_removed_from_mesh"
                | "client_invite_requires_client"
        )
}
