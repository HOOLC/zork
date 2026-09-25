//! Real Station, Mesh transport and an isolated SDK ADB server. The adbd peer is
//! a protocol fixture; this does not claim Android authorization or APK install QA.
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    process::{Child, Command},
    sync::watch,
};
use zork_client_core::{
    adb::{Action, Controller},
    store::{ClientStore, RemoteNode, SavedNode},
};
use zork_client_types::adb::Facts;

const TOKEN: &str = "isolated-adb-fixture";
fn config() -> zork_config::MeshConfig {
    zork_config::MeshConfig {
        enabled: true,
        offline: true,
        bind: Some("127.0.0.1:0".into()),
        ..Default::default()
    }
}
fn process(binary: &Path, args: &[&str], log: &Path, socket: &str) -> Result<Child> {
    let log = std::fs::File::create(log)?;
    Ok(Command::new(binary)
        .args(args)
        .env("ADB_SERVER_SOCKET", socket)
        .env("ADB_MDNS_AUTO_CONNECT", "0")
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .kill_on_drop(true)
        .spawn()?)
}
async fn request(
    base: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()?;
    let mut request = client
        .request(method, format!("{base}{path}"))
        .bearer_auth(TOKEN);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    let status = response.status();
    let value = response.json::<Value>().await?;
    ensure!(status.is_success(), "{path} {status}: {value}");
    Ok(value)
}
async fn wait_state(controller: &Controller, check: impl Fn(&Value) -> bool) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let snapshot = controller.snapshot();
        if check(&snapshot) {
            return Ok(snapshot);
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "ADB state timeout: {snapshot}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
async fn packet(socket: &mut TcpStream) -> Result<([u8; 4], u32, Vec<u8>)> {
    let mut header = [0; 24];
    socket.read_exact(&mut header).await?;
    let word = |i| u32::from_le_bytes(header[i..i + 4].try_into().unwrap());
    ensure!(
        word(20) == word(0) ^ u32::MAX && word(12) <= 1024 * 1024,
        "invalid ADB packet"
    );
    let mut bytes = vec![0; word(12) as usize];
    socket.read_exact(&mut bytes).await?;
    Ok((header[..4].try_into().unwrap(), word(4), bytes))
}
async fn send(
    socket: &mut TcpStream,
    kind: &[u8; 4],
    arg0: u32,
    arg1: u32,
    bytes: &[u8],
) -> Result<()> {
    let command = u32::from_le_bytes(*kind);
    for word in [
        command,
        arg0,
        arg1,
        bytes.len() as u32,
        bytes.iter().map(|b| *b as u32).sum(),
        command ^ u32::MAX,
    ] {
        socket.write_u32_le(word).await?;
    }
    socket.write_all(bytes).await?;
    Ok(())
}
fn payload() -> Vec<u8> {
    (0..1024 * 1024).map(|n| (n % 251) as u8).collect()
}
async fn adbd(mut socket: TcpStream, authorized: bool) -> Result<()> {
    let (kind, _, _) = packet(&mut socket).await?;
    ensure!(&kind == b"CNXN", "expected CNXN");
    if !authorized {
        send(&mut socket, b"AUTH", 1, 0, &[7; 20]).await?;
        loop {
            let (_, kind, _) = packet(&mut socket).await?;
            if kind == 2 {
                send(&mut socket, b"AUTH", 1, 0, &[7; 20]).await?;
            }
        }
    }
    send(
        &mut socket,
        b"CNXN",
        0x01000000,
        4096,
        b"device::ro.product.model=MeshFixture;\0",
    )
    .await?;
    loop {
        let (kind, remote, bytes) = packet(&mut socket).await?;
        if &kind != b"OPEN" {
            continue;
        }
        ensure!(
            bytes.starts_with(b"exec:zork-fixture"),
            "unexpected fixture command: {:?}",
            bytes
        );
        send(&mut socket, b"OKAY", 1, remote, &[]).await?;
        for chunk in payload().chunks(4096) {
            send(&mut socket, b"WRTE", 1, remote, chunk).await?;
            ensure!(
                &packet(&mut socket).await?.0 == b"OKAY",
                "missing write acknowledgement"
            );
        }
        send(&mut socket, b"CLSE", 1, remote, &[]).await?;
    }
}
struct OwnedController(Arc<Controller>);
impl Drop for OwnedController {
    fn drop(&mut self) {
        self.0.pause();
    }
}

struct HostIdentity {
    root: PathBuf,
    origin: String,
    address: String,
    reservation: Option<std::net::UdpSocket>,
}
impl HostIdentity {
    async fn new(root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&root)?;
        let mut bootstrap = zork_mesh::managed::start_client(&root, &config()).await?;
        let origin = bootstrap.node().identity().await?;
        bootstrap.shutdown().await?;
        let reservation = std::net::UdpSocket::bind("127.0.0.1:0")?;
        let address = reservation.local_addr()?.to_string();
        Ok(Self {
            root,
            origin,
            address,
            reservation: Some(reservation),
        })
    }
    fn save(&self, store: &ClientStore, name: &str) -> Result<()> {
        store.save_node(&SavedNode { machine_name: None, color_key: None,
            id: self.origin.clone(),
            name: name.into(),
            url: String::new(),
            token: None,
            local: false,
            mesh: Some(RemoteNode {
                routes: None,
                origin: self.origin.clone(),
                addr: Some(self.address.clone()),
            }),
            group: None,
        })
    }
}
struct Host {
    identity: HostIdentity,
    base: String,
    session: String,
    adb_port: String,
    adb_socket: String,
    adb: Child,
    process: Option<Child>,
    reservations: Vec<std::net::TcpListener>,
}
impl Host {
    fn new(
        identity: HostIdentity,
        phone: &str,
        phone_address: Option<&str>,
        adb: &Path,
    ) -> Result<Self> {
        let mut bind = serde_json::Map::new();
        let mut reservations = vec![];
        for name in ["station", "runtime", "control", "agent"] {
            let socket = std::net::TcpListener::bind("127.0.0.1:0")?;
            bind.insert(name.into(), json!(socket.local_addr()?.to_string()));
            reservations.push(socket);
        }
        let base = format!("http://{}", bind["runtime"].as_str().unwrap());
        let settings = json!({"bind":bind,"admin":{"token":TOKEN},"mesh":{"enabled":true,"offline":true,"bind":identity.address,
            "peers":[{"origin":phone,"name":"Phone fixture","addr":phone_address,"client":true,"execute":[]}]}});
        std::fs::write(
            identity.root.join("config.json"),
            serde_json::to_vec(&settings)?,
        )?;
        std::fs::create_dir_all(identity.root.join("profiles"))?;
        std::fs::write(
            identity.root.join("profiles/fixture.json"),
            serde_json::to_vec(&json!({
            "provider":"openai","billing":"usage","base_url":"http://127.0.0.1:9/v1","auth":{"type":"api_key","key":"sk-fixture"},
            "models":[{"id":"fixture-model","api":"openai-completions","streaming":false,"thinking":["off"],"default_thinking":"off",
                "capabilities":{"input":["text"]},"limits":{"context_window_tokens":100000,"max_output_tokens":10000},"default":true}]}))?,
        )?;
        let reservation = std::net::TcpListener::bind("127.0.0.1:0")?;
        let adb_port = reservation.local_addr()?.port().to_string();
        let adb_socket = format!("tcp:{adb_port}");
        drop(reservation);
        let adb = process(
            adb,
            &[
                "--one-device",
                "zork-adb-fixture-no-usb",
                "-L",
                &adb_socket,
                "server",
                "nodaemon",
            ],
            &identity.root.join("adb.log"),
            &adb_socket,
        )?;
        Ok(Self {
            identity,
            base,
            session: String::new(),
            adb_port,
            adb_socket,
            adb,
            process: None,
            reservations,
        })
    }
    async fn start(&mut self, binary: &Path) -> Result<()> {
        self.reservations.clear();
        self.identity.reservation.take();
        self.process = Some(process(
            binary,
            &[
                "--data",
                self.identity.root.to_str().unwrap(),
                "--fake-agent",
            ],
            &self.identity.root.join("station.log"),
            &self.adb_socket,
        )?);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while request(&self.base, reqwest::Method::GET, "/readyz", None)
            .await
            .is_err()
        {
            ensure!(
                tokio::time::Instant::now() < deadline,
                "Station did not start"
            );
            ensure!(
                self.process.as_mut().unwrap().try_wait()?.is_none(),
                "Station exited"
            );
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        ensure!(
            std::fs::read_to_string(self.identity.root.join("skills/bundled/android-debugging/SKILL.md"))?
                .contains("adb devices -l"),
            "bundled Android Skill missing"
        );
        if self.session.is_empty() {
            let workspace = self.identity.root.join("workspace");
            std::fs::create_dir_all(&workspace)?;
            self.session = request(&self.base,reqwest::Method::POST,"/v1/im/sessions",
                Some(json!({"profile_id":"fixture","model":"fixture-model","thinking":"off","workspace":workspace})))
                .await?["session_id"].as_str().context("missing Session")?.to_owned();
        }
        Ok(())
    }
    async fn stop(&mut self) {
        if let Some(mut process) = self.process.take() {
            let _ = process.kill().await;
            let _ = process.wait().await;
        }
    }
    async fn devices(&self, binary: &Path) -> Result<String> {
        let output = Command::new(binary)
            .args(["-P", &self.adb_port, "devices", "-l"])
            .kill_on_drop(true)
            .output()
            .await?;
        ensure!(output.status.success(), "adb devices failed");
        Ok(String::from_utf8(output.stdout)?)
    }
    async fn transfer(&self, binary: &Path, serial: &str) -> Result<()> {
        let transfer = tokio::time::timeout(
            Duration::from_secs(30),
            Command::new(binary)
                .args([
                    "-P",
                    &self.adb_port,
                    "-s",
                    serial,
                    "exec-out",
                    "zork-fixture",
                ])
                .kill_on_drop(true)
                .output(),
        )
        .await??;
        ensure!(
            transfer.status.success(),
            "ADB exec-out failed: {}",
            String::from_utf8_lossy(&transfer.stderr)
        );
        ensure!(
            transfer.stdout == payload(),
            "binary data changed across Mesh"
        );
        Ok(())
    }
}
fn station_snapshot(snapshot: &Value, origin: &str) -> Value {
    snapshot["stations"]
        .as_array()
        .and_then(|stations| stations.iter().find(|station| station["origin"] == origin))
        .cloned()
        .unwrap_or(Value::Null)
}
fn serial(snapshot: &Value, origin: &str) -> Result<String> {
    station_snapshot(snapshot, origin)["serial"]
        .as_str()
        .map(str::to_owned)
        .context("missing Station serial")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires freshly built ZORK_TEST_STATION_BIN and SDK ZORK_TEST_ADB_BIN"]
async fn adb_mesh_multiple_stations_activation_transfer_reconnect_disable_and_revoke() -> Result<()>
{
    let binary = PathBuf::from(
        std::env::var_os("ZORK_TEST_STATION_BIN")
            .context("rebuild Station and set ZORK_TEST_STATION_BIN")?,
    );
    let adb_binary = PathBuf::from(
        std::env::var_os("ZORK_TEST_ADB_BIN").context("set ZORK_TEST_ADB_BIN to SDK adb")?,
    );
    let temporary = tempfile::Builder::new()
        .prefix("zork-adb-mesh-")
        .tempdir()?;
    let root = temporary.path();
    let identities = [
        HostIdentity::new(root.join("host-a")).await?,
        HostIdentity::new(root.join("host-b")).await?,
    ];
    let phone = root.join("phone");
    let store = Arc::new(ClientStore::open(&phone)?);
    identities[0].save(&store, "First Station")?;
    let mut phone_config = config();
    for identity in &identities {
        phone_config.peers.push(zork_config::MeshPeer {
            routes: None,
            origin: identity.origin.clone(),
            name: "Station fixture".into(),
            addr: Some(identity.address.clone()),
            execute: vec![],
            client: false,
            collaborate: false,
        });
    }
    let (mut runtime, phone_origin) =
        zork_client_core::transport::start(&phone, &phone_config).await?;
    let phone_address = runtime
        .node()
        .address()?
        .ip_addrs()
        .find(|a| a.is_ipv4())
        .map(|a| a.to_string());
    let mut hosts = identities
        .into_iter()
        .map(|identity| {
            Host::new(
                identity,
                &phone_origin,
                phone_address.as_deref(),
                &adb_binary,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let origins = [
        hosts[0].identity.origin.clone(),
        hosts[1].identity.origin.clone(),
    ];
    let controller = OwnedController(Controller::new(store.clone())?);
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (activated, mut activation) = watch::channel(0_u8);
    let fake_adbd = zork_notify::Task(tokio::spawn(async move {
        let mut clients = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                result = listener.accept() => {
                    let Ok((socket,_)) = result else { return };
                    let mode = *activation.borrow();
                    if mode!=0 { clients.spawn(async move { let _ = adbd(socket,mode==2).await; }); }
                },
                _ = activation.changed() => { clients.abort_all(); },
                Some(_) = clients.join_next(), if !clients.is_empty() => {},
            }
        }
    }));
    let result = async {
        hosts[0].start(&binary).await?;
        controller.0.execute(Action::Facts {
            facts: Facts {
                supported: true,
                application_id: "fixture.phone".into(),
                name: "MeshFixture".into(),
                developer_enabled: Some(true),
                usb_enabled: Some(true),
                wireless_enabled: Some(false),
            },
        })?;
        controller.0.execute(Action::SetPort {
            port: port.to_string(),
        })?;
        controller.0.execute(Action::SetEnabled { enabled: true })?;
        controller.0.start(runtime.node());
        wait_state(&controller.0, |s| s["page"]["step"] == "activation").await?;
        assert!(!hosts[0].devices(&adb_binary).await?.contains("\tdevice"));
        eprintln!("PASS inactive adbd reports activation to the phone and standard adb has no connected device");
        activated.send_replace(1);
        let authorization = wait_state(&controller.0, |s| {
            station_snapshot(s, &origins[0])["state"] == "unauthorized"
        })
        .await?;
        assert_eq!(authorization["page"]["step"], "ready");
        ensure!(
            station_snapshot(&authorization, &origins[0])["help"]["paragraphs"][0]
                .as_str()
                .unwrap()
                .contains("系统弹窗"),
            "Station authorization action missing"
        );
        activated.send_replace(2);
        let ready = wait_state(&controller.0, |s| {
            station_snapshot(s, &origins[0])["state"] == "ready"
        })
        .await?;
        let first_serial = serial(&ready, &origins[0])?;
        controller.0.execute(Action::Refresh)?;
        hosts[1].identity.save(&store, "Second Station")?;
        controller.0.execute(Action::Refresh)?;
        wait_state(&controller.0, |s| {
            s["stations"].as_array().unwrap().len() == 2
        })
        .await?;
        assert_eq!(serial(&controller.0.snapshot(), &origins[0])?, first_serial);
        hosts[1].start(&binary).await?;
        let ready = wait_state(&controller.0, |s| {
            origins
                .iter()
                .all(|id| station_snapshot(s, id)["state"] == "ready")
        })
        .await?;
        let second_serial = serial(&ready, &origins[1])?;
        assert_eq!(
            serial(&ready, &origins[0])?,
            first_serial,
            "joining Station replaced the first connection"
        );
        let first_devices = hosts[0].devices(&adb_binary).await?;
        let second_devices = hosts[1].devices(&adb_binary).await?;
        assert!(first_devices.lines().any(|line| line.starts_with(&first_serial) && line.split_whitespace().nth(1) == Some("device")), "{first_devices}");
        assert!(second_devices.lines().any(|line| line.starts_with(&second_serial) && line.split_whitespace().nth(1) == Some("device")), "{second_devices}");
        tokio::try_join!(
            hosts[0].transfer(&adb_binary, &first_serial),
            hosts[1].transfer(&adb_binary, &second_serial)
        )?;
        eprintln!("PASS two Stations and two isolated SDK ADB servers transfer 1 MiB concurrently");
        hosts[1].stop().await;
        wait_state(&controller.0, |s| {
            station_snapshot(s, &origins[1])["state"] != "ready"
        })
        .await?;
        assert_eq!(serial(&controller.0.snapshot(), &origins[0])?, first_serial);
        hosts[0].transfer(&adb_binary, &first_serial).await?;
        hosts[1].start(&binary).await?;
        wait_state(&controller.0, |s| {
            station_snapshot(s, &origins[1])["state"] == "ready"
        })
        .await?;
        assert_eq!(serial(&controller.0.snapshot(), &origins[0])?, first_serial);
        eprintln!("PASS one Station reconnects without interrupting the other Station");
        activated.send_replace(0);
        wait_state(&controller.0, |s| {
            s["local_state"] == "activation_required"
                && s["stations"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|host| host["serial"].is_null())
        })
        .await?;
        activated.send_replace(2);
        let ready = wait_state(&controller.0, |s| {
            origins
                .iter()
                .all(|id| station_snapshot(s, id)["state"] == "ready")
        })
        .await?;
        let first_serial = serial(&ready, &origins[0])?;
        let second_serial = serial(&ready, &origins[1])?;
        eprintln!("PASS stopped adbd clears both connections and reactivation restores both");
        let mut mesh = request(&hosts[0].base, reqwest::Method::GET, "/v1/node/mesh", None).await?
            ["config"]
            .clone();
        mesh["peers"] = json!([]);
        request(
            &hosts[0].base,
            reqwest::Method::PUT,
            "/v1/node/mesh",
            Some(mesh),
        )
        .await?;
        wait_state(&controller.0, |s| {
            station_snapshot(s, &origins[0])["state"] != "ready"
        })
        .await?;
        ensure!(
            TcpStream::connect(&first_serial).await.is_err(),
            "revoked Station listener stayed open"
        );
        assert_eq!(
            serial(&controller.0.snapshot(), &origins[1])?,
            second_serial
        );
        hosts[1].transfer(&adb_binary, &second_serial).await?;
        eprintln!("PASS revoking one Station closes its listener while the other still transfers");
        controller
            .0
            .execute(Action::SetEnabled { enabled: false })?;
        assert_eq!(controller.0.snapshot()["service_requested"], false);
        ensure!(
            TcpStream::connect(&second_serial).await.is_err(),
            "disabled listener stayed open"
        );
        controller.0.execute(Action::SetEnabled { enabled: true })?;
        let ready = wait_state(&controller.0, |s| {
            station_snapshot(s, &origins[1])["state"] == "ready"
        })
        .await?;
        let second_serial = serial(&ready, &origins[1])?;
        store.revoke_replica(&origins[0])?;
        controller.0.execute(Action::Refresh)?;
        let remaining = wait_state(&controller.0, |s| {
            s["stations"].as_array().unwrap().len() == 1
        })
        .await?;
        assert_eq!(serial(&remaining, &origins[1])?, second_serial);
        hosts[1].transfer(&adb_binary, &second_serial).await?;
        eprintln!("PASS global disable and re-enable, followed by local per-Station revocation");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    controller.0.pause();
    drop(fake_adbd);
    for host in &mut hosts {
        host.stop().await;
        let _ = host.adb.kill().await;
        let _ = host.adb.wait().await;
    }
    runtime.shutdown().await?;
    if result.is_err() {
        for host in &hosts {
            for file in ["station.log", "adb.log"] {
                if let Ok(log) = std::fs::read_to_string(host.identity.root.join(file)) {
                    eprintln!(
                        "{} {file}: {}",
                        host.identity.root.display(),
                        log.lines()
                            .rev()
                            .take(35)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .collect::<Vec<_>>()
                            .join("\n")
                    );
                }
            }
        }
    }
    result
}
