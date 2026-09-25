//! Durable enrollment operation ownership and explicit local Mesh transitions.
use super::*;
use zork_client_core::mesh_enrollment::{JoinProgress, JoinRequest};
use zork_config::membership::MeshVersion;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct JoinRecord {
    request: JoinRequest,
    progress: JoinProgress,
    /// A record written before this field existed gets one more attempt and
    /// then finishes; it must not restart its retry window on every load.
    #[serde(default)]
    started_at: u64,
    /// The inviting device once resolved, so a cancel can drop the bootstrap trust.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    inviter: Option<String>,
}
const JOIN_PATH: &str = "mesh/join-operation.json";
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
        let unfinished = record.as_ref().filter(|r| !r.progress.finished).cloned();
        if let Some(previous) = &unfinished {
            if !previous.request.same_target(&request) {
                // A different invitation replaces an unfinished join only on an
                // explicit request, and never while membership is being committed.
                ensure!(request.replace, "another_join_is_in_progress");
                ensure!(
                    previous.progress.phase != "confirming",
                    "join_confirming_try_again"
                );
                if let Some(job) = job.take() {
                    job.abort();
                }
                let mut cancelled = previous.clone();
                mark_cancelled(&mut cancelled.progress, "已放弃，改用新的邀请");
                private_json(&self.root.join(JOIN_PATH), &cancelled)?;
                *record = Some(cancelled);
                self.drop_bootstrap_trust(previous);
            } else if job.as_ref().is_some_and(|job| !job.is_finished()) {
                return Ok(previous.progress.clone());
            }
        }
        let resumed = unfinished.filter(|previous| previous.request.same_target(&request));
        let next = if let Some(previous) = resumed {
            previous
        } else {
            let mut request = request.clone();
            request.replace = false;
            JoinRecord {
                request,
                started_at: enrollment::now(),
                inviter: None,
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
        let request = next.request.clone();
        private_json(&self.root.join(JOIN_PATH), &next)?;
        *record = Some(next);
        let service = self.clone();
        *job = Some(zork_notify::Task(tokio::spawn(async move {
            service.run_join(state, request, id).await;
        })));
        Ok(progress)
    }
    /// Stop an unfinished join and finish it as cancelled. Refused only while
    /// the inviting device is committing membership (seconds); retry then.
    pub async fn cancel_join(&self, state: &AppState, id: Option<&str>) -> Result<JoinProgress> {
        let (progress, previous) = {
            let mut guard = self.join_state.lock().unwrap();
            let record = guard.as_mut().context("join_operation_not_found")?;
            if let Some(id) = id {
                ensure!(record.progress.id == id, "join_operation_not_found");
            }
            if record.progress.finished {
                return Ok(record.progress.clone());
            }
            ensure!(
                record.progress.phase != "confirming",
                "join_confirming_try_again"
            );
            // Finished before the task is stopped: any late phase update from
            // the running attempt is refused by `update_join`.
            mark_cancelled(&mut record.progress, "已取消接入");
            private_json(&self.root.join(JOIN_PATH), record)?;
            (record.progress.clone(), record.clone())
        };
        self.stop_join().await;
        self.drop_bootstrap_trust(&previous);
        state.db.realtime.notify(crate::realtime::MESH);
        Ok(progress)
    }
    /// A cancelled join may have trusted the inviter for its confirmation step.
    fn drop_bootstrap_trust(&self, record: &JoinRecord) {
        let Some(inviter) = record.inviter.clone() else {
            return;
        };
        let member = zork_config::load_config(&self.root)
            .is_ok_and(|config| config.mesh.peers.iter().any(|peer| peer.origin == inviter));
        if !member {
            let node = self.node.clone();
            tokio::spawn(async move {
                let _ = node.untrust(&inviter).await;
            });
        }
    }
    pub(super) fn join_resolved(&self, state: &AppState, id: &str, inviter: &str) -> Result<()> {
        {
            let mut guard = self.join_state.lock().unwrap();
            let record = guard
                .as_mut()
                .filter(|record| record.progress.id == id && !record.progress.finished)
                .context("join_operation_replaced")?;
            record.inviter = Some(inviter.to_owned());
            private_json(&self.root.join(JOIN_PATH), record)?;
        }
        state.db.realtime.notify(crate::realtime::MESH);
        Ok(())
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
            .filter(|record| record.progress.id == id && !record.progress.finished)
            .context("join_operation_replaced")?;
        change(&mut record.progress);
        private_json(&self.root.join(JOIN_PATH), record)?;
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
        // No invitation outlives this window; retrying past it cannot succeed.
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
                    let mut reason = error.to_string();
                    let permanent = terminal_error(&reason);
                    let delay = retry.next_delay();
                    // The next attempt would start after the invitation expired.
                    let expired =
                        !permanent && enrollment::now().saturating_add(delay.as_secs()) >= deadline;
                    if expired {
                        reason = format!("invite_expired_while_unreachable: {reason}");
                    }
                    let terminal = permanent || expired;
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
        // Tell the managing device first, while it is still a trusted peer, so
        // its directory stops listing us. Best effort: leaving never waits on
        // an offline manager, which can still remove us with `mesh remove`.
        let service = state.mesh.get().context("mesh_not_ready")?;
        let manager_notified = match before
            .group
            .as_ref()
            .filter(|group| group.authority != self.origin)
        {
            Some(group) => tokio::time::timeout(
                std::time::Duration::from_secs(8),
                service.membership_call(&group.authority, "member_left", json!({})),
            )
            .await
            .map_err(anyhow::Error::from)
            .and_then(|result| result)
            .inspect_err(
                |error| tracing::info!(%error, "Mesh manager was not told about this leave"),
            )
            .is_ok(),
            None => false,
        };
        self.archive_membership(&before)?;
        zork_config::update_config(&self.root, |config| {
            zork_config::membership::detach(&mut config.mesh, &self.origin, expected)?;
            Ok(())
        })?;
        service.refresh(state).await?;
        Ok(
            json!({"left":true,"origin":self.origin,"data_preserved":true,"manager_notified":manager_notified}),
        )
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
fn mark_cancelled(progress: &mut JoinProgress, label: &str) {
    progress.finished = true;
    progress.phase = "cancelled".into();
    progress.label = label.into();
    progress.retry_in_seconds = None;
    progress.result = None;
    progress.error = Some("join_cancelled".into());
}
pub(super) fn terminal_error(error: &str) -> bool {
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
                | "mesh_disabled"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permanent_invitation_errors_stop_retrying() {
        for reason in [
            "invite_expired",
            "invite_expired_or_restart",
            "invite_expired_or_unknown",
            "invite_not_found",
            "invite_already_used",
            "invite_claimed_by_another_device",
            "invalid_or_revoked_invite",
            "mesh_disabled",
        ] {
            assert!(terminal_error(reason), "{reason}");
        }
        for reason in [
            "enrollment_connection_timeout",
            "mesh_not_ready",
            "bootstrap_bind_timeout",
        ] {
            assert!(!terminal_error(reason), "{reason}");
        }
    }

    #[test]
    fn legacy_record_without_start_time_does_not_restart_its_window() {
        let record: JoinRecord = serde_json::from_value(json!({
            "request": {"invitation": "zj1_x", "name": null},
            "progress": {"id": ulid::Ulid::new().to_string(), "phase": "retrying", "label": "", "attempt": 3,
                "retry_in_seconds": 30, "finished": false, "result": null, "error": null}
        }))
        .unwrap();
        assert_eq!(record.started_at, 0);
        assert!(record.inviter.is_none());
        let mut progress = record.progress.clone();
        mark_cancelled(&mut progress, "已取消接入");
        assert!(progress.finished && progress.phase == "cancelled");
        assert_eq!(progress.error.as_deref(), Some("join_cancelled"));
    }
}
