use super::*;
use std::{io::SeekFrom, process::Stdio};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
const MAX_OUTPUT: usize = 8 * 1024 * 1024;
struct Child(tokio::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0.id() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        let _ = self.0.start_kill();
    }
}
fn output(state: &AppState, id: &str) -> std::path::PathBuf {
    zork_config::files_root(&state.config.data_root).join("device-output").join(id)
}
pub async fn run(
    state: &AppState,
    rpc: &Rpc,
    id: &str,
    mut stopped: watch::Receiver<bool>,
    local: bool,
) -> Result<()> {
    let script = rpc.arguments["command"]
        .as_str()
        .filter(|s| !s.is_empty())
        .context("device_missing_parameter")?;
    ensure!(script.len() <= 64 * 1024, "device_command_limit");
    let root = zork_config::files_root(&state.config.data_root)
        .join("device-workspaces")
        .join(fingerprint(&rpc.subject)?);
    let cwd = rpc
        .arguments
        .get("cwd")
        .and_then(Value::as_str)
        .map(std::path::PathBuf::from)
        .unwrap_or(root);
    ensure!(cwd.is_absolute(), "device_absolute_cwd_required");
    tokio::fs::create_dir_all(&cwd).await?;
    let environment: std::collections::BTreeMap<String, String> = rpc
        .arguments
        .get("env")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    ensure!(environment.len() <= 64, "device_environment_limit");
    let path = output(state, id);
    tokio::fs::create_dir_all(path.parent().context("device_output_path")?).await?;
    let mut log = tokio::fs::File::create(&path).await?;
    #[cfg(not(windows))]
    let mut command = {
        let mut c = tokio::process::Command::new("/bin/sh");
        c.arg("-c").arg(format!("exec 2>&1\n{script}"));
        c
    };
    #[cfg(windows)]
    let mut command = {
        let mut c = tokio::process::Command::new("cmd.exe");
        c.args(["/D", "/S", "/C"]).arg(format!("{script} 2>&1"));
        c
    };
    command
        .current_dir(&cwd)
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    node_access::manage(state, &rpc.subject, local)?;
    if *stopped.borrow() {
        return state.node_tools.store.finish(
            id,
            "cancelled",
            Some(json!({
                "cwd":cwd,"process_state":"not_started","effects_may_have_occurred":false
            })),
        );
    }
    state.node_tools.store.finish(
        id,
        "dispatching",
        Some(json!({"cwd":cwd,"process_state":"starting"})),
    )?;
    let mut child = match command.spawn() {
        Ok(child) => Child(child),
        Err(_) => return state.node_tools.store.finish(id, "failed", Some(json!({
            "cwd":cwd,"process_state":"not_started","effects_may_have_occurred":false,"error":"device_spawn_failed"
        }))),
    };
    let pid = child.0.id();
    let mut stdout = child.0.stdout.take().context("device_output_missing")?;
    if let Err(error) = state.node_tools.store.finish(
        id,
        "running",
        Some(json!({"cwd":cwd,"pid":pid,"process_state":"running"})),
    ) {
        let _ = terminate(&mut child).await;
        return Err(error);
    }
    let mut total = 0;
    let outcome = {
        let capture = async {
            let mut buffer = [0u8; 8192];
            loop {
                let n = stdout.read(&mut buffer).await?;
                if n == 0 {
                    break;
                }
                ensure!(total + n <= MAX_OUTPUT, "device_output_limit");
                log.write_all(&buffer[..n]).await?;
                total += n;
                log.flush().await?;
                state.node_tools.store.notify();
            }
            log.flush().await?;
            Ok::<_, anyhow::Error>(child.0.wait().await?)
        };
        let mut policy = state.db.realtime.listen(crate::realtime::MESH);
        let revoked = policy.until(|| node_access::manage(state, &rpc.subject, local).err());
        tokio::select! {
            result = capture => result.map_err(|e| ("failed", error(&e))),
            _ = cancellation(&mut stopped) => Err(("cancelled", "device_interrupted".into())),
            _ = revoked => Err(("failed", "node_management_denied".into())),
        }
    };
    let (status, state_name, reason) = match outcome {
        Ok(status) => (
            Some(status),
            if status.success() {
                "succeeded"
            } else {
                "failed"
            },
            None,
        ),
        Err((terminal, reason)) => {
            // Keep ownership until termination is observed. Cancellation stops
            // the process; it does not roll back already completed effects.
            match terminate(&mut child).await {
                Ok(status) => (Some(status), terminal, Some(reason)),
                Err(_) => (
                    None,
                    "outcome_unknown",
                    Some("device_termination_unconfirmed".into()),
                ),
            }
        }
    };
    let _ = log.flush().await;
    let bytes = log
        .metadata()
        .await
        .map(|m| m.len())
        .unwrap_or(total as u64);
    let mut result = json!({
        "cwd":cwd,"pid":pid,"process_state":if status.is_some(){"exited"}else{"unknown"},
        "exit_code":status.and_then(|s|s.code()),"output_bytes":bytes,"read_with":"device.read",
        "effects_may_have_occurred":true
    });
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        result["exit_signal"] = json!(status.and_then(|s| s.signal()));
    }
    if let Some(reason) = reason {
        result["error"] = json!(reason);
    }
    state.node_tools.store.finish(id, state_name, Some(result))
}

async fn cancellation(stopped: &mut watch::Receiver<bool>) {
    let requested = *stopped.borrow_and_update();
    if !requested {
        let _ = stopped.changed().await;
    }
}

async fn terminate(child: &mut Child) -> Result<std::process::ExitStatus> {
    #[cfg(unix)]
    if let Some(pid) = child.0.id() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.0.start_kill();
    tokio::time::timeout(Duration::from_secs(10), child.0.wait())
        .await
        .context("device_termination_unconfirmed")?
        .context("device_termination_unconfirmed")
}

pub async fn status(state: &AppState, rpc: &Rpc) -> Result<Value> {
    let id = field(&rpc.arguments, "operation_id")?;
    let value = state.node_tools.store.view(id, &rpc.subject)?;
    if rpc.tool == "device.cancel" {
        let requested =
            if let Some(stop) = state.node_tools.active.lock().expect("device jobs").get(id) {
                stop.send_replace(true);
                true
            } else {
                false
            };
        let mut value = value;
        value["cancel_requested"] = json!(requested);
        return Ok(value);
    }
    if rpc.tool == "device.read" {
        ensure!(value["tool"] == "device.exec", "device_no_process_output");
        let mut file = tokio::fs::File::open(output(state, id))
            .await
            .context("device_output_unavailable")?;
        let offset = rpc
            .arguments
            .get("offset")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        ensure!(
            offset <= file.metadata().await?.len(),
            "device_invalid_offset"
        );
        file.seek(SeekFrom::Start(offset)).await?;
        let mut data = vec![0; 32 * 1024];
        let read = file.read(&mut data).await?;
        data.truncate(read);
        // Base64 keeps byte offsets exact even for binary output or split UTF-8.
        use base64::Engine;
        return Ok(
            json!({"operation_id":id,"state":value["state"],"offset":offset,"base64":base64::engine::general_purpose::STANDARD.encode(&data),"text":String::from_utf8_lossy(&data),"next_offset":offset+read as u64,"eof":offset+read as u64==file.metadata().await?.len()}),
        );
    }
    Ok(value)
}
