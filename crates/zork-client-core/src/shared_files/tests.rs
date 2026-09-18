use super::*;
use std::time::Duration;

const SPACE: &str = zork_config::tree::SHARED_FILES_SPACE;
async fn wait(
    source: &SharedFiles,
    condition: impl Fn(&SharedFilesData) -> bool,
) -> Arc<SharedFilesData> {
    let mut updates = source.subscribe();
    let mut ready = updates.readiness();
    tokio::time::timeout(Duration::from_secs(10), async {
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
    .unwrap_or_else(|_| {
        panic!(
            "file tree did not converge: {}",
            serde_json::to_string(&source.snapshot()).unwrap()
        )
    })
}
async fn runtime(root: &std::path::Path) -> zork_mesh::managed::Runtime {
    zork_mesh::managed::start_client(
        root,
        &zork_config::MeshConfig {
            enabled: true,
            offline: true,
            ..Default::default()
        },
    )
    .await
    .unwrap()
}
fn bind(source: &Arc<SharedFiles>, client: Arc<StationClient>) {
    source.replace_devices(vec![(
        "station-binding".into(),
        "Station".into(),
        false,
        client,
    )]);
}
async fn open(source: &Arc<SharedFiles>) {
    source
        .dispatch(Action::Activate { active: true })
        .await
        .unwrap();
    wait(source, |s| s.spaces.iter().any(|space| space.id == SPACE)).await;
    source
        .dispatch(Action::OpenSpace {
            space: SPACE.into(),
        })
        .await
        .unwrap();
    wait(source, |s| !s.loading && !s.entries.is_empty()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_embedded_node_uses_all_api_paths_and_bounded_pages() {
    let root = tempfile::tempdir().unwrap();
    let mut runtime = runtime(&root.path().join("mesh")).await;
    let node = runtime.node();
    node.add_api_source(SPACE).await.unwrap();
    for space in [
        zork_config::tree::STATION_FILES_SPACE,
        "skills",
        "zork-control",
    ] {
        node.add_api_source(space).await.unwrap();
        node.put(space, "internal.txt", b"separate namespace")
            .await
            .unwrap();
    }
    for i in 0..267 {
        node.put(SPACE, &format!("entry-{i:03}.txt"), b"body")
            .await
            .unwrap();
    }
    node.put(SPACE, "unfamiliar/new-category/file.txt", b"new category")
        .await
        .unwrap();
    let handle = MeshNode::unbound(node.data_dir().to_path_buf());
    let client = Arc::new(StationClient::new_mesh(
        handle.clone(),
        node.identity().await.unwrap(),
    ));
    let source = SharedFiles::new(Arc::new(
        ClientStore::open(&root.path().join("client")).unwrap(),
    ));
    bind(&source, client.clone());
    source
        .dispatch(Action::Activate { active: true })
        .await
        .unwrap();
    assert!(source.snapshot().offline);
    handle.attach(&node).unwrap();
    bind(&source, client);
    open(&source).await;
    assert!(source
        .snapshot()
        .spaces
        .iter()
        .all(|space| space.id == SPACE));
    assert!(!source
        .snapshot()
        .entries
        .iter()
        .any(|entry| entry.path == "internal.txt"));
    assert!(source
        .dispatch(Action::OpenSpace {
            space: "skills".into()
        })
        .await
        .is_err());
    assert_eq!(source.snapshot().entries.len(), 128);
    source.dispatch(Action::More).await.unwrap();
    assert_eq!(source.snapshot().entries.len(), 256);
    let (epoch, window, catalog) = {
        let s = source.owned.lock().unwrap();
        (s.epoch, s.window, s.catalog.clone())
    };
    source.accept(
        epoch,
        window,
        Ok((
            catalog,
            Err(anyhow::anyhow!("transient directory read failure")),
        )),
    );
    assert_eq!(
        source.snapshot().entries.len(),
        256,
        "a failed refresh retains the last complete range"
    );
    assert!(source.snapshot().error.is_some());
    let updated = node
        .put(SPACE, "entry-000.txt", b"changed after paging")
        .await
        .unwrap();
    wait(&source, |s| {
        s.entries
            .iter()
            .any(|e| e.path == "entry-000.txt" && e.versions.iter().any(|v| v.root == updated.root))
    })
    .await;
    assert_eq!(
        source.snapshot().entries.len(),
        256,
        "a file update must retain the loaded range"
    );
    source.dispatch(Action::More).await.unwrap();
    assert_eq!(source.snapshot().entries.len(), 268);
    assert!(!source.snapshot().more);
    source
        .dispatch(Action::Sort {
            sort: Sort::NameDescending,
        })
        .await
        .unwrap();
    wait(&source, |s| {
        !s.loading && s.entries.first().is_some_and(|e| e.path == "unfamiliar")
    })
    .await;
    source
        .dispatch(Action::OpenEntry {
            id: "unfamiliar".into(),
        })
        .await
        .unwrap();
    wait(&source, |s| {
        s.entries
            .first()
            .is_some_and(|e| e.path == "unfamiliar/new-category")
    })
    .await;
    source
        .dispatch(Action::OpenEntry {
            id: "unfamiliar/new-category".into(),
        })
        .await
        .unwrap();
    wait(&source, |s| {
        s.entries
            .first()
            .is_some_and(|e| e.path == "unfamiliar/new-category/file.txt")
    })
    .await;
    source
        .dispatch(Action::OpenEntry {
            id: "unfamiliar/new-category/file.txt".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        source.snapshot().preview.as_ref().unwrap().text.as_deref(),
        Some("new category")
    );
    source.dispatch(Action::ClosePreview).await.unwrap();
    source
        .dispatch(Action::Search {
            query: "missing".into(),
        })
        .await
        .unwrap();
    wait(&source, |s| {
        !s.loading && s.empty == Some(EmptyState::NoResults)
    })
    .await;
    node.put(
        SPACE,
        "unfamiliar/new-category/session.jsonl",
        b"{\"event\":\"snapshot\"}\n",
    )
    .await
    .unwrap();
    source
        .dispatch(Action::Search {
            query: "session.jsonl".into(),
        })
        .await
        .unwrap();
    wait(&source, |s| {
        !s.loading && s.entries.len() == 1 && s.entries[0].name == "session.jsonl"
    })
    .await;
    source
        .dispatch(Action::OpenEntry {
            id: "unfamiliar/new-category/session.jsonl".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        source.snapshot().preview.as_ref().unwrap().text.as_deref(),
        Some("{\"event\":\"snapshot\"}\n")
    );
    for _ in 0..5 {
        source.dispatch(Action::Back).await.unwrap();
    }
    assert!(
        !source.snapshot().active,
        "root Back closes the shared-file page on Android"
    );
    source.pause();
    runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn system_file_metadata_does_not_populate_an_empty_user_share() {
    let root = tempfile::tempdir().unwrap();
    let mut runtime = runtime(&root.path().join("mesh")).await;
    let node = runtime.node();
    let names = ["jobs", "repos", "sessions", "workspaces"];
    node.add_api_source(zork_config::tree::STATION_FILES_SPACE)
        .await
        .unwrap();
    for name in names {
        node.put(
            zork_config::tree::STATION_FILES_SPACE,
            &format!("{name}/system.txt"),
            b"existing system file",
        )
        .await
        .unwrap();
    }
    node.add_api_source(SPACE).await.unwrap();
    let source = SharedFiles::new(Arc::new(
        ClientStore::open(&root.path().join("client")).unwrap(),
    ));
    bind(
        &source,
        Arc::new(StationClient::new_mesh(
            node.clone(),
            node.identity().await.unwrap(),
        )),
    );
    source
        .dispatch(Action::Activate { active: true })
        .await
        .unwrap();
    let empty = wait(&source, |s| {
        !s.loading && s.error.is_none() && !s.spaces.is_empty()
    })
    .await;
    assert!(empty.entries.is_empty());
    assert!(source
        .dispatch(Action::OpenSpace {
            space: zork_config::tree::STATION_FILES_SPACE.into(),
        })
        .await
        .is_err());

    for name in names {
        node.put(SPACE, &format!("{name}/user.txt"), b"explicitly shared")
            .await
            .unwrap();
    }
    let shared = wait(&source, |s| s.entries.len() == names.len()).await;
    assert!(names
        .iter()
        .all(|name| shared.entries.iter().any(|e| e.path == *name)));
    source
        .dispatch(Action::Activate { active: false })
        .await
        .unwrap();
    runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn source_presence_and_save_capability_follow_the_selected_business_state() {
    let root = tempfile::tempdir().unwrap();
    let mut runtime = runtime(&root.path().join("mesh")).await;
    let node = runtime.node();
    let origin = node.identity().await.unwrap();
    node.add_api_source(SPACE).await.unwrap();
    let small = node.put(SPACE, "report.txt", b"readable").await.unwrap();
    let client = Arc::new(StationClient::new_mesh(node.clone(), origin.clone()));
    let source = SharedFiles::new(Arc::new(
        ClientStore::open(&root.path().join("client")).unwrap(),
    ));
    bind(&source, client.clone());
    open(&source).await;
    assert_eq!(source.snapshot().devices[0].online, None);
    let mut state = crate::state::DeviceData::default();
    Arc::make_mut(&mut state.mesh).origin = Some(origin.clone());
    state.online = Some(false);
    source.update_device("station-binding", &client, &state);
    assert_eq!(source.snapshot().devices[0].online, Some(false));
    state.online = Some(true);
    source.update_device("station-binding", &client, &state);
    assert_eq!(source.snapshot().devices[0].online, Some(true));
    source
        .dispatch(Action::OpenEntry {
            id: "report.txt".into(),
        })
        .await
        .unwrap();
    // The tree can list a version above the browser's read limit without
    // fetching its body. Exercise switching in both directions.
    let large = zork_mesh::content_root(b"large metadata fixture");
    {
        let mut s = source.owned.lock().unwrap();
        let p = s.data.preview.as_mut().unwrap();
        let mut version = p.versions[0].clone();
        version.root = large.clone();
        version.size = 301 * 1024 * 1024;
        version.can_read = false;
        p.versions.push(version);
    }
    source
        .dispatch(Action::SelectVersion { root: large })
        .await
        .unwrap();
    assert!(!source.snapshot().preview.as_ref().unwrap().can_save);
    source
        .dispatch(Action::SelectVersion { root: small.root })
        .await
        .unwrap();
    assert!(source.snapshot().preview.as_ref().unwrap().can_save);
    let replacement = Arc::new(StationClient::new_mesh(
        node.clone(),
        "key:replacement".into(),
    ));
    bind(&source, replacement);
    state.revoked = true;
    source.update_device("station-binding", &client, &state);
    assert_eq!(source.access_peers(), vec!["station-binding"]);
    source.pause();
    runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn immutable_copy_survives_path_update_and_binding_revoke_invalidates_pending_frame() {
    let root = tempfile::tempdir().unwrap();
    let mut runtime = runtime(&root.path().join("mesh")).await;
    let node = runtime.node();
    node.add_api_source(SPACE).await.unwrap();
    let old = node.put(SPACE, "report.txt", b"original").await.unwrap();
    let client = Arc::new(StationClient::new_mesh(
        node.clone(),
        node.identity().await.unwrap(),
    ));
    let source = SharedFiles::new(Arc::new(
        ClientStore::open(&root.path().join("client")).unwrap(),
    ));
    bind(&source, client);
    open(&source).await;
    source
        .dispatch(Action::OpenEntry {
            id: "report.txt".into(),
        })
        .await
        .unwrap();
    source.dispatch(Action::PrepareSave).await.unwrap();
    let ticket = source.snapshot().save.ticket.clone().unwrap();
    let new = node.put(SPACE, "report.txt", b"newer").await.unwrap();
    wait(&source, |s| {
        s.entries
            .iter()
            .any(|e| e.versions.iter().any(|v| v.root == new.root))
    })
    .await;
    let mut bytes = vec![];
    source.write_copy(&ticket, &mut bytes).unwrap();
    assert_eq!(bytes, b"original");
    assert_eq!(node.tree_read(&old).await.unwrap(), b"original");
    source.dispatch(Action::ClosePreview).await.unwrap();
    source
        .dispatch(Action::OpenEntry {
            id: "report.txt".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        source.snapshot().preview.as_ref().unwrap().text.as_deref(),
        Some("newer")
    );
    source.dispatch(Action::PrepareSave).await.unwrap();
    let ticket = source.snapshot().save.ticket.clone().unwrap();
    let mut updates = source.subscribe();
    let batch = updates.prepare().unwrap();
    source.revoke("station-binding");
    assert!(!updates.valid(batch.id));
    assert!(source.snapshot().preview.is_none());
    assert!(source.snapshot().devices.is_empty());
    assert!(source.write_copy(&ticket, &mut vec![]).is_err());
    source.pause();
    runtime.shutdown().await.unwrap();
}
