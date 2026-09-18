use anyhow::{ensure, Context, Result};
use std::{fs, io::Write, path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use zork_config::update;

pub async fn run(mut argv: Vec<String>) -> Result<()> {
    let at = argv
        .iter()
        .position(|v| v == "--version")
        .context("--version is required")?;
    ensure!(at + 1 < argv.len(), "--version is required");
    let version = argv.remove(at + 1);
    argv.remove(at);
    ensure!(update::valid_version(&version), "Invalid release version");
    let args = zork_config::parse_process_args_from(argv)?;
    let root = args.data_root.canonicalize()?;
    update::eligible(&root, &std::env::current_exe()?)?;
    let _lock = zork_config::service::exclusive_lock(&root.join("run/update.lock"))
        .context("An update is already running")?;
    update::write_state(&root, "downloading", &version, "正在下载并校验完整版本包")?;
    println!("accepted");
    std::io::stdout().flush()?;
    let result = perform(&root, &version).await;
    if let Err(error) = &result {
        update::write_state(&root, "failed", &version, &error.to_string())?;
    }
    result
}

async fn perform(root: &Path, version: &str) -> Result<()> {
    let stage = root.join("run/update-stage");
    if stage.exists() {
        fs::remove_dir_all(&stage)?;
    }
    let installer = root.join("run/update-download.sh");
    fs::write(&installer, include_str!("../../../scripts/install.sh"))?;
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("logs/update.log"))?;
    let status = tokio::process::Command::new("sh")
        .arg(&installer)
        .arg("--base-url")
        .arg(update::RELEASE_BASE)
        .arg("--version")
        .arg(version)
        .arg("--download-only")
        .arg(&stage)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log))
        .status()
        .await?;
    ensure!(
        status.success(),
        "版本包下载或校验失败，当前版本未改动；详情见 logs/update.log"
    );
    // This capability prevents handing control to an older supervisor that cannot
    // participate in future upgrades.
    let output = tokio::process::Command::new(stage.join("zork"))
        .arg("capabilities")
        .output()
        .await?;
    ensure!(
        output.status.success()
            && serde_json::from_slice::<serde_json::Value>(&output.stdout)?["native_update"] == 1,
        "目标版本不支持完整节点升级，当前版本未改动"
    );
    update::write_state(
        root,
        "restarting",
        version,
        "正在切换版本并重启设备，连接将暂时中断",
    )?;
    let previous_station = zork_config::read_ready_pid(root, "zork-station")?;
    let mut socket = tokio::net::UnixStream::connect(zork_config::zork_sock_path(root)).await?;
    socket.write_all(b"activate-update\n").await?;
    let mut reply = String::new();
    tokio::time::timeout(
        Duration::from_secs(60),
        BufReader::new(socket).read_line(&mut reply),
    )
    .await??;
    ensure!(
        reply.trim() == "accepted",
        "版本切换未接受：{}",
        reply.trim()
    );
    let config = zork_config::load_config(root)?;
    let base = zork_config::loopback_base_url(&config.bind.runtime);
    let token: String = serde_json::from_slice(&fs::read(root.join("run/node-token.json"))?)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    let mut events = zork_config::service::Events::new(root)?;
    let mut changes = events.subscribe();
    let mut retry = zork_notify::retry::Retry::default();
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            changes.checkpoint();
            events.refresh()?;
            let operation = update::state(root);
            ensure!(operation["phase"] != "failed", "{}",
                operation["message"].as_str().unwrap_or("版本切换失败"));
            let mut failed = false;
            if zork_config::read_ready_pid(root, "zork-station")?.is_some_and(|pid| Some(pid) != previous_station) {
                let verify = async {
                    let info: serde_json::Value = client.get(format!("{base}/v1/node/info"))
                        .bearer_auth(&token).send().await?.error_for_status()?.json().await?;
                    ensure!(info["station"]["release_version"] == version, "设备尚未切换到目标版本");
                    let agent_base = zork_config::loopback_base_url(&config.bind.agent);
                    client.get(format!("{agent_base}/readyz")).send().await?.error_for_status()?;
                    Ok::<_, anyhow::Error>(())
                }.await;
                if verify.is_ok() {
                    update::write_state(root, "complete", version, "版本升级完成")?;
                    return Ok(());
                }
                failed = true;
            }
            if !failed { retry.reset(); }
            tokio::select! {
                changed = changes.changed() => { changed?; },
                _ = retry.wait(), if failed => {},
            }
        }
    }).await.context("新版本未在两分钟内恢复连接。旧版本保留在 run/update-previous-*；请检查设备日志，升级后的数据不会自动回退")?
}

/// Called only by the supervisor after the Station (including its Agent) has exited. Renaming the
/// complete directory prevents mixing components. exec preserves the supervisor
/// PID, background service relationship, and original startup arguments.
pub fn activate(root: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let stage = root.join("run/update-stage");
    let previous = root.join(format!(
        "run/update-previous-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let bin = root.join("bin");
    swap_directories(&bin, &stage)?;
    if let Err(error) = fs::rename(&stage, &previous) {
        swap_directories(&bin, &stage)?;
        return Err(error.into());
    }
    let mut command = std::process::Command::new(bin.join("zork"));
    command.args(std::env::args_os().skip(1));
    zork_config::service::prepare_child(&mut command);
    let error = command.exec();
    // exec did not start: no new code has accessed data, so restoring is safe.
    swap_directories(&bin, &previous)?;
    Err(error).context("启动新版本失败，已恢复原版本")
}

fn swap_directories(left: &Path, right: &Path) -> Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let left = CString::new(left.as_os_str().as_bytes())?;
    let right = CString::new(right.as_os_str().as_bytes())?;
    // Both directories are on the node's filesystem. Exchange leaves `bin`
    // valid even if the machine stops between the switch and exec.
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            left.as_ptr(),
            libc::AT_FDCWD,
            right.as_ptr(),
            libc::RENAME_SWAP,
        )
    };
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            left.as_ptr(),
            libc::AT_FDCWD,
            right.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    anyhow::bail!("Native updates require macOS or Linux");
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("无法原子切换版本目录，原版本未改动");
    }
    Ok(())
}
