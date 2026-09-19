//! CLI presentation for the shared relay account controller.
use anyhow::{Context, Result};
use std::{path::PathBuf, process::Command};
use zork_client_core::relay_account::Account;

pub async fn run(mut argv: Vec<String>) -> Result<()> {
    let command = if argv.first().is_some_and(|a| !a.starts_with('-')) {
        argv.remove(0)
    } else {
        "status".into()
    };
    let mut data = zork_config::default_data_root();
    let mut all = false;
    let mut json = false;
    let mut no_browser = false;
    let mut device = false;
    let mut session = None;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--data" if i + 1 < argv.len() => {
                data = PathBuf::from(&argv[i + 1]);
                i += 2;
            }
            "--all" if command == "logout" => {
                all = true;
                i += 1;
            }
            "--json" => {
                json = true;
                i += 1;
            }
            "--device" if command == "login" => {
                device = true;
                i += 1;
            }
            "--no-browser" if command == "login" => {
                no_browser = true;
                i += 1;
            }
            value if command == "revoke" && session.is_none() && !value.starts_with('-') => {
                session = Some(value.to_owned());
                i += 1;
            }
            _ => anyhow::bail!("unknown account argument"),
        }
    }
    let channel = zork_config::channel::activate_for_data(&data)?;
    zork_config::channel::claim(&data, channel)?;
    let account = Account::configured(data)?;
    match command.as_str() {
        "login" if device => {
            let login = account.begin_device_login(&zork_config::device_name()).await?;
            println!("Open this URL on any device to log in to Zork:\n{}", login.url());
            if !no_browser { let _ = open_browser(login.url()); }
            let status = tokio::select! {
                result = login.finish() => result?,
                _ = tokio::signal::ctrl_c() => { login.cancel().await?; anyhow::bail!("Google login cancelled"); }
            };
            if json { println!("{}", serde_json::to_string(&status)?); }
            else { println!("Logged in as {}. Relay access updates automatically.", status.email.or(status.subject).unwrap_or_default()); }
            Ok(())
        }
        "login" => {
            let login = account.begin_login(&zork_config::device_name()).await?;
            if no_browser {
                // This is an authorization URL with PKCE challenge, never an access
                // or refresh token. Native integration captures it without logging it.
                println!("{}", login.url());
            } else if let Err(error) = open_browser(login.url()) {
                login.cancel().await?;
                return Err(error);
            } else {
                println!("Complete Google login in the browser.");
            }
            let status = tokio::select! {
                result = login.finish() => result?,
                _ = tokio::signal::ctrl_c() => {
                    login.cancel().await?;
                    anyhow::bail!("Google login cancelled");
                }
            };
            if json {
                println!("{}", serde_json::to_string(&status)?);
            } else {
                println!(
                    "Logged in as {}. Relay access updates automatically.",
                    status.email.or(status.subject).unwrap_or_default()
                );
            }
            Ok(())
        }
        "logout" => {
            let pending = account.logout(all).await?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({"logged_out_locally":true,"revocations_pending":pending})
                );
            }
            if pending > 0 {
                anyhow::bail!(
                    "Logged out locally. Server revocation is pending; reconnect and run zork account logout again."
                );
            }
            if !json {
                println!("Logged out. No server revocations remain pending.");
            }
            Ok(())
        }
        "status" => {
            let status = account.status(true).await?;
            if json {
                println!("{}", serde_json::to_string(&status)?);
            } else if status.authenticated {
                println!(
                    "Logged in as {} ({})",
                    status.email.or(status.subject).unwrap_or_default(),
                    if status.remote_verified {
                        "server verified"
                    } else {
                        "cached; server unavailable"
                    }
                );
            } else {
                println!("Not logged in. Run: zork account login");
            }
            Ok(())
        }
        "refresh" => {
            let access = account
                .access(true)
                .await?
                .context("relay login required")?;
            if json {
                println!("{}", serde_json::json!({"expires_at":access.expires_at()}));
            } else {
                println!("Relay session renewed.");
            }
            Ok(())
        }
        "sessions" => {
            let sessions = account.sessions().await?;
            if json {
                println!("{}", serde_json::to_string(&sessions)?);
            } else {
                for session in sessions {
                    println!(
                        "{}  {}{}",
                        session.id,
                        session.name,
                        if session.current { " (current)" } else { "" }
                    );
                }
            }
            Ok(())
        }
        "revoke" => {
            account
                .revoke(
                    session
                        .as_deref()
                        .context("Usage: zork account revoke SESSION_ID [--data DIR]")?,
                )
                .await?;
            println!("Session revoked and its relay connections closed.");
            Ok(())
        }
        _ => anyhow::bail!(
            "Usage: zork account login [--device] [--no-browser]|status|refresh|sessions|revoke SESSION_ID|logout [--all] [--data DIR] [--json]"
        ),
    }
}
fn open_browser(url: &str) -> Result<()> {
    let mut cmd = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(target_os = "windows") {
        let mut cmd = Command::new("rundll32");
        cmd.arg("url.dll,FileProtocolHandler");
        cmd
    } else {
        Command::new("xdg-open")
    };
    if cmd
        .arg(url)
        .status()
        .context("failed to open the browser")?
        .success()
    {
        Ok(())
    } else {
        anyhow::bail!("failed to open the browser")
    }
}
