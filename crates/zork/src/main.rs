use std::fs;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

mod account;
mod mcp;
mod mesh;
mod service;
#[cfg(all(test, unix))]
mod tests;
mod upgrade;

fn usage() -> &'static str {
    "\
Usage:
  zork start [--data DIR] [--listen HOST] [--agent-token TOKEN]
  zork install [--data DIR] [--name DEVICE_NAME]
  zork update [--data DIR]
  zork upgrade --version X.Y.Z [--data DIR]
  zork account login|status|logout [--data DIR]
  zork mesh invite|join|status [--data DIR]
  zork mcp list|add FILE|get ID|probe ID|enable ID|disable ID|remove ID [--data DIR]
  zork mcp update ID FILE [--data DIR]
  zork service install|uninstall|status [--data DIR] [--at-login]
  zork stop [--data DIR]

start   runs zork-station with embedded Agent (Slack + mailbox delivery + admin)
update  drains and restarts zork-station, including its embedded Agent
"
}

fn main() -> Result<()> {
    zork_config::startup::mark("supervisor.main");
    let identity = zork_config::service::prepare_process_identity()?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(identity))
}

async fn run(identity: zork_config::service::ProcessIdentity) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?),
        )
        .init();

    let mut argv: Vec<String> = std::env::args().skip(1).collect();
    let command = if argv.first().is_some_and(|arg| !arg.starts_with('-')) {
        argv.remove(0)
    } else {
        "start".into()
    };
    if argv.iter().any(|arg| arg == "--help" || arg == "-h") {
        identity.register()?;
        print!("{}", usage());
        return Ok(());
    }
    if command == "start" {
        return run_supervisor(argv, identity).await;
    }
    identity.register()?;
    match command.as_str() {
        "install" => {
            argv.insert(0, "install".into());
            mesh::run(argv).await
        }
        "update" => send_reload(argv).await,
        "upgrade" => upgrade::run(argv).await,
        "account" => account::run(argv).await,
        "mesh" => mesh::run(argv).await,
        "mcp" => mcp::run(argv).await,
        "service" => service::command(argv).await,
        "service-run" => service::run(argv).await,
        "stop" => service::stop(argv).await,
        "capabilities" => {
            println!("{{\"mesh_join\":1,\"supervisor\":1,\"native_update\":1}}");
            Ok(())
        }
        other => anyhow::bail!("unknown command {other}\n{}", usage().trim()),
    }
}

async fn send_reload(argv: Vec<String>) -> Result<()> {
    let args = zork_config::parse_process_args_from(argv)?;
    let sock = zork_config::zork_sock_path(&args.data_root);
    // An old supervisor still starts a second Agent and cannot hot-reload this process model.
    let mut status = UnixStream::connect(&sock)
        .await
        .with_context(|| format!("zork is not running ({})", sock.display()))?;
    status.write_all(b"status\n").await?;
    let mut reply = String::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        BufReader::new(status).read_line(&mut reply),
    )
    .await??;
    let status: serde_json::Value = serde_json::from_str(&reply)?;
    anyhow::ensure!(
        status["agent_mode"] == "embedded",
        "the running supervisor uses a standalone Agent; restart the zork supervisor once to activate the embedded Agent (hot reload is not supported for this migration)"
    );
    let mut stream = UnixStream::connect(&sock)
        .await
        .with_context(|| format!("zork is not running ({})", sock.display()))?;
    stream.write_all(b"reload\n").await?;
    stream.flush().await?;
    let mut buf = String::new();
    BufReader::new(stream).read_line(&mut buf).await?;
    let reply = buf.trim();
    if reply != "ok" {
        anyhow::bail!("reload failed: {reply}");
    }
    println!("updated");
    Ok(())
}

async fn run_supervisor(
    argv: Vec<String>,
    identity: zork_config::service::ProcessIdentity,
) -> Result<()> {
    // Arm signals before launching any child, including while macOS registers
    // this process identity. Early shutdown must not leave an orphaned Station.
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let mut hangup = Box::pin(sighup());
    let args = zork_config::parse_process_args_from(argv)?;
    let mut file = zork_config::ensure_layout(&args.data_root)?;
    zork_config::startup::mark("supervisor.layout_ready");
    let _instance = zork_config::service::supervisor_lock(&args.data_root)?;
    zork_config::service::register(&args.data_root)?;
    if let Some(host) = &args.listen_host {
        zork_config::apply_listen(&mut file, host);
    }
    let missing = zork_config::missing_commands(&["git", "gh", "rg"]);
    if !missing.is_empty() {
        eprintln!(
            "missing on PATH (needed for agent turns): {}",
            missing.join(", ")
        );
    }

    let admin = zork_config::loopback_base_url(&file.bind.control);
    println!("admin API  {admin}");
    println!("data   {}", args.data_root.display());
    if !zork_config::has_configured_im_connection(&file) {
        println!("setup  use the desktop client; configure an IM connection for Slack");
    }

    let pid_path = zork_config::zork_pid_path(&args.data_root);
    fs::write(&pid_path, format!("{}\n", std::process::id()))?;
    let sock_path = zork_config::zork_sock_path(&args.data_root);
    let _ = fs::remove_file(&sock_path);

    zork_config::startup::mark("supervisor.before_station_spawn");
    let mut station = spawn_named("zork-station", &args)?;
    zork_config::startup::mark("supervisor.station_spawned");
    // Station can initialize while Launch Services checks in its supervisor.
    // Publish the control socket only after that registration succeeds.
    if let Err(error) = identity.register() {
        terminate_child(&mut station);
        if wait_for_exit(&mut station, Duration::from_secs(8))
            .await
            .is_err()
        {
            let _ = station.kill().await;
        }
        let _ = fs::remove_file(&pid_path);
        return Err(error);
    }

    let (reload_tx, mut reload_rx) = mpsc::channel::<SupervisorCommand>(16);
    let sock_for_listen = sock_path.clone();
    let tx = reload_tx.clone();
    tokio::spawn(async move {
        if let Err(error) = listen_reload(&sock_for_listen, tx).await {
            warn!(error = %error, "reload socket ended");
        }
    });

    info!("zork started");
    let mut parent_closed: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
        Box::pin(wait_parent_pipe());
    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => break,
            status = station.wait() => {
                log_exit("station", status);
                tokio::time::sleep(Duration::from_millis(500)).await;
                station = spawn_named("zork-station", &args)?;
            }
            req = reload_rx.recv() => {
                let Some(command) = req else { break };
                let (ack,_mesh_only)=match command {
                    SupervisorCommand::Status(ack)=>{
                        let _=ack.send(serde_json::json!({"protocol":1,"agent_mode":"embedded","pid":std::process::id(),"data_root":args.data_root,"client_owned":std::env::var_os("ZORK_PARENT_PIPE").is_some(),"background":zork_config::service::settings(&args.data_root).is_ok_and(|s|s.enabled)}).to_string());
                        continue;
                    }
                    SupervisorCommand::Lease(ack,closed)=>{
                        parent_closed=Box::pin(async move {let _=closed.await;});
                        let _=ack.send("ok".into());continue;
                    }
                    SupervisorCommand::Stop(ack)=>{let _=ack.send("ok".into());break;}
                    SupervisorCommand::Upgrade(ack)=>{
                        if let Err(error) = zork_config::update::eligible(&args.data_root, &std::env::current_exe()?) {
                            let _ = ack.send(format!("error: {error}")); continue;
                        }
                        if !args.data_root.join("run/update-stage/zork").is_file() {
                            let _ = ack.send("error: staged release missing".into()); continue;
                        }
                        let _ = ack.send("accepted".into());
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        terminate_child(&mut station);
                        let stopped = wait_for_exit(&mut station,Duration::from_secs(30)).await;
                        let result = match stopped {Ok(_)=>upgrade::activate(&args.data_root),Err(error)=>Err(error)};
                        if let Err(error) = result {
                            let version=zork_config::update::state(&args.data_root)["version"].as_str().unwrap_or_default().to_owned();
                            let _=zork_config::update::write_state(&args.data_root,"failed",&version,&error.to_string());
                        }
                        if station.try_wait()?.is_some(){station=spawn_named("zork-station",&args)?;}
                        continue;
                    }
                    SupervisorCommand::Reload(ack,mesh_only)=>(ack,mesh_only),
                };
                info!("zork update starting");
                let result = controlled_restart(
                    &args,
                    &mut station,
                )
                .await;
                match result {
                    Ok(()) => {
                        info!("zork update complete");
                        let _ = ack.send("ok".into());
                    }
                    Err(error) => {
                        error!(error = %error, "zork update failed");
                        let _ = ack.send(format!("error: {error}"));
                    }
                }
            }
            _ = &mut parent_closed => {
                if zork_config::service::settings(&args.data_root).is_ok_and(|s|s.enabled) {
                    parent_closed=Box::pin(std::future::pending());
                } else {break;}
            }
            _ = &mut hangup => {
                hangup = Box::pin(sighup());
                info!("zork update starting");
                if let Err(error) = controlled_restart(
                    &args,
                    &mut station,
                )
                .await
                {
                    error!(error = %error, "zork update failed");
                } else {
                    info!("zork update complete");
                }
            }
        }
    }
    info!("zork shutting down");
    terminate_child(&mut station);
    let _ = tokio::time::timeout(Duration::from_secs(8), station.wait()).await;
    let _ = fs::remove_file(&sock_path);
    let _ = fs::remove_file(&pid_path);
    Ok(())
}

type OneshotAck = tokio::sync::oneshot::Sender<String>;

enum SupervisorCommand {
    Reload(OneshotAck, bool),
    Upgrade(OneshotAck),
    Status(OneshotAck),
    Stop(OneshotAck),
    Lease(OneshotAck, tokio::sync::oneshot::Receiver<()>),
}

async fn listen_reload(sock: &std::path::Path, tx: mpsc::Sender<SupervisorCommand>) -> Result<()> {
    let listener = UnixListener::bind(sock).with_context(|| format!("bind {}", sock.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(sock, fs::Permissions::from_mode(0o600))?;
    }
    loop {
        let (stream, _) = listener.accept().await?;
        let tx = tx.clone();
        tokio::spawn(async move {
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut line = String::new();
            if !matches!(
                tokio::time::timeout(Duration::from_secs(3), reader.read_line(&mut line)).await,
                Ok(Ok(1..=64))
            ) {
                return;
            }
            let (ack, rx) = tokio::sync::oneshot::channel();
            let (closed, close_rx) = tokio::sync::oneshot::channel();
            let request = match line.trim() {
                "activate-update" => SupervisorCommand::Upgrade(ack),
                "status" | "observe" => SupervisorCommand::Status(ack),
                "lease" => SupervisorCommand::Lease(ack, close_rx),
                "stop" => SupervisorCommand::Stop(ack),
                "reload" | "reload-mesh" => {
                    SupervisorCommand::Reload(ack, line.trim() == "reload-mesh")
                }
                _ => {
                    let _ = writer.write_all(b"error: unknown command\n").await;
                    return;
                }
            };
            if tx.send(request).await.is_err() {
                return;
            }
            let reply = rx.await.unwrap_or_else(|_| "error: cancelled".into());
            if writer
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .is_ok()
                && matches!(line.trim(), "lease" | "observe")
            {
                let mut byte = [0u8; 1];
                while matches!(reader.read(&mut byte).await, Ok(1..)) {}
            }
            let _ = closed.send(());
        });
    }
}

async fn controlled_restart(args: &zork_config::ProcessArgs, station: &mut Child) -> Result<()> {
    restart_named("zork-station", station, args).await
}

/// Desktop-owned nodes receive a private stdin pipe. EOF also handles a killed
/// or crashed GUI; no PID polling or independent login service is needed.
async fn wait_parent_pipe() {
    if std::env::var_os("ZORK_PARENT_PIPE").is_none() {
        std::future::pending::<()>().await;
        return;
    }
    // AsyncFd makes this lease cancellable. tokio::io::stdin uses a blocking
    // read that would keep the supervisor alive after an explicit socket stop.
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let fd = unsafe { libc::dup(libc::STDIN_FILENO) };
    if fd < 0 {
        return;
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return;
    }
    let Ok(input) = tokio::io::unix::AsyncFd::new(fd) else {
        return;
    };
    let mut bytes = [0u8; 64];
    loop {
        let Ok(mut ready) = input.readable().await else {
            return;
        };
        match ready.try_io(|input| {
            let count = unsafe {
                libc::read(
                    input.get_ref().as_raw_fd(),
                    bytes.as_mut_ptr().cast(),
                    bytes.len(),
                )
            };
            if count < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(count)
            }
        }) {
            Ok(Ok(0)) | Ok(Err(_)) => return,
            _ => {}
        }
    }
}

async fn restart_named(
    name: &str,
    current: &mut Child,
    args: &zork_config::ProcessArgs,
) -> Result<()> {
    info!(name, "draining current process");
    terminate_child(current);
    if wait_for_exit(current, Duration::from_secs(8))
        .await
        .is_err()
    {
        let _ = current.start_kill();
        wait_for_exit(current, Duration::from_secs(3)).await?;
    }
    info!(name, "starting updated process");
    let mut next = spawn_named(name, args)?;
    let pid = next.id().context("updated process pid")?;
    if let Err(error) = wait_ready(name, &args.data_root, pid).await {
        terminate_child(&mut next);
        let _ = next.wait().await;
        return Err(error).with_context(|| format!("{name} readyz"));
    }
    *current = next;
    info!(name, pid, "restarted");
    Ok(())
}

/// Waiting is owned by the process handle; the deadline only bounds failure.
async fn wait_for_exit(child: &mut Child, timeout: Duration) -> Result<()> {
    tokio::time::timeout(timeout, child.wait())
        .await
        .context("child did not confirm exit before deadline")??;
    Ok(())
}

async fn wait_ready(name: &str, data_root: &std::path::Path, pid: u32) -> Result<()> {
    let events = zork_config::service::Events::new(data_root)?;
    let process = events.watch_process(pid)?;
    let mut changes = events.subscribe();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            changes.checkpoint();
            anyhow::ensure!(!process.exited()?, "process exited before readiness");
            if zork_config::read_ready_pid(data_root, name)? == Some(pid) {
                return Ok::<_, anyhow::Error>(());
            }
            changes.changed().await?;
        }
    })
    .await
    .context("process did not become ready before deadline")?
}

fn log_exit(name: &str, status: std::result::Result<std::process::ExitStatus, std::io::Error>) {
    match status {
        Ok(status) if status.success() => info!(name, "process exited"),
        Ok(status) => error!(name, %status, "process exited"),
        Err(error) => error!(name, error = %error, "wait failed"),
    }
}

fn spawn_named(name: &str, args: &zork_config::ProcessArgs) -> Result<Child> {
    let bin = zork_config::find_bin(name, &args.data_root)
        .with_context(|| format!("{name} not found next to zork or on PATH"))?;
    let mut command = Command::new(zork_config::service::launch_path(&bin));
    command
        .arg("--data")
        .arg(&args.data_root)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    if let Some(host) = &args.listen_host {
        command.arg("--listen").arg(host);
    }
    if let Some(token) = &args.agent_token {
        command.arg("--agent-token").arg(token);
    }
    if args.no_streaming {
        command.arg("--no-streaming");
    }
    if args.fake_agent {
        command.arg("--fake-agent");
    }
    zork_config::service::prepare_child(command.as_std_mut());
    command
        .spawn()
        .with_context(|| format!("spawn {}", bin.display()))
}

fn terminate_child(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.start_kill();
    }
}

fn shutdown_signal() -> impl std::future::Future<Output = ()> {
    #[cfg(unix)]
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("listen for SIGTERM");
    #[cfg(unix)]
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .expect("listen for SIGINT");
    async move {
        #[cfg(unix)]
        tokio::select! {
            _ = sigint.recv() => {}
            _ = sigterm.recv() => {}
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

fn sighup() -> impl std::future::Future<Output = ()> {
    #[cfg(unix)]
    let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
        .expect("listen for SIGHUP");
    async move {
        #[cfg(unix)]
        {
            hangup.recv().await;
        }
        #[cfg(not(unix))]
        {
            std::future::pending::<()>().await;
        }
    }
}
