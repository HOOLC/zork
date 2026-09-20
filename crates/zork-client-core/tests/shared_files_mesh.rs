//! Three real Stations, including an initially empty accessor Station. No
//! directory registration, Skill export, or remote installation is performed.
use anyhow::{ensure, Context, Result};
use reqwest::Method;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::Duration,
};
use zork_client_core::{
    api::StationClient,
    shared_files::{Action, SharedFiles, SharedFilesData},
    store::ClientStore,
};
use zork_config::tree::{Reference, SHARED_FILES_SPACE, STATION_FILES_SPACE};
use zork_mesh::node::MeshNode;
const TOKEN: &str = "isolated-file-tree-fixture";
struct Station {
    root: PathBuf,
    url: String,
    agent_url: String,
    origin: String,
    process: Child,
}
impl Station {
    fn stop(&mut self) {
        if self.process.try_wait().ok().flatten().is_some() {
            return;
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(self.process.id() as i32, libc::SIGTERM);
        }
        let until = std::time::Instant::now() + Duration::from_secs(20);
        while self.process.try_wait().ok().flatten().is_none() {
            if std::time::Instant::now() > until {
                let _ = self.process.kill();
                let _ = self.process.wait();
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for Station {
    fn drop(&mut self) {
        self.stop();
    }
}
fn config() -> zork_config::MeshConfig {
    zork_config::MeshConfig {
        enabled: true,
        offline: true,
        bind: Some("127.0.0.1:0".into()),
        ..Default::default()
    }
}
async fn request(base: &str, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
    let mut request = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(45))
        .build()?
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
async fn identity(root: &Path) -> Result<String> {
    std::fs::create_dir_all(root)?;
    let mut runtime = zork_mesh::managed::start_client(root, &config()).await?;
    let origin = runtime.node().identity().await?;
    runtime.shutdown().await?;
    Ok(origin)
}
async fn start(
    binary: &Path,
    root: PathBuf,
    origin: String,
    client: &str,
    stations: &[String],
    routes: &BTreeMap<String, String>,
) -> Result<Station> {
    let mut listeners = vec![];
    let mut bind = serde_json::Map::new();
    for name in ["station", "runtime", "control", "agent"] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        bind.insert(name.into(), json!(listener.local_addr()?.to_string()));
        listeners.push(listener);
    }
    let url = format!("http://{}", bind["runtime"].as_str().unwrap());
    let agent_url = format!("http://{}", bind["agent"].as_str().unwrap());
    let peers = std::iter::once(client)
        .chain(
            stations
                .iter()
                .map(String::as_str)
                .filter(|other| *other != origin),
        )
        .map(|peer| json!({"origin":peer,"addr":routes[peer],"name":"Fixture peer","client":true,"execute":[]}))
        .collect::<Vec<_>>();
    let settings = json!({"bind":bind,"admin":{"token":TOKEN},"mesh":{"enabled":true,"offline":true,"bind":routes[&origin],"name":root.file_name().unwrap().to_string_lossy(),"peers":peers}});
    std::fs::write(root.join("config.json"), serde_json::to_vec(&settings)?)?;
    std::fs::create_dir_all(root.join("profiles"))?;
    std::fs::write(
        root.join("profiles/fixture.json"),
        serde_json::to_vec(
            &json!({"provider":"openai","billing":"usage","base_url":"http://127.0.0.1:9/v1","auth":{"type":"api_key","key":"sk-fixture"},"models":[{"id":"fixture-model","api":"openai-completions","streaming":false,"thinking":["off"],"default_thinking":"off","capabilities":{"input":["text"]},"limits":{"context_window_tokens":100000,"max_output_tokens":10000},"default":true}]}),
        )?,
    )?;
    drop(listeners);
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("station.log"))?;
    let process = Command::new(binary)
        .args(["--data", root.to_str().unwrap(), "--fake-agent"])
        .env("ZORK_REGISTRY_DIR", root.parent().unwrap().join("registry"))
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()?;
    let mut station = Station {
        root,
        url,
        agent_url,
        origin,
        process,
    };
    let until = tokio::time::Instant::now() + Duration::from_secs(45);
    loop {
        if request(&station.url, Method::GET, "/v1/node/shared-files", None)
            .await
            .is_ok()
        {
            return Ok(station);
        }
        if station.process.try_wait()?.is_some() || tokio::time::Instant::now() > until {
            anyhow::bail!(
                "Station failed: {}",
                std::fs::read_to_string(station.root.join("station.log"))?
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
async fn wait(
    source: &SharedFiles,
    condition: impl Fn(&SharedFilesData) -> bool,
) -> Result<Arc<SharedFilesData>> {
    let mut updates = source.subscribe();
    let mut ready = updates.readiness();
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            if let Some(batch) = updates.prepare() {
                let data = batch.snapshot.value.clone();
                updates.acknowledge(batch.id);
                if condition(&data) {
                    return data;
                }
            }
            ready.changed().await.unwrap();
        }
    })
    .await
    .with_context(|| {
        format!(
            "file tree did not converge: {}",
            serde_json::to_string(&source.snapshot()).unwrap()
        )
    })
}
async fn tool(
    station: &Station,
    chat: &str,
    session: &str,
    name: &str,
    args: Value,
) -> Result<Value> {
    tool_outcome(station, chat, session, name, args, "succeeded").await
}
async fn tool_outcome(
    station: &Station,
    chat: &str,
    session: &str,
    name: &str,
    args: Value,
    outcome: &str,
) -> Result<Value> {
    let history = format!("/sessions/{session}/history?limit=200");
    let before = match request(&station.agent_url, Method::GET, &history, None).await {
        Ok(value) => value,
        Err(error) if error.to_string().contains("404 Not Found") => json!({"items":[]}),
        Err(error) => return Err(error),
    };
    let seen = before["items"]
        .as_array()
        .context("history items")?
        .iter()
        .filter_map(|item| item["event_id"].as_str().map(str::to_owned))
        .collect::<std::collections::BTreeSet<_>>();
    request(&station.url,Method::POST,&format!("/v1/im/sessions/{chat}/messages"),Some(json!({"request_id":format!("fixture-{}",ulid::Ulid::new()),"content":json!({"fake_tool":{"name":name,"input":args}}).to_string()}))).await?;
    let until = tokio::time::Instant::now() + Duration::from_secs(45);
    loop {
        let history = match request(&station.agent_url, Method::GET, &history, None).await {
            Ok(value) => value,
            Err(error)
                if error.to_string().contains("404 Not Found")
                    && tokio::time::Instant::now() < until =>
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            Err(error) => return Err(error),
        };
        if let Some(result) = history["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| !seen.contains(item["event_id"].as_str().unwrap_or("")))
            .find_map(|item| {
                let event = &item["event"];
                (event["kind"] == "tool_result" && event["result"]["tool"] == name)
                    .then(|| event["result"].clone())
            })
        {
            ensure!(result["outcome"] == outcome, "{name}: {result}");
            return Ok(result["data"].clone());
        }
        ensure!(
            tokio::time::Instant::now() < until,
            "tool {name} did not finish: {history}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
fn write(root: &Path, path: &str, bytes: &[u8]) -> Result<()> {
    let path = if let Some(skill) = path.strip_prefix("skills/") {
        root.join("skills").join(skill)
    } else {
        zork_config::shared_files_root(root).join(path)
    };
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, bytes)?;
    Ok(())
}
async fn published(node: &MeshNode, reference: &Reference) -> Result<zork_mesh::node::ObjectRef> {
    let until = tokio::time::Instant::now() + Duration::from_secs(45);
    loop {
        let error = match node.tree_object(reference).await {
            Ok(object) => return Ok(object),
            Err(error) => error,
        };
        ensure!(
            tokio::time::Instant::now() < until,
            "file never published: {}: {error:#}",
            reference.uri(),
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn skill_catalog(station: &Station) -> Result<Value> {
    Ok(request(
        &station.url,
        Method::GET,
        "/v1/node/agents/reader/skills",
        None,
    )
    .await?["catalog"]
        .clone())
}

fn business_source_retired(root: &Path) -> Result<()> {
    let database = rusqlite::Connection::open_with_flags(
        zork_mesh::managed::data_dir(root).join("synchronicity.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    for table in ["sources", "entries", "local_files"] {
        let count: i64 = database.query_row(
            &format!("SELECT count(*) FROM {table} WHERE space IN ('files','zork')"),
            [],
            |row| row.get(0),
        )?;
        ensure!(count == 0, "retired business source remains in {table}");
    }
    Ok(())
}
fn reference(path: &str, origin: &str) -> Reference {
    Reference {
        space: if path.starts_with("skills/") {
            "skills"
        } else {
            SHARED_FILES_SPACE
        }
        .into(),
        path: path.strip_prefix("skills/").unwrap_or(path).into(),
        origin: Some(origin.into()),
        root: None,
        snapshot: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires ZORK_TEST_STATION_BIN pointing to a freshly built Station"]
async fn three_stations_publish_generic_tree_and_execute_remote_skills_without_installation(
) -> Result<()> {
    let binary =
        PathBuf::from(std::env::var_os("ZORK_TEST_STATION_BIN").context("build Station first")?);
    let root = tempfile::tempdir()?;
    let result = run(&binary, root.path()).await;
    let evidence = std::env::var_os("ZORK_SHARED_FILES_TEST_OUTPUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../artifacts/unified-file-tree/process")
        });
    std::fs::create_dir_all(&evidence)?;
    for name in ["first", "second", "third"] {
        let path = root.path().join(name).join("station.log");
        if path.exists() {
            std::fs::copy(path, evidence.join(format!("{name}.log")))?;
        }
    }
    result
}
async fn run(binary: &Path, root: &Path) -> Result<()> {
    let mut transport =
        zork_mesh::managed::start_client(&root.join("transport"), &config()).await?;
    let node = transport.node();
    let accessor = node.identity().await?;
    for i in 0..5000 {
        write(
            &root.join("first"),
            &format!("skills/many-resources/resources/{i:05}.txt"),
            b"resource",
        )?;
    }
    write(&root.join("first"),"skills/many-resources/SKILL.md",b"---\nname: many-resources\ndescription: A large resource tree\n---\nRead resources when needed.\n")?;
    std::fs::create_dir_all(
        zork_config::shared_files_root(&root.join("first")).join("empty-tree/nested"),
    )?;
    let first_id = identity(&root.join("first")).await?;
    let second_id = identity(&root.join("second")).await?;
    let third_id = identity(&root.join("third")).await?;
    // Model a previously used node whose old business source is already
    // published. The new user share must neither expose nor relocate it.
    let legacy_root = zork_config::files_root(&root.join("third"));
    for directory in ["jobs", "repos", "sessions", "workspaces"] {
        std::fs::create_dir_all(legacy_root.join(directory))?;
    }
    let legacy_file = legacy_root.join("workspaces/keep.txt");
    std::fs::write(&legacy_file, b"existing workspace bytes")?;
    let mut legacy = zork_mesh::managed::start_client(&root.join("third"), &config()).await?;
    legacy
        .node()
        .add_filesystem_source(STATION_FILES_SPACE, &legacy_root)
        .await?;
    legacy
        .node()
        .schedule_source_scan(STATION_FILES_SPACE)
        .await?;
    let legacy_reference = Reference {
        space: STATION_FILES_SPACE.into(),
        ..reference("workspaces/keep.txt", &third_id)
    };
    published(&legacy.node(), &legacy_reference).await?;
    legacy
        .node()
        .trust(
            &accessor,
            "accessor before upgrade",
            node.address()?
                .ip_addrs()
                .next()
                .map(ToString::to_string)
                .as_deref(),
        )
        .await?;
    node.trust(
        &third_id,
        "source before upgrade",
        legacy
            .node()
            .address()?
            .ip_addrs()
            .next()
            .map(ToString::to_string)
            .as_deref(),
    )
    .await?;
    let old_object = published(&node, &legacy_reference).await?;
    ensure!(node.tree_read(&old_object).await? == b"existing workspace bytes");
    legacy.shutdown().await?;
    write(
        &root.join("third"),
        "skills/damaged/.zork/.state.json",
        b"{",
    )?;
    let ids = vec![first_id.clone(), second_id.clone(), third_id.clone()];
    // This contract exercises publication and consumption, not multicast
    // discovery (covered by Mesh tests). Give each isolated peer a direct route.
    let mut routes = BTreeMap::from([(
        accessor.clone(),
        node.address()?
            .ip_addrs()
            .next()
            .context("accessor route")?
            .to_string(),
    )]);
    let mut reserved = BTreeMap::new();
    for origin in &ids {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0")?;
        routes.insert(origin.clone(), socket.local_addr()?.to_string());
        reserved.insert(origin.clone(), socket);
    }
    for (name, body) in [
        ("first", b"first version".as_slice()),
        ("second", b"second version".as_slice()),
    ] {
        write(&root.join(name), "project/same.txt", b"same content")?;
        write(&root.join(name), "project/version.txt", body)?;
    }
    let managed = ulid::Ulid::new().to_string();
    write(
        &root.join("first"),
        &format!("skills/{managed}/SKILL.md"),
        b"---\nname: bound-guide\ndescription: Explicit source binding\n---\nBOUND_GUIDE\n",
    )?;
    write(
        &root.join("first"),
        &format!("skills/{managed}/large-resource.bin"),
        &vec![b'x'; 100 * 1024],
    )?;
    for i in 0..35 {
        write(
            &root.join("first"),
            &format!("skills/{managed}/resource-{i}.txt"),
            b"resource",
        )?;
    }
    write(
        &root.join("first"),
        "skills/retired/.skill-archive/old.md",
        b"retired manifest",
    )?;
    write(&root.join("first"), "skills/retired/resources/example/SKILL.md", b"---\nname: archived-resource\ndescription: A retained example\n---\nNot an active Skill.\n")?;
    write(&root.join("first"), "skills/.hidden/SKILL.md", b"---\nname: hidden-source\ndescription: An implicit hidden directory\n---\nNot an active Skill.\n")?;
    let manifest=b"---\nname: remote-guide\ndescription: Read from a different Station\n---\nREMOTE_GUIDE_V1\nUse scripts/run.sh.\n";
    write(
        &root.join("first"),
        "skills/remote-guide/SKILL.md",
        manifest,
    )?;
    write(
        &root.join("first"),
        "skills/invalid/SKILL.md",
        b"malformed manifest",
    )?;
    write(
        &root.join("first"),
        "skills/remote-guide/scripts/run.sh",
        b"#!/bin/sh\nprintf REMOTE_SCRIPT_V1\\n\n",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            root.join("first/skills/remote-guide/scripts/run.sh"),
            std::fs::Permissions::from_mode(0o755),
        )?;
        std::os::unix::fs::symlink(
            "same.txt",
            zork_config::shared_files_root(&root.join("first")).join("project/link"),
        )?;
        std::os::unix::fs::symlink(
            "large-resource.bin",
            root.join(format!("first/skills/{managed}/linked-resource")),
        )?;
    }
    reserved.remove(&first_id);
    let mut first = start(
        binary,
        root.join("first"),
        first_id.clone(),
        &accessor,
        &ids,
        &routes,
    )
    .await?;
    reserved.remove(&second_id);
    let second = start(
        binary,
        root.join("second"),
        second_id,
        &accessor,
        &ids,
        &routes,
    )
    .await?;
    reserved.remove(&third_id);
    let third = start(
        binary,
        root.join("third"),
        third_id,
        &accessor,
        &ids,
        &routes,
    )
    .await?;
    let empty = request(
        &third.url,
        Method::POST,
        "/v1/node/shared-files/directory",
        Some(json!({
            "space": SHARED_FILES_SPACE, "path": "", "origin": third.origin, "after": null,
        })),
    )
    .await?;
    ensure!(
        empty["entries"]
            .as_array()
            .context("shared directory entries")?
            .is_empty(),
        "system directories leaked into a default user share: {empty}"
    );
    ensure!(
        std::fs::read_dir(zork_config::shared_files_root(&third.root))?
            .next()
            .is_none()
    );
    ensure!(std::fs::read(&legacy_file)? == b"existing workspace bytes");
    business_source_retired(&third.root)?;
    let rejected = reqwest::Client::builder()
        .no_proxy()
        .build()?
        .post(format!("{}/v1/node/shared-files/directory", third.url))
        .bearer_auth(TOKEN)
        .json(&json!({"space": STATION_FILES_SPACE, "after": null}))
        .send()
        .await?;
    ensure!(rejected.status() == reqwest::StatusCode::BAD_REQUEST);
    println!("Three Stations ready; the existing node's user share is empty and its system files remain intact");
    for origin in &ids {
        node.trust(origin, "unified tree fixture", Some(&routes[origin]))
            .await?;
    }
    // A single saved Station binding suffices: the tree comes from the one
    // embedded Synch node, not from an HTTP catalog fanout to saved Stations.
    let source = SharedFiles::new(Arc::new(ClientStore::open(&root.join("client"))?));
    let client = Arc::new(StationClient::new_mesh(node.clone(), third.origin.clone()));
    source.replace_devices(vec![(
        "empty-station".into(),
        "Third Station".into(),
        false,
        client,
    )]);
    source.dispatch(Action::Activate { active: true }).await?;
    wait(&source, |s| {
        s.spaces
            .iter()
            .any(|space| space.id == SHARED_FILES_SPACE && space.sources.len() == 2)
    })
    .await?;
    source
        .dispatch(Action::OpenSpace {
            space: SHARED_FILES_SPACE.into(),
        })
        .await?;
    wait(&source, |s| s.entries.iter().any(|e| e.path == "project")).await?;
    ensure!(
        source
            .snapshot()
            .entries
            .iter()
            .any(|e| e.path == "empty-tree"),
        "empty directory was not published across Stations"
    );
    ensure!(
        !source.snapshot().entries.iter().any(|e| [
            "jobs",
            "repos",
            "sessions",
            "workspaces",
            "skills",
            "zork-control"
        ]
        .contains(&e.path.as_str())),
        "business spaces mixed in shared files"
    );
    let until = tokio::time::Instant::now() + Duration::from_secs(45);
    while !node
        .tree_space(STATION_FILES_SPACE)
        .await?
        .spaces
        .is_empty()
    {
        ensure!(
            tokio::time::Instant::now() < until,
            "peer retained the retired business tree"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    ensure!(node.tree_object(&legacy_reference).await.is_err());
    write(
        &third.root,
        "jobs/user.txt",
        b"a user directory may have any name",
    )?;
    let user_file = published(&node, &reference("jobs/user.txt", &third.origin)).await?;
    ensure!(node.tree_read(&user_file).await? == b"a user directory may have any name");
    wait(&source, |s| {
        s.entries.iter().any(|entry| entry.path == "jobs")
    })
    .await?;
    println!("Retirement reaches an existing peer, preserves local business files and allows arbitrary user directory names");
    source
        .dispatch(Action::OpenEntry {
            id: "project".into(),
        })
        .await?;
    let data = wait(&source, |s| {
        s.entries
            .iter()
            .any(|e| e.path == "project/version.txt" && e.versions.len() == 2)
    })
    .await?;
    let same = data
        .entries
        .iter()
        .find(|e| e.path == "project/same.txt")
        .unwrap();
    ensure!(same.versions.len() == 1 && same.versions[0].sources.len() == 2);
    #[cfg(unix)]
    ensure!(data
        .entries
        .iter()
        .any(|e| e.path == "project/link" && e.target.as_deref() == Some("same.txt")));
    source
        .dispatch(Action::OpenEntry {
            id: "project/version.txt".into(),
        })
        .await?;
    source
        .dispatch(Action::SelectVersion {
            root: zork_mesh::content_root(b"first version"),
        })
        .await?;
    source.dispatch(Action::PrepareSave).await?;
    let ticket = source
        .snapshot()
        .save
        .ticket
        .clone()
        .context("fixed save ticket missing")?;
    write(&first.root, "project/version.txt", b"first updated")?;
    wait(&source, |s| {
        s.entries.iter().any(|e| {
            e.versions
                .iter()
                .any(|v| v.root == zork_mesh::content_root(b"first updated"))
        })
    })
    .await?;
    let mut saved = vec![];
    source.write_copy(&ticket, &mut saved)?;
    ensure!(saved == b"first version");
    source.dispatch(Action::ClosePreview).await?;
    println!("Merged metadata, versions, symlink and immutable copy verified");

    request(&third.url,Method::POST,"/v1/node/agents",Some(json!({"id":"reader","name":"Reader","role":"leader","profile_id":"fixture","model":"fixture-model","thinking":"off"}))).await?;
    let opened = request(
        &third.url,
        Method::POST,
        "/v1/node/agents/reader/open",
        Some(json!({})),
    )
    .await?;
    let session = opened["session_id"]
        .as_str()
        .context("Chat identity missing")?;
    let execution = opened["agent"]["session_id"]
        .as_str()
        .context("Execution identity missing")?;
    let until = tokio::time::Instant::now() + Duration::from_secs(45);
    let guide = loop {
        let catalog = request(
            &third.url,
            Method::GET,
            "/v1/node/agents/reader/skills",
            None,
        )
        .await?;
        if let Some(guide) = catalog["catalog"]["skills"]
            .as_array()
            .unwrap()
            .iter()
            .find(|skill| skill["name"] == "remote-guide")
        {
            ensure!(
                catalog["catalog"]["diagnostics"]
                    .to_string()
                    .contains("invalid/SKILL.md"),
                "bad manifest hid valid Skill discovery"
            );
            break guide.clone();
        }
        ensure!(
            tokio::time::Instant::now() < until,
            "remote Skill never discovered: {catalog}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let uri = guide["path"].as_str().unwrap();
    ensure!(uri.starts_with("synch://skills/"));
    ensure!(!third.root.join("skills/remote-guide").exists());
    let read = tool(&third, session, execution, "file.read", json!({"path":uri})).await?;
    ensure!(read["content"]
        .as_str()
        .unwrap()
        .contains("REMOTE_GUIDE_V1"));
    let listed = skill_catalog(&third).await?;
    ensure!(
        !listed["skills"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"] == "archived-resource" || s["name"] == "hidden-source"),
        "implicit archived or hidden resources became remote Skills"
    );
    ensure!(listed["skills"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["path"] == uri));
    let directory_page = tool(
        &third,
        session,
        execution,
        "file.list",
        json!({"path":guide["source"],"limit":1}),
    )
    .await?;
    ensure!(directory_page["path"]
        .as_str()
        .unwrap()
        .contains("snapshot="));
    ensure!(directory_page["next_cursor"].is_string());
    let next_page = tool(
        &third,
        session,
        execution,
        "file.list",
        json!({"path":directory_page["path"],"cursor":directory_page["next_cursor"],"limit":1}),
    )
    .await?;
    ensure!(!next_page["entries"].as_array().unwrap().is_empty());
    let public = request(
        &third.url,
        Method::GET,
        "/v1/node/agents/reader/skills/catalog",
        None,
    )
    .await?;
    let skill = public["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "remote-guide")
        .unwrap();
    let details = request(
        &third.url,
        Method::GET,
        &format!(
            "/v1/node/agents/reader/skills/{}?file=scripts%2Frun.sh",
            skill["id"].as_str().unwrap()
        ),
        None,
    )
    .await?;
    ensure!(details["document"]["text"]
        .as_str()
        .unwrap()
        .contains("REMOTE_SCRIPT_V1"));
    let materialized = tool(
        &third,
        session,
        execution,
        "file.materialize",
        json!({"path":guide["source"]}),
    )
    .await?;
    let directory = PathBuf::from(materialized["path"].as_str().unwrap());
    ensure!(directory.starts_with(third.root.join("state/file-materializations")));
    let command = format!("'{}'", directory.join("scripts/run.sh").display());
    let executed = tool(
        &third,
        session,
        execution,
        "shell.run",
        json!({"command":command}),
    )
    .await?;
    ensure!(executed["output"]
        .as_str()
        .unwrap()
        .contains("REMOTE_SCRIPT_V1"));
    let single_reference =
        Reference::parse(guide["source"].as_str().unwrap())?.child("scripts/run.sh");
    let single = tool(
        &third,
        session,
        execution,
        "file.materialize",
        json!({"path":single_reference.uri()}),
    )
    .await?;
    let single_path = PathBuf::from(single["path"].as_str().unwrap());
    let output = Command::new(&single_path).output()?;
    ensure!(
        output.status.success() && String::from_utf8(output.stdout)?.contains("REMOTE_SCRIPT_V1"),
        "single-file materialization lost executable permissions"
    );
    let old_manifest = published(
        &node,
        &reference("skills/remote-guide/SKILL.md", &first.origin),
    )
    .await?;
    let next = b"---\nname: remote-guide\ndescription: Changed on owner\n---\nREMOTE_GUIDE_V2\n";
    write(&first.root, "skills/remote-guide/SKILL.md", next)?;
    write(
        &first.root,
        "skills/remote-guide/scripts/run.sh",
        b"#!/bin/sh\nprintf REMOTE_SCRIPT_V2\\n\n",
    )?;
    let until = tokio::time::Instant::now() + Duration::from_secs(45);
    loop {
        let catalog = skill_catalog(&third).await?;
        if catalog["skills"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"] == "remote-guide" && s["content_hash"] != guide["content_hash"])
        {
            break;
        }
        ensure!(
            tokio::time::Instant::now() < until,
            "Skill update not observed"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    ensure!(std::fs::read_to_string(directory.join("scripts/run.sh"))?.contains("REMOTE_SCRIPT_V1"));
    ensure!(node.tree_read(&old_manifest).await? == manifest);
    let newer = tool(
        &third,
        session,
        execution,
        "file.materialize",
        json!({"path":skill_catalog(&third).await?["skills"].as_array().unwrap().iter().find(|s|s["name"]=="remote-guide").unwrap()["source"]}),
    )
    .await?;
    let newer = PathBuf::from(newer["path"].as_str().unwrap());
    ensure!(newer != directory);
    ensure!(std::fs::read_to_string(newer.join("scripts/run.sh"))?.contains("REMOTE_SCRIPT_V2"));
    ensure!(!third.root.join("skills/remote-guide").exists());
    println!(
        "Remote Skill catalog, ordinary file.read, resource details and real shell execution verified without installation"
    );

    let catalog = skill_catalog(&third).await?;
    let binding = catalog["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|skill| skill["name"] == "bound-guide")
        .context("large remote Skill not discovered")?["source"]
        .clone();
    let materialized = tool(
        &third,
        session,
        execution,
        "file.materialize",
        json!({"path":binding}),
    )
    .await?;
    let directory = PathBuf::from(materialized["path"].as_str().context("materialized path")?);
    ensure!(std::fs::read(directory.join("large-resource.bin"))? == vec![b'x'; 100 * 1024]);
    for i in 0..35 {
        ensure!(std::fs::read(directory.join(format!("resource-{i}.txt")))? == b"resource");
    }
    #[cfg(unix)]
    ensure!(
        std::fs::read_link(directory.join("linked-resource"))? == Path::new("large-resource.bin")
    );
    ensure!(!third.root.join("skills").join(&managed).exists());
    println!(
        "Ordinary file.materialize preserves large/numerous Skill resources without installation"
    );

    let empty = zork_config::shared_files_root(&first.root).join("snapshot-empty");
    std::fs::create_dir_all(&empty)?;
    let live = reference("", &first.origin).uri();
    let until = tokio::time::Instant::now() + Duration::from_secs(30);
    let pinned = loop {
        let page = tool(
            &third,
            session,
            execution,
            "file.list",
            json!({"path":live}),
        )
        .await?;
        if let Some(entry) = page["entries"].as_array().unwrap().iter().find(|entry| {
            Reference::parse(entry["path"].as_str().unwrap())
                .unwrap()
                .path
                == "snapshot-empty"
        }) {
            break Reference::parse(entry["path"].as_str().unwrap())?;
        }
        ensure!(
            tokio::time::Instant::now() < until,
            "empty directory did not publish"
        );
    };
    std::fs::remove_dir(&empty)?;
    let future = zork_config::shared_files_root(&first.root).join("snapshot-future");
    std::fs::create_dir_all(&future)?;
    let until = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let page = tool(
            &third,
            session,
            execution,
            "file.list",
            json!({"path":live}),
        )
        .await?;
        let paths = page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                Reference::parse(entry["path"].as_str().unwrap())
                    .unwrap()
                    .path
            })
            .collect::<Vec<_>>();
        if paths.contains(&"snapshot-future".into()) && !paths.contains(&"snapshot-empty".into()) {
            break;
        }
        ensure!(
            tokio::time::Instant::now() < until,
            "directory changes did not publish"
        );
    }
    let materialized = tool(
        &third,
        session,
        execution,
        "file.materialize",
        json!({"path":pinned.uri()}),
    )
    .await?;
    ensure!(Path::new(materialized["path"].as_str().unwrap()).is_dir());
    let absent = Reference {
        path: "snapshot-future".into(),
        ..pinned
    };
    let rejected = tool_outcome(
        &third,
        session,
        execution,
        "file.materialize",
        json!({"path":absent.uri()}),
        "failed",
    )
    .await?;
    ensure!(rejected["error"].as_str().unwrap().contains("snapshot"));

    write(&first.root, "skills/SKILL.md", b"---\nname: source-root-guide\ndescription: A source root is a valid Skill\n---\nROOT_SKILL_BODY\n")?;
    let until = tokio::time::Instant::now() + Duration::from_secs(30);
    let root_skill = loop {
        let catalog = request(
            &third.url,
            Method::GET,
            "/v1/node/agents/reader/skills/catalog",
            None,
        )
        .await?;
        if let Some(skill) = catalog["skills"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == "source-root-guide")
        {
            break skill.clone();
        }
        ensure!(
            tokio::time::Instant::now() < until,
            "source-root Skill did not appear"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let mut inspection = reqwest::Url::parse(&format!(
        "{}/v1/node/agents/reader/skills/{}",
        third.url,
        root_skill["id"].as_str().unwrap()
    ))?;
    inspection
        .query_pairs_mut()
        .append_pair("reference", root_skill["path"].as_str().unwrap());
    let details = request(
        &third.url,
        Method::GET,
        &format!("{}?{}", inspection.path(), inspection.query().unwrap()),
        None,
    )
    .await?;
    ensure!(details["document"]["text"]
        .as_str()
        .unwrap()
        .contains("ROOT_SKILL_BODY"));
    println!(
        "Damaged management metadata is isolated; empty snapshots and root-level Skill inspection verified"
    );

    let attachment = (0..75_000).map(|i| (i % 251) as u8).collect::<Vec<_>>();
    let content_root = zork_mesh::content_root(&attachment);
    for (index, bytes) in attachment
        .chunks(zork_client_core::files::CHUNK_BYTES)
        .enumerate()
    {
        request(&third.url,Method::POST,&format!("/v1/im/sessions/{session}/files"),Some(json!({"file":{"id":"file-fixture","name":"attachment.txt","byte_len":attachment.len(),"content_root":content_root},"offset":index*zork_client_core::files::CHUNK_BYTES,"bytes":bytes}))).await?;
    }
    let attachment_path = format!("attachments/{content_root}/attachment.txt");
    ensure!(
        std::fs::read(zork_config::files_root(&third.root).join(&attachment_path))? == attachment
    );
    let bytes = reqwest::Client::builder()
        .no_proxy()
        .build()?
        .get(format!("{}/v1/artifacts/file-fixture/content", third.url))
        .bearer_auth(TOKEN)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    ensure!(bytes.as_ref() == attachment.as_slice());
    let remote = StationClient::new_mesh(node.clone(), third.origin.clone());
    ensure!(remote.artifact_content("file-fixture").await? == attachment);
    let mesh_database = rusqlite::Connection::open_with_flags(
        zork_mesh::managed::data_dir(&third.root).join("synchronicity.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    ensure!(
        mesh_database.query_row(
            "SELECT count(*) FROM sources WHERE space='zork-client'",
            [],
            |row| row.get::<_, i64>(0)
        )? == 0,
        "client attachment download created a permanent response publication"
    );
    drop(mesh_database);
    business_source_retired(&third.root)?;
    let database = rusqlite::Connection::open_with_flags(
        third.root.join("state/station.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let metadata: String = database.query_row(
        "SELECT snapshot FROM conversation_file_snapshots WHERE artifact_id='file-fixture'",
        [],
        |r| r.get(0),
    )?;
    ensure!(serde_json::from_str::<Value>(&metadata)?["byte_len"] == attachment.len());
    drop(database);
    ensure!(third
        .root
        .join("shared-files/sessions")
        .join(execution)
        .join("segments")
        .is_dir());
    write(
        &first.root,
        "invented-after-start/nested/unclassified.txt",
        b"automatic",
    )?;
    let arbitrary = published(
        &node,
        &reference(
            "invented-after-start/nested/unclassified.txt",
            &first.origin,
        ),
    )
    .await?;
    ensure!(node.tree_read(&arbitrary).await? == b"automatic");
    ensure!(node
        .tree_space(STATION_FILES_SPACE)
        .await?
        .spaces
        .is_empty());
    ensure!(std::fs::read(&legacy_file)? == b"existing workspace bytes");
    println!(
        "Session history and Chat attachments stay local; deliberate new directories still publish"
    );

    source
        .dispatch(Action::Source {
            peer: Some(first.origin.clone()),
        })
        .await?;
    wait(&source, |s| {
        !s.loading && s.entries.iter().any(|e| e.path == "project/version.txt")
    })
    .await?;
    source
        .dispatch(Action::OpenEntry {
            id: "project/version.txt".into(),
        })
        .await?;
    source.dispatch(Action::PrepareSave).await?;
    let revoked_ticket = source.snapshot().save.ticket.clone().unwrap();
    let mut subscription = source.subscribe();
    let stale = subscription.prepare().unwrap();
    first.stop();
    ensure!(node.tree_read(&arbitrary).await? == b"automatic");
    node.untrust(&first.origin).await?;
    wait(&source, |s| {
        !s.devices.iter().any(|d| d.id == first.origin) && s.preview.is_none()
    })
    .await?;
    ensure!(!subscription.valid(stale.id));
    ensure!(source.write_copy(&revoked_ticket, &mut vec![]).is_err());
    ensure!(node.tree_read(&arbitrary).await.is_err());
    first = start(
        binary,
        root.join("first"),
        first_id,
        &accessor,
        &ids,
        &routes,
    )
    .await?;
    node.trust(&first.origin, "restored fixture trust", None)
        .await?;
    wait(&source, |s| s.devices.iter().any(|d| d.id == first.origin)).await?;
    ensure!(node.tree_read(&arbitrary).await? == b"automatic");
    source.pause();
    drop(source);
    transport.shutdown().await?;
    let mut reopened = zork_mesh::managed::start_client(&root.join("transport"), &config()).await?;
    ensure!(reopened.node().identity().await? == accessor);
    reopened.shutdown().await?;
    drop(first);
    drop(second);
    drop(third);
    println!(
        "PASS: three real Stations, generic automatic publication, remote Skill consumption, fixed versions, restart and trust withdrawal"
    );
    Ok(())
}
