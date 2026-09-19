//! The installer only prepares local prerequisites. Station owns mesh joins.
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

struct LocalStation {
    root: PathBuf,
    http: reqwest::Client,
}
impl LocalStation {
    fn new(root: PathBuf) -> Result<Self> {
        Ok(Self {
            root,
            http: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(55))
                .build()?,
        })
    }
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let config = zork_config::load_config(&self.root)?;
        let token_path = self.root.join("run/node-token.json");
        let token = if token_path.exists() {
            serde_json::from_slice::<String>(&std::fs::read(token_path)?)?
        } else {
            config.admin.token
        };
        let mut request = self
            .http
            .request(
                method,
                format!(
                    "{}{}",
                    zork_config::loopback_base_url(&config.bind.runtime),
                    path
                ),
            )
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            anyhow::bail!("The running Station does not support mesh enrollment; update it before retrying. Its tasks have been left running.");
        }
        let value: Value = response.json().await?;
        ensure!(
            status.is_success(),
            "{}",
            value["error"].as_str().unwrap_or("Station request failed")
        );
        Ok(value)
    }
    fn core_client(&self) -> Result<zork_client_core::api::StationClient> {
        let config = zork_config::load_config(&self.root)?;
        let token_path = self.root.join("run/node-token.json");
        let token = if token_path.exists() {
            serde_json::from_slice::<String>(&std::fs::read(token_path)?)?
        } else {
            config.admin.token
        };
        Ok(zork_client_core::api::StationClient::new(
            zork_config::loopback_base_url(&config.bind.runtime),
            Some(token),
        ))
    }
    async fn ready(&self) -> bool {
        let Ok(config) = zork_config::load_config(&self.root) else {
            return false;
        };
        self.http
            .get(format!(
                "{}/readyz",
                zork_config::loopback_base_url(&config.bind.runtime)
            ))
            .timeout(Duration::from_millis(500))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
    }
    async fn verify(&self) -> Result<()> {
        let status = self
            .request(reqwest::Method::GET, "/v1/node/status", None)
            .await?;
        ensure!(
            status["protocol"] == 1 && status["mesh_join"] == 1,
            "Station version does not support this invitation"
        );
        let root = status["data_root"]
            .as_str()
            .context("Station omitted its data directory")?;
        ensure!(Path::new(root).canonicalize()?==self.root.canonicalize()?,"Station listener belongs to a different data directory; specify the correct --data directory");
        Ok(())
    }
}

async fn choose_root(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(root) = explicit {
        return Ok(root);
    }
    let mut installed = vec![];
    let mut running = vec![];
    for root in zork_config::service::known_roots()
        .into_iter()
        .filter(|r| zork_config::config_path(r).exists())
    {
        let root = root.canonicalize()?;
        if zork_config::channel::recorded(&root)?.unwrap_or_default()
            != zork_config::channel::current()?
        {
            continue;
        }
        if installed.contains(&root) {
            continue;
        }
        if LocalStation::new(root.clone())?.ready().await {
            running.push(root.clone());
        }
        installed.push(root);
    }
    let candidates = if running.is_empty() {
        installed
    } else {
        running
    };
    ensure!(
        candidates.len() <= 1,
        "Multiple Station installations found. Repeat with --data and one of: {}",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(candidates
        .into_iter()
        .next()
        .unwrap_or_else(zork_config::default_data_root))
}

fn install_binaries(root: &Path) -> Result<PathBuf> {
    let current = std::env::current_exe()?;
    let mut sources = vec![("zork", current)];
    for name in ["zork-station", "zork-agent", "zork-gh"] {
        sources.push((
            name,
            zork_config::find_bin(name, root).with_context(|| {
                format!("Installer is missing {name}; use the complete Zork node package")
            })?,
        ));
    }
    for (name, source) in sources {
        let target = root.join("bin").join(name);
        if source.canonicalize().ok() == target.canonicalize().ok() {
            continue;
        }
        // Do not overwrite an existing installation as a side effect of joining.
        if target.exists() {
            continue;
        }
        let temporary = target.with_extension("installing");
        std::fs::copy(source, &temporary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o755))?;
        }
        std::fs::rename(temporary, target)?;
    }
    Ok(root.join("bin/zork"))
}

pub async fn run(mut argv: Vec<String>) -> Result<()> {
    ensure!(
        !argv.is_empty(),
        "mesh requires invite, join, switch, leave or status"
    );
    let action = argv.remove(0);
    let mut explicit = None;
    let mut name = None;
    let mut ticket = None;
    let mut index = 0;
    let mut json_output = false;
    let mut confirmed = false;
    while index < argv.len() {
        let argument = &argv[index];
        match argument.as_str() {
            "--json" => json_output = true,
            "--yes" => confirmed = true,
            "--channel" => {
                index += 1;
                let value = argv.get(index).context("missing channel")?;
                zork_config::channel::set_host_channel(zork_config::channel::Channel::parse(
                    value,
                )?)?;
            }
            "--data" | "--name" => {
                index += 1;
                let value = argv.get(index).context("missing option value")?.clone();
                match argument.as_str() {
                    "--data" => explicit = Some(PathBuf::from(value)),
                    "--name" => name = Some(value),
                    _ => unreachable!(),
                }
            }
            value
                if !value.starts_with('-')
                    && matches!(action.as_str(), "join" | "switch")
                    && ticket.is_none() =>
            {
                ticket = Some(value.to_owned())
            }
            _ => anyhow::bail!("unknown mesh option {argument}"),
        }
        index += 1;
    }
    ensure!(
        matches!(
            action.as_str(),
            "install" | "invite" | "join" | "switch" | "leave" | "status"
        ),
        "unknown mesh action"
    );
    let root = choose_root(explicit).await?;
    let channel = zork_config::channel::activate_for_data(&root)?;
    if matches!(action.as_str(), "join" | "switch") {
        ensure!(ticket.is_some(), "join requires an invitation");
    }
    if action == "status" {
        ensure!(
            zork_config::config_path(&root).exists(),
            "No Station is installed on this device"
        );
        let station = LocalStation::new(root.clone())?;
        station.verify().await?;
        let value = station
            .request(reqwest::Method::GET, "/v1/node/mesh", None)
            .await?;
        if json_output {
            println!("{value}");
        } else {
            println!("Station: {}", root.display());
            let listed = |values: &Value| {
                let items = values
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|item| item.as_str())
                    .collect::<Vec<_>>();
                if items.is_empty() {
                    "none".to_string()
                } else {
                    items.join(", ")
                }
            };
            println!("  direct: {}", listed(&value["address"]["direct"]));
            println!("  relays: {}", listed(&value["address"]["relays"]));
            for peer in value["config"]["peers"].as_array().into_iter().flatten() {
                println!("  {}", peer["name"].as_str().unwrap_or("Device"));
            }
            if let Some(join) = value.get("join").filter(|join| !join.is_null()) {
                println!(
                    "接入：{}（尝试 {} 次）",
                    join["label"].as_str().unwrap_or("状态未知"),
                    join["attempt"]
                );
                if let Some(seconds) = join["retry_in_seconds"].as_u64() {
                    println!("  {seconds} 秒后重试");
                }
            }
        }
        return Ok(());
    }
    if action == "leave" {
        let station = LocalStation::new(root.clone())?;
        station.verify().await?;
        let config = zork_config::load_config(&root)?;
        let group = config.mesh.group.context("当前设备尚未加入 Mesh")?;
        ensure!(
            confirmed,
            "将断开本机与当前 Mesh 的连接，保留本机数据和其它成员。确认后请加 --yes 重试。"
        );
        let client = station.core_client()?;
        let value = zork_client_core::mesh_enrollment::leave(
            &client,
            &zork_config::membership::MeshVersion::of(&group),
        )
        .await?;
        if json_output {
            println!("{value}");
        } else {
            println!("已离开 Mesh，本机身份、文件和历史保留。");
        }
        return Ok(());
    }
    let installed = zork_config::config_path(&root).exists();
    zork_config::channel::claim(&root, channel)?;
    zork_config::ensure_layout(&root)?;
    let root = root.canonicalize()?;
    let _setup = zork_config::service::exclusive_lock(&root.join("run/mesh-setup.lock"))
        .context("Another Station setup is already running for this device")?;
    let station = LocalStation::new(root.clone())?;
    let running = station.ready().await;
    if running {
        station.verify().await?;
        eprintln!("Using the running Station at {}", root.display());
    } else {
        // A supervisor may still be starting. Do not create a competing service.
        if zork_config::service::running(&root) {
            eprintln!("Waiting for the existing Station to become ready…");
        } else {
            let binary = install_binaries(&root)?;
            zork_config::update_config(&root, |config| {
                if !installed {
                    config.admin.token = zork_mesh::enrollment::secret();
                    // Reserve distinct, available loopback ports. An unrelated
                    // local application must not make first-time setup fail.
                    let mut listeners = vec![];
                    for binding in [
                        &mut config.bind.station,
                        &mut config.bind.runtime,
                        &mut config.bind.control,
                        &mut config.bind.agent,
                    ] {
                        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
                        *binding = listener.local_addr()?.to_string();
                        listeners.push(listener);
                    }
                }
                config.mesh.enabled = true;
                if config.mesh.name.is_empty() {
                    config.mesh.name = name.clone().unwrap_or_else(zork_config::device_name);
                }
                Ok(())
            })?;
            let config = zork_config::load_config(&root)?;
            zork_mesh::managed::validate(&config.mesh)?;
            zork_config::service::install(&root, &binary, true)?;
            eprintln!(
                "Station installed as a background service at {}",
                root.display()
            );
        }
        let mut events = zork_config::service::Events::new(&root)?;
        let mut changes = events.subscribe();
        let mut retry = zork_notify::retry::Retry::default();
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                changes.checkpoint();
                events.refresh()?;
                let announced = zork_config::read_ready_pid(&root, "zork-station")?.is_some();
                if announced && station.ready().await {
                    return Ok::<_, anyhow::Error>(());
                }
                tokio::select! {
                    result = changes.changed() => { result?; },
                    _ = retry.wait(), if announced => {},
                }
            }
        })
        .await
        .context("Station did not become ready")??;
        station.verify().await?;
    }
    if action == "install" {
        if json_output {
            println!("{}", json!({"installed":true,"data_root":root}));
        } else {
            println!("Station ready at {}", root.display());
            println!(
                "Manage it with: '{}' service status --data '{}'",
                root.join("bin/zork").display(),
                root.display()
            );
        }
        return Ok(());
    }
    let value = station
        .request(reqwest::Method::GET, "/v1/node/mesh", None)
        .await?;
    let mut config: zork_config::MeshConfig = serde_json::from_value(value["config"].clone())?;
    let previous = config.clone();
    config.enabled = true;
    let switch_from = if action == "switch" {
        let group = previous
            .group
            .as_ref()
            .context("当前没有 Mesh，请使用 mesh join")?;
        ensure!(
            confirmed,
            "将从当前 Mesh 切换到邀请中的 Mesh，保留本机数据和其它成员。确认后请加 --yes 重试。"
        );
        let expected = zork_config::membership::MeshVersion::of(group);
        Some(expected)
    } else {
        None
    };
    // Enabling Mesh is a local prerequisite. Invitation resolution and membership
    // live in the Station operation; joining never rewrites operator network choices.
    let mut replaced_pid = None;
    if config != previous {
        let status = station
            .request(reqwest::Method::GET, "/v1/node/status", None)
            .await?;
        let saved = station
            .request(reqwest::Method::PUT, "/v1/node/mesh", Some(json!(config)))
            .await?;
        if saved["restarting"] == true {
            replaced_pid = Some(status["pid"].clone());
        }
    }
    let mut events = zork_config::service::Events::new(&root)?;
    let mut changes = events.subscribe();
    let mut retry = zork_notify::retry::Retry::default();
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            changes.checkpoint();
            events.refresh()?;
            let ready = async {
                let status = station
                    .request(reqwest::Method::GET, "/v1/node/status", None)
                    .await?;
                if replaced_pid
                    .as_ref()
                    .is_some_and(|pid| *pid == status["pid"])
                {
                    return Ok::<_, anyhow::Error>(false);
                }
                let value = station
                    .request(reqwest::Method::GET, "/v1/node/mesh", None)
                    .await?;
                Ok(value["origin"].is_string())
            };
            let failed = match tokio::time::timeout(Duration::from_secs(2), ready).await {
                Ok(Ok(true)) => return Ok::<_, anyhow::Error>(()),
                Ok(Ok(false)) => {
                    retry.reset();
                    false
                }
                _ => true,
            };
            tokio::select! {
                result = changes.changed() => { result?; },
                _ = retry.wait(), if failed => {},
            }
        }
    })
    .await
    .context("Station Mesh did not become ready; check node logs and retry the join command")??;
    match action.as_str() {
        "invite" => {
            let value = station
                .request(reqwest::Method::POST, "/v1/node/mesh/invites", None)
                .await?;
            if json_output {
                println!("{value}");
            } else {
                println!(
                    "Run this command on the device to add (valid for 15 minutes):\n\n{}",
                    value["command"].as_str().context("missing join command")?
                );
            }
        }
        "join" | "switch" => {
            eprintln!("Joining your mesh: devices may assign tasks to each other, access task files and manage nodes.");
            let client = station.core_client()?;
            let mut shown = String::new();
            let value = zork_client_core::mesh_enrollment::join(
                &client,
                &zork_client_core::mesh_enrollment::JoinRequest {
                    invitation: ticket.context("missing invitation")?,
                    name: name.clone(),
                    switch_from,
                },
                |progress| {
                    let message = format!(
                        "{}{}",
                        progress.label,
                        progress
                            .retry_in_seconds
                            .map(|seconds| format!(
                                "（{seconds} 秒后，第 {} 次）",
                                progress.attempt + 1
                            ))
                            .unwrap_or_default()
                    );
                    if message != shown {
                        eprintln!("{message}");
                        shown = message;
                    }
                },
            )
            .await?;
            if json_output {
                println!("{value}");
            } else {
                let device = value["group"]["members"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|m| m["origin"] == value["origin"])
                    .and_then(|m| m["name"].as_str())
                    .unwrap_or("This device");
                println!(
                    "{device} {} your mesh.",
                    if value["already_joined"] == true {
                        "is already in"
                    } else {
                        "has joined"
                    }
                );
                println!("Open this device in Zork to configure model connections and Agents.");
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}
