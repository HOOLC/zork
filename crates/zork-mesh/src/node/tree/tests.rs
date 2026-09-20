use super::*;
fn query(space: &str) -> TreeQuery {
    TreeQuery {
        space: space.into(),
        path: String::new(),
        origin: None,
        search: String::new(),
        descending: false,
        after: None,
        limit: 128,
    }
}

#[tokio::test]
async fn continuous_file_changes_are_published_without_waiting_for_quiescence() -> Result<()> {
    let root = tempfile::tempdir()?;
    let config = zork_config::MeshConfig {
        enabled: true,
        offline: true,
        ..Default::default()
    };
    let mut runtime = crate::managed::start_client(root.path(), &config).await?;
    let node = runtime.node();
    node.add_api_source("files").await?;
    let engine = node.engine()?;
    let content = engine.store().ingest_bytes(b"resource", 0)?;
    let mut published = false;
    for index in 0..70 {
        let entry = synch_core::FileEntry::file(8, synch_core::now_ns(), content, 1);
        engine.stage([(
            synch_core::file_key("files", &format!("entry-{index}"))?,
            Some(synch_core::record::encode(&entry)?),
        )]);
        tokio::time::sleep(Duration::from_millis(100)).await;
        // Observe the committed first entry. A directory page can correctly
        // reject its snapshot when this test's publisher commits mid-query.
        if node
            .blocking(move |node| {
                Ok(node
                    .versions("files", "entry-0")?
                    .entries
                    .iter()
                    .any(|entry| entry.content == Some(content)))
            })
            .await?
        {
            published = true;
            break;
        }
    }
    runtime.shutdown().await?;
    ensure!(
        published,
        "ongoing file changes postponed publication beyond its maximum age"
    );
    Ok(())
}

#[tokio::test]
async fn empty_directories_and_snapshot_resources_follow_one_committed_tree() -> Result<()> {
    let root = tempfile::tempdir()?;
    let config = zork_config::MeshConfig {
        enabled: true,
        offline: true,
        ..Default::default()
    };
    let mut runtime = crate::managed::start_client(&root.path().join("mesh"), &config).await?;
    let node = runtime.node();
    let files = root.path().join("source");
    std::fs::create_dir_all(files.join("empty/nested"))?;
    std::fs::create_dir_all(files.join("guide/scripts"))?;
    std::fs::write(files.join("guide/SKILL.md"), b"v1")?;
    std::fs::write(files.join("guide/scripts/run.sh"), b"script-v1")?;
    node.add_filesystem_source("files", &files).await?;
    node.scan_source("files").await?;
    let page = node.tree_directory(query("files")).await?;
    ensure!(page
        .entries
        .iter()
        .any(|e| e.path == "empty" && e.kind == "directory"));
    let reference = node
        .tree_snapshot(zork_config::tree::Reference {
            space: "files".into(),
            path: "guide".into(),
            origin: Some(node.identity().await?),
            root: None,
            snapshot: None,
        })
        .await?;
    let page = node.tree_directory_at(reference.clone(), None, 1).await?;
    ensure!(node.tree_walk(reference.clone(), 0).await.is_err());
    ensure!(node.tree_walk(reference.clone(), 4097).await.is_err());
    std::fs::write(files.join("unrelated.txt"), b"other")?;
    node.scan_source("files").await?;
    let latest = node
        .tree_snapshot(zork_config::tree::Reference {
            snapshot: None,
            ..reference.clone()
        })
        .await?;
    ensure!(node.tree_same_directory(reference.clone(), latest).await?);

    std::fs::rename(files.join("empty"), files.join("renamed"))?;
    std::fs::write(files.join("guide/SKILL.md"), b"v2")?;
    std::fs::write(files.join("guide/scripts/run.sh"), b"script-v2")?;
    node.scan_source("files").await?;
    ensure!(
        node.tree_file(&reference.child("SKILL.md"), 0, 100)
            .await?
            .bytes
            == b"v1"
    );
    ensure!(
        node.tree_file(&reference.child("scripts/run.sh"), 0, 100)
            .await?
            .bytes
            == b"script-v1"
    );
    ensure!(!node
        .tree_directory_at(reference.clone(), page.next, 1)
        .await?
        .entries
        .is_empty());
    let page = node.tree_directory(query("files")).await?;
    ensure!(page.entries.iter().any(|e| e.path == "renamed"));
    ensure!(!page.entries.iter().any(|e| e.path == "empty"));
    std::fs::remove_dir_all(files.join("renamed"))?;
    node.scan_source("files").await?;
    ensure!(!node
        .tree_directory(query("files"))
        .await?
        .entries
        .iter()
        .any(|e| e.path == "renamed"));
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn snapshot_pages_keep_frame_bounds_with_long_names_and_resume_without_gaps() -> Result<()> {
    let root = tempfile::tempdir()?;
    let config = zork_config::MeshConfig {
        enabled: true,
        offline: true,
        ..Default::default()
    };
    let mut runtime = crate::managed::start_client(root.path(), &config).await?;
    let node = runtime.node();
    node.add_api_source("long").await?;
    for i in 0..80 {
        node.put("long", &format!("{i:03}-{}", "x".repeat(1500)), b"bytes")
            .await?;
    }
    let reference = node
        .tree_snapshot(zork_config::tree::Reference {
            space: "long".into(),
            path: String::new(),
            origin: Some(node.identity().await?),
            root: None,
            snapshot: None,
        })
        .await?;
    let mut after = None;
    let mut paths = std::collections::BTreeSet::new();
    loop {
        let page = node
            .tree_directory_at(reference.clone(), after, 128)
            .await?;
        ensure!(serde_json::to_vec(&page)?.len() < 120 * 1024);
        for entry in page.entries {
            ensure!(paths.insert(entry.path));
        }
        after = page.next;
        if after.is_none() {
            break;
        }
    }
    ensure!(paths.len() == 80);
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn snapshot_pages_order_implicit_directories_before_punctuation_siblings() -> Result<()> {
    let root = tempfile::tempdir()?;
    let config = zork_config::MeshConfig {
        enabled: true,
        offline: true,
        ..Default::default()
    };
    let mut runtime = crate::managed::start_client(root.path(), &config).await?;
    let node = runtime.node();
    node.add_api_source("files").await?;
    let paths = [
        "a/child",
        "a.txt",
        "a-/child",
        "a-.txt",
        "a-b",
        "a/x/child",
        "a/x.txt",
        "a/.hidden",
        "b/file",
        "z",
    ];
    for path in paths {
        node.put("files", path, b"resource").await?;
    }
    let reference = node
        .tree_snapshot(zork_config::tree::Reference {
            space: "files".into(),
            path: String::new(),
            origin: Some(node.identity().await?),
            root: None,
            snapshot: None,
        })
        .await?;
    for (directory, expected) in [
        ("", vec!["a", "a-", "a-.txt", "a-b", "a.txt", "b", "z"]),
        ("a", vec!["a/.hidden", "a/child", "a/x", "a/x.txt"]),
    ] {
        for limit in [1, 2, 128] {
            let mut after = None;
            let mut found = Vec::new();
            loop {
                let page = node
                    .tree_directory_at(
                        zork_config::tree::Reference {
                            path: directory.into(),
                            ..reference.clone()
                        },
                        after,
                        limit,
                    )
                    .await?;
                found.extend(page.entries.into_iter().map(|entry| entry.path));
                after = page.next;
                if after.is_none() {
                    break;
                }
            }
            assert_eq!(found, expected, "directory={directory}, page size={limit}");
        }
    }
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn snapshot_pages_match_direct_children_across_mixed_names_and_tombstones() -> Result<()> {
    let root = tempfile::tempdir()?;
    let config = zork_config::MeshConfig {
        enabled: true,
        offline: true,
        ..Default::default()
    };
    let mut runtime = crate::managed::start_client(root.path(), &config).await?;
    let node = runtime.node();
    node.add_api_source("files").await?;
    let engine = node.engine()?;
    let content = engine.store().ingest_bytes(b"x", 0)?;
    let mut changes = Vec::new();
    let mut expected = BTreeSet::new();
    for index in 0..100 {
        let mut name = String::from("x");
        let mut digits = index;
        for _ in 0..3 {
            name.push(['a', '-', '.', '!', '中'][digits % 5]);
            digits /= 5;
        }
        let path = format!(
            "folder/{name}{}",
            if index % 2 == 0 { "/child" } else { "" }
        );
        let entry = if index % 7 == 0 {
            synch_core::FileEntry::tombstone(0, 1, None)
        } else {
            expected.insert(format!("folder/{name}"));
            synch_core::FileEntry::file(1, 0, content, 1)
        };
        changes.push((
            synch_core::file_key("files", &path)?,
            Some(synch_core::record::encode(&entry)?),
        ));
    }
    engine.stage(changes);
    engine.flush_staged().await?;
    let reference = node
        .tree_snapshot(zork_config::tree::Reference {
            space: "files".into(),
            path: "folder".into(),
            origin: Some(node.identity().await?),
            root: None,
            snapshot: None,
        })
        .await?;
    let expected = expected.into_iter().collect::<Vec<_>>();
    for limit in [1, 7, 128] {
        let mut found = Vec::new();
        let mut after = None;
        loop {
            let page = node
                .tree_directory_at(reference.clone(), after, limit)
                .await?;
            found.extend(page.entries.into_iter().map(|entry| entry.path));
            after = page.next;
            if after.is_none() {
                break;
            }
        }
        assert_eq!(found, expected);
    }
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_files_and_api_paths_share_ordering_cursors_and_verified_old_ranges() -> Result<()> {
    let root = tempfile::tempdir()?;
    let config = zork_config::MeshConfig {
        enabled: true,
        offline: true,
        ..Default::default()
    };
    let mut runtime = crate::managed::start(root.path(), &config).await?;
    let node = runtime.node();
    let files = root.path().join("business");
    std::fs::create_dir_all(files.join("unexpected/child"))?;
    std::fs::write(files.join("unexpected/child/file.txt"), b"original bytes")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            files.join("unexpected/child/file.txt"),
            std::fs::Permissions::from_mode(0o755),
        )?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink("child/file.txt", files.join("unexpected/link"))?;
    node.add_filesystem_source("files", &files).await?;
    node.scan_source("files").await?;
    let mut directory = query("files");
    directory.path = "unexpected".into();
    let page = node.tree_directory(directory).await?;
    ensure!(page
        .entries
        .iter()
        .any(|e| e.path == "unexpected/child" && e.kind == "directory"));
    #[cfg(unix)]
    ensure!(page.entries.iter().any(|e| e.path == "unexpected/link"
        && e.kind == "symlink"
        && e.target.as_deref() == Some("child/file.txt")));
    let reference = zork_config::tree::Reference {
        space: "files".into(),
        path: "unexpected/child/file.txt".into(),
        origin: Some(node.identity().await?),
        root: None,
        snapshot: None,
    };
    let old = node.tree_object(&reference).await?;
    #[cfg(unix)]
    ensure!(node
        .tree_object_with_mode(&reference)
        .await?
        .1
        .is_some_and(|mode| mode & 0o111 != 0));
    node.tree_read(&old).await?;
    std::fs::write(files.join("unexpected/child/file.txt"), b"changed body")?;
    node.scan_source("files").await?;
    let pinned = zork_config::tree::Reference {
        root: Some(old.root),
        ..reference
    };
    let part = node.tree_file(&pinned, 2, 5).await?;
    assert_eq!(part.bytes, b"igina");
    assert_eq!(part.next_offset, Some(7));
    node.add_api_source("unregistered-api-space").await?;
    for path in ["a/file", "a.txt", "z.txt"] {
        node.put("unregistered-api-space", path, b"api").await?;
    }
    let mut q = query("unregistered-api-space");
    q.limit = 1;
    let first = node.tree_directory(q.clone()).await?;
    assert_eq!(first.entries[0].path, "a");
    q.after = first.next.clone();
    let second = node.tree_directory(q.clone()).await?;
    assert_eq!(second.entries[0].path, "a.txt");
    let window = node
        .tree_directory_window(query("unregistered-api-space"), 256)
        .await?;
    assert_eq!(window.entries.len(), 3);
    ensure!(window.next.is_none());
    ensure!(node
        .tree_directory_window(query("unregistered-api-space"), 8193)
        .await
        .is_err());
    node.add_api_source("unrelated").await?;
    node.put("unrelated", "new", b"unrelated").await?;
    assert_eq!(
        node.tree_directory(q.clone()).await?.revision,
        first.revision
    );
    node.put("unregistered-api-space", "b.txt", b"new").await?;
    assert!(
        node.tree_directory(q.clone()).await.is_err(),
        "cursor accepted a different directory state"
    );
    q.after = None;
    q.descending = true;
    q.search = ".txt".into();
    assert_eq!(node.tree_directory(q).await?.entries[0].path, "z.txt");
    let catalog = node.tree_catalog().await?;
    ensure!(catalog
        .spaces
        .iter()
        .any(|s| s.id == "unregistered-api-space"));
    runtime.shutdown().await?;
    Ok(())
}

#[test]
fn late_metadata_changes_revision_even_below_the_current_max_sequence() -> Result<()> {
    let connection = rusqlite::Connection::open_in_memory()?;
    connection.execute_batch("CREATE TABLE entries(origin_id TEXT,space TEXT,path TEXT,kind INTEGER,size INTEGER,mtime_ns INTEGER,unix_mode INTEGER,content BLOB,seq INTEGER,symlink_target TEXT);
        INSERT INTO entries VALUES('origin','space','a',0,1,1,NULL,X'00',100,NULL),('origin','space','b',0,1,1,NULL,X'01',5,NULL);")?;
    let allowed = BTreeSet::from(["origin".into()]);
    let q = query("space");
    let before = revision(&connection, &q, &allowed)?;
    connection.execute("UPDATE entries SET seq=7,content=X'02' WHERE path='b'", [])?;
    let after = revision(&connection, &q, &allowed)?;
    assert_ne!(before, after);
    connection.execute("UPDATE entries SET content=X'03' WHERE path='b'", [])?;
    assert_ne!(after, revision(&connection, &q, &allowed)?);
    Ok(())
}

#[tokio::test]
async fn excluded_version_history_does_not_consume_the_discovery_budget() -> Result<()> {
    let root = tempfile::tempdir()?;
    let config = zork_config::MeshConfig {
        enabled: true,
        offline: true,
        ..Default::default()
    };
    let mut runtime = crate::managed::start_client(root.path(), &config).await?;
    let node = runtime.node();
    node.add_api_source("walk").await?;
    node.put("walk", "guide/SKILL.md", b"visible").await?;
    let own = node.identity().await?;
    let origin = own.clone();
    node.blocking(move |node| {
        let mut connection = rusqlite::Connection::open(node.store().db_path())?;
        let transaction = connection.transaction()?;
        {
            let mut insert = transaction.prepare("INSERT INTO entries(origin_id,space,path,kind,size,mtime_ns,content,seq) VALUES(?1,'walk',?2,0,1,1,?3,1)")?;
            for i in 0..5000 {
                insert.execute(rusqlite::params![origin,format!(".zork/.versions/{i:05}/SKILL.md"),blake3::hash(b"x").as_bytes()])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }).await?;
    let reference = zork_config::tree::Reference {
        space: "walk".into(),
        path: String::new(),
        origin: Some(own),
        root: None,
        snapshot: None,
    };
    assert!(node.tree_walk(reference.clone(), 4096).await.is_err());
    let visible = node
        .tree_walk_excluding(reference.clone(), 1, Some(".zork".into()))
        .await?;
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].path, "guide/SKILL.md");
    let explicit = zork_config::tree::Reference {
        path: ".zork/.versions/00000".into(),
        ..reference
    };
    assert_eq!(
        node.tree_walk_excluding(explicit, 1, Some(".zork".into()))
            .await?[0]
            .path,
        ".zork/.versions/00000/SKILL.md"
    );
    // Other hidden markers still reach consumers, including archived skills.
    node.put("walk", "guide/.skill-archive/old.md", b"archived")
        .await?;
    let entries = node
        .tree_walk_excluding(
            zork_config::tree::Reference {
                space: "walk".into(),
                path: "guide".into(),
                origin: None,
                root: None,
                snapshot: None,
            },
            2,
            Some(".zork".into()),
        )
        .await?;
    assert!(entries
        .iter()
        .any(|entry| entry.path == "guide/.skill-archive/old.md"));
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn selected_origin_bounds_exclude_other_origins_subtrees() -> Result<()> {
    let root = tempfile::tempdir()?;
    let config = zork_config::MeshConfig {
        enabled: true,
        offline: true,
        ..Default::default()
    };
    let mut runtime = crate::managed::start_client(root.path(), &config).await?;
    let node = runtime.node();
    node.add_api_source("walk").await?;
    node.put("walk", "ours/file.txt", b"local").await?;
    let foreign = format!("key:{}", iroh::SecretKey::generate().public().to_z32());
    node.trust(&foreign, "metadata fixture", None).await?;
    node.blocking(move |node| {
        let connection=rusqlite::Connection::open(node.store().db_path())?;
        for path in ["earlier-a","earlier-b"] {
            connection.execute("INSERT INTO entries(origin_id,space,path,kind,size,mtime_ns,content,seq) VALUES(?1,'walk',?2,0,1,1,?3,1)",rusqlite::params![foreign,path,blake3::hash(b"x").as_bytes()])?;
        }
        Ok(())
    }).await?;
    let entries = node
        .tree_walk(
            zork_config::tree::Reference {
                space: "walk".into(),
                path: String::new(),
                origin: Some(node.identity().await?),
                root: None,
                snapshot: None,
            },
            1,
        )
        .await?;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "ours/file.txt");
    runtime.shutdown().await?;
    Ok(())
}

/// This fixture measures the metadata reader, not file ingestion or networking.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "standalone 100,000-record metadata measurement"]
async fn directory_with_100000_metadata_records_allocates_only_one_page() -> Result<()> {
    let root = tempfile::tempdir()?;
    let config = zork_config::MeshConfig {
        enabled: true,
        offline: true,
        ..Default::default()
    };
    let mut runtime = crate::managed::start_client(root.path(), &config).await?;
    let node = runtime.node();
    node.add_api_source("large").await?;
    node.blocking(|node| {
        let mut connection=rusqlite::Connection::open(node.store().db_path())?;
        let tx=connection.transaction()?;
        {
            let mut insert=tx.prepare("INSERT INTO entries(origin_id,space,path,kind,size,mtime_ns,content,seq) VALUES(?1,'large',?2,0,1,1,?3,?4)")?;
            let content=blake3::hash(b"x");let origin=node.origin().to_string();
            for i in 0..100_000 {insert.execute(rusqlite::params![origin,format!("file-{i:06}.txt"),content.as_bytes(),i+1])?;}
        }
        tx.commit()?;Ok(())
    }).await?;
    for (label, search, descending) in [
        ("ascending", "", false),
        ("descending", "", true),
        ("search", "99999", false),
    ] {
        let mut q = query("large");
        q.search = search.into();
        q.descending = descending;
        let start = std::time::Instant::now();
        let page = node.tree_directory(q).await?;
        let ms = start.elapsed().as_secs_f64() * 1000.;
        assert!(page.entries.len() <= 128);
        ensure!(serde_json::to_vec(&page)?.len() < 128 * 1024);
        if label == "search" {
            assert_eq!(page.entries[0].path, "file-099999.txt");
        }
        println!(
            "100000 metadata records {label}: {ms:.2} ms, {} returned, {} encoded bytes",
            page.entries.len(),
            serde_json::to_vec(&page)?.len()
        );
    }
    let start = std::time::Instant::now();
    let window = node.tree_directory_window(query("large"), 8192).await?;
    assert_eq!(window.entries.len(), 8192);
    assert!(window.next.is_some());
    println!(
        "100000 metadata records loaded window: {:.2} ms, {} returned, {} encoded bytes",
        start.elapsed().as_secs_f64() * 1000.,
        window.entries.len(),
        serde_json::to_vec(&window)?.len()
    );
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "standalone fixed-snapshot directory pagination measurement"]
async fn fixed_snapshot_directory_paging_cost() -> Result<()> {
    let root = tempfile::tempdir()?;
    let config = zork_config::MeshConfig {
        enabled: true,
        offline: true,
        ..Default::default()
    };
    let mut runtime = crate::managed::start_client(&root.path().join("mesh"), &config).await?;
    let node = runtime.node();
    let source = root.path().join("source");
    for (name, count) in [("small", 512), ("large", 2048)] {
        for n in 0..count {
            std::fs::create_dir_all(source.join(format!("{name}/item-{n:05}")))?;
        }
    }
    node.add_filesystem_source("files", &source).await?;
    node.scan_source("files").await?;
    let origin = node.identity().await?;
    for (name, count) in [("small", 512), ("large", 2048)] {
        let reference = node
            .tree_snapshot(zork_config::tree::Reference {
                space: "files".into(),
                path: name.into(),
                origin: Some(origin.clone()),
                root: None,
                snapshot: None,
            })
            .await?;
        let mut cursor = None;
        let mut pages = 0;
        let mut loaded = 0;
        let started = std::time::Instant::now();
        loop {
            let page = node
                .tree_directory_at(reference.clone(), cursor, 128)
                .await?;
            pages += 1;
            loaded += page.entries.len();
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(loaded, count);
        eprintln!(
            "PAGING count={count} pages={pages} elapsed_ms={:.3}",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
    runtime.shutdown().await?;
    Ok(())
}
