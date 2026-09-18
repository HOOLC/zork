use anyhow::{ensure, Context, Result};
use std::{process::Stdio, time::Duration};
use zork_config::service as manager;

pub async fn command(mut argv: Vec<String>) -> Result<()> {
    ensure!(
        !argv.is_empty(),
        "service requires install, uninstall or status"
    );
    let action = argv.remove(0);
    let at_login = argv.iter().any(|a| a == "--at-login");
    argv.retain(|a| a != "--at-login");
    let args = zork_config::parse_process_args_from(argv)?;
    zork_config::ensure_layout(&args.data_root)?;
    match action.as_str() {
        "install" => {
            manager::install(&args.data_root, &std::env::current_exe()?, at_login)?;
            println!("Station background service enabled");
        }
        "uninstall" => {
            manager::uninstall(&args.data_root)?;
            println!("Background service removed; use zork stop to stop the running Station");
        }
        "status" => println!(
            "{}",
            serde_json::json!({"settings":manager::settings(&args.data_root)?,"running":manager::running(&args.data_root)})
        ),
        _ => anyhow::bail!("unknown service action"),
    }
    Ok(())
}

pub async fn stop(argv: Vec<String>) -> Result<()> {
    let args = zork_config::parse_process_args_from(argv)?;
    if manager::settings(&args.data_root)?.enabled {
        manager::uninstall(&args.data_root)?;
    }
    let mut events = manager::Events::new(&args.data_root)?;
    let mut changes = events.subscribe();
    manager::connect(&args.data_root, "stop")?;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            changes.checkpoint();
            events.refresh()?;
            if events.supervisor_exited()? && !manager::running(&args.data_root) {
                println!("Station stopped");
                return Ok::<_, anyhow::Error>(());
            }
            changes.changed().await?;
        }
    })
    .await
    .context("Station has not confirmed shutdown")?
}

/// A service manager watches an existing supervisor in place. It does not
/// restart work to adopt it, nor terminate it when registration changes.
pub async fn run(argv: Vec<String>) -> Result<()> {
    let args = zork_config::parse_process_args_from(argv.clone())?;
    let root = args.data_root;
    let watcher = root.join("run/service-watch.lock");
    let _watcher = zork_config::service::exclusive_lock(&watcher)?;
    let binary = std::env::current_exe()?;
    let mut child: Option<tokio::process::Child> = None;
    let mut events = manager::Events::new(&root)?;
    let mut changes = events.subscribe();
    let mut retry = zork_notify::retry::Retry::default();
    loop {
        changes.checkpoint();
        events.refresh()?;
        if !manager::settings(&root)?.enabled {
            return Ok(());
        }
        let running = manager::running(&root);
        if running {
            retry.reset();
        }
        if !running
            && events.supervisor_exited()?
            && child
                .as_mut()
                .is_none_or(|p| p.try_wait().ok().flatten().is_some())
        {
            let log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(root.join("logs/supervisor.log"))?;
            let mut command = tokio::process::Command::new(&binary);
            command
                .arg("start")
                .args(&argv)
                .env_remove("ZORK_PARENT_PIPE")
                .stdin(Stdio::null())
                .stdout(Stdio::from(log.try_clone()?))
                .stderr(Stdio::from(log));
            #[cfg(unix)]
            {
                command.process_group(0);
            }
            manager::prepare_child(command.as_std_mut());
            child = Some(command.spawn().context("start background Station")?);
        }
        tokio::select! {
            changed = changes.changed() => { changed?; },
            exit = async { child.as_mut().unwrap().wait().await }, if child.is_some() => {
                exit?;
                child = None;
                // A failed child start is retried; a healthy service has no clock.
                tokio::select! { _ = retry.wait() => {}, changed = changes.changed() => { changed?; } }
            }
        }
    }
}
