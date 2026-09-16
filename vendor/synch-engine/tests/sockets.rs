//! Sockets, from the engine's side (`docs/SOCKETS.md`).
//!
//! The properties here are the ones the design turns on, and most of them need
//! no eBPF runtime to check: what the scanner publishes, what an activation
//! means, and what happens to a socket that arrives from somebody else. The
//! runtime's own end-to-end tests live in `synch-sock`.

mod common;

use std::path::Path;

use synch_core::{EntryKind, Hash};
use synch_engine::{Node, NodeConfig};
use synch_store::SocketActivation;

/// A node with one filesystem source and its directory.
async fn node_with_space() -> (tempfile::TempDir, tempfile::TempDir, Node) {
    let data = tempfile::tempdir().unwrap();
    let space = tempfile::tempdir().unwrap();
    Node::init(data.path(), None).unwrap();
    let node = Node::open(NodeConfig::loopback(data.path())).await.unwrap();
    node.add_filesystem_source("code", space.path()).unwrap();
    (data, space, node)
}

fn write(space: &Path, name: &str, body: &[u8]) {
    if let Some(parent) = Path::new(name).parent() {
        std::fs::create_dir_all(space.join(parent)).unwrap();
    }
    std::fs::write(space.join(name), body).unwrap();
}

fn activation(space: &str, path: &str) -> SocketActivation {
    SocketActivation::new(space, path, synch_core::now_ns())
}

#[tokio::test]
async fn an_activated_path_is_published_as_a_socket_and_an_ordinary_one_is_not() {
    let (_data, space, node) = node_with_space().await;
    write(space.path(), "git.sock", b"\x7fELF not really");
    write(space.path(), "readme.md", b"hello");

    node.socket_activate(&activation("code", "git.sock"))
        .unwrap();
    node.scan_and_publish().unwrap();

    let socket = node
        .store()
        .entry(node.origin(), "code", "git.sock")
        .unwrap()
        .unwrap();
    assert_eq!(socket.kind, EntryKind::Socket);
    assert!(socket.content.is_some(), "a socket carries its ELF's root");

    let plain = node
        .store()
        .entry(node.origin(), "code", "readme.md")
        .unwrap()
        .unwrap();
    assert_eq!(plain.kind, EntryKind::File);
}

#[tokio::test]
async fn activating_an_unchanged_published_file_changes_its_kind() {
    let (_data, space, node) = node_with_space().await;
    write(space.path(), "git.sock", b"\x7fELF unchanged");
    node.scan_and_publish().unwrap();
    assert_eq!(
        node.store()
            .entry(node.origin(), "code", "git.sock")
            .unwrap()
            .unwrap()
            .kind,
        EntryKind::File
    );

    node.socket_activate(&activation("code", "git.sock"))
        .unwrap();
    node.scan_and_publish().unwrap();
    assert_eq!(
        node.store()
            .entry(node.origin(), "code", "git.sock")
            .unwrap()
            .unwrap()
            .kind,
        EntryKind::Socket,
        "the scanner's unchanged-file cache hid the new activation"
    );
}

#[tokio::test]
async fn deactivating_republishes_it_as_an_ordinary_file_and_blocks_resolution() {
    // The kind is an assertion this origin makes about its own copy, so
    // withdrawing the assertion has to change what it publishes — and
    // admission must refuse before the scan runs, not after.
    let (_data, space, node) = node_with_space().await;
    write(space.path(), "git.sock", b"\x7fELF");
    node.socket_activate(&activation("code", "git.sock"))
        .unwrap();
    node.scan_and_publish().unwrap();
    assert_eq!(
        node.store()
            .entry(node.origin(), "code", "git.sock")
            .unwrap()
            .unwrap()
            .kind,
        EntryKind::Socket
    );

    assert!(node.socket_deactivate("code", "git.sock").unwrap());

    // Resolution — the front of admission — refuses immediately, while the
    // stale Socket entry is still published.
    assert!(
        node.resolve_socket("code", "git.sock").unwrap().is_none(),
        "a deactivated path resolved to a runnable socket before the rescan"
    );

    node.scan_and_publish().unwrap();
    assert_eq!(
        node.store()
            .entry(node.origin(), "code", "git.sock")
            .unwrap()
            .unwrap()
            .kind,
        EntryKind::File,
        "a path with no activation must publish as a file"
    );
}

#[tokio::test]
async fn a_content_change_is_a_deployment_not_a_disarmament() {
    let (_data, space, node) = node_with_space().await;
    write(space.path(), "git.sock", b"\x7fELF v1");
    node.socket_activate(&activation("code", "git.sock"))
        .unwrap();
    node.scan_and_publish().unwrap();
    let first = node.resolve_socket("code", "git.sock").unwrap().unwrap();

    write(space.path(), "git.sock", b"\x7fELF v2, deployed");
    node.scan_and_publish().unwrap();

    let second = node.resolve_socket("code", "git.sock").unwrap().unwrap();
    assert_ne!(second.root, first.root, "the bytes changed");
    assert_eq!(
        second.activation.qualified(),
        "code/git.sock",
        "the activation stands untouched across a deployment"
    );
    assert_eq!(
        node.store()
            .entry(node.origin(), "code", "git.sock")
            .unwrap()
            .unwrap()
            .kind,
        EntryKind::Socket,
        "a deployed socket is still published as one"
    );
}

#[tokio::test]
async fn adopting_someone_elses_socket_adopts_its_bytes_and_not_its_socket_ness() {
    // The property the whole design rests on: a node executes only what is in
    // its own tree at a path it activated, and taking a peer's socket does not
    // put a socket in it. Remote content that arrives at an ordinary path is a
    // file, whatever it was to its publisher.
    let (_data, space, node) = node_with_space().await;
    write(space.path(), "theirs.sock", b"\x7fELF from a peer");
    node.scan_and_publish().unwrap();

    let entry = node
        .store()
        .entry(node.origin(), "code", "theirs.sock")
        .unwrap()
        .unwrap();
    assert_eq!(
        entry.kind,
        EntryKind::File,
        "bytes that arrived from elsewhere are a file until this node says otherwise"
    );
    assert!(
        node.resolve_socket("code", "theirs.sock")
            .unwrap()
            .is_none(),
        "a path that is not activated resolves to no socket at all"
    );
}

#[tokio::test]
async fn a_socket_cannot_be_activated_outside_a_filesystem_source() {
    let (_data, _space, node) = node_with_space().await;
    assert!(node
        .socket_activate(&activation("nowhere", "x.sock"))
        .is_err());
    assert!(
        node.socket_activate(&activation("code", "../escape.sock"))
            .is_err(),
        "a path that leaves the space must be refused at activation"
    );
}

#[tokio::test]
async fn a_socket_cannot_be_activated_in_an_api_source() {
    let data = tempfile::tempdir().unwrap();
    Node::init(data.path(), None).unwrap();
    let node = Node::open(NodeConfig::loopback(data.path())).await.unwrap();
    node.add_api_source("code").unwrap();
    let err = node.socket_activate(&activation("code", "git.sock"));
    assert!(err.is_err(), "a space with no scanner accepted a socket");
}

#[tokio::test]
async fn an_activated_path_with_nothing_published_resolves_to_nothing() {
    let (_data, _space, node) = node_with_space().await;
    node.socket_activate(&activation("code", "missing.sock"))
        .unwrap();
    assert!(node
        .resolve_socket("code", "missing.sock")
        .unwrap()
        .is_none());
}

#[cfg(all(
    any(target_os = "linux", target_os = "macos", target_os = "openbsd"),
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[tokio::test]
async fn an_invalid_update_stays_activated_but_unavailable() {
    // The bytes at the activated path are not a loadable program. The path
    // stays activated and published — deploying a fixed object is the remedy —
    // and every connection is refused with a message naming the defect.
    let (_data, space, node) = node_with_space().await;
    write(space.path(), "local.sock", b"\x7fELF but not really");
    node.socket_activate(&activation("code", "local.sock"))
        .unwrap();
    node.scan_and_publish().unwrap();

    let err = node
        .connect_socket(node.origin(), "code", "local.sock", Vec::new())
        .await
        .expect_err("an unloadable update should refuse after the self-connection lands");
    assert!(
        err.to_string().contains("program-invalid"),
        "self-connection did not reach the manifest gate: {err}"
    );
    // Still activated, still published as a socket: the defect is the
    // content's, not the path's.
    assert!(node.resolve_socket("code", "local.sock").unwrap().is_some());
}

#[cfg(all(
    any(target_os = "linux", target_os = "macos", target_os = "openbsd"),
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[tokio::test]
async fn a_self_connection_runs_the_activated_program() {
    use synch_core::SockStatus;
    use synch_engine::sockets::SocketConnection;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (_data, space, node) = node_with_space().await;
    let elf = synch_cc::compile(
        include_str!("../../synch-sock/examples/echo.c"),
        "echo.c",
        &[("synch.h", synch_sock::sdk::HEADER)],
        &[],
    )
    .unwrap();
    write(space.path(), "echo.sock", &elf);
    node.socket_activate(&activation("code", "echo.sock"))
        .unwrap();
    node.scan_and_publish().unwrap();

    let connection = node
        .connect_socket(node.origin(), "code", "echo.sock", Vec::new())
        .await
        .unwrap();
    let SocketConnection::Local {
        mut stream,
        completion,
        ..
    } = connection
    else {
        panic!("a self-connection used the remote transport");
    };
    stream.write_all(b"hello local socket").await.unwrap();
    stream.shutdown().await.unwrap();
    let mut echoed = Vec::new();
    stream.read_to_end(&mut echoed).await.unwrap();

    assert_eq!(echoed, b"hello local socket");
    assert_eq!(
        completion.await.unwrap(),
        SockStatus::Ok(echoed.len() as i64)
    );
    node.shutdown().await.unwrap();
}

/// A re-activation is a new bargain, and admission serves it: re-activating
/// with a rotated config reaches the next invocation with no content change at
/// all. The property this guards is that the operator half of the policy —
/// config and the stream cap — is read from the live activation row rather
/// than carried on any cached socket state; the admission path computes it
/// from the row it resolves under the authorization lock (`sockets.rs`), so a
/// re-activation that lands mid-admission cannot hand a stale config through.
#[cfg(all(
    any(target_os = "linux", target_os = "macos", target_os = "openbsd"),
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[tokio::test]
async fn a_reactivation_config_reaches_the_next_invocation() {
    use synch_engine::sockets::SocketConnection;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const CONF: &str = r#"
#include <synch.h>
SY_MANIFEST("{\"manifest\":1,\"name\":\"conf\",\"max_streams\":4}");
SY_ENTRY sy_s64 entry(void) {
  char v[64];
  sy_s64 n = sy_config_get(SY_STR("token"), v, sizeof v);
  if (n < 0) sy_write_all(SY_SELF, SY_STR("none"), 5000);
  else       sy_write_all(SY_SELF, v, (sy_u64)n, 5000);
  sy_shutdown(SY_SELF);
  return 0;
}
"#;
    let elf =
        synch_cc::compile(CONF, "conf.c", &[("synch.h", synch_sock::sdk::HEADER)], &[]).unwrap();

    let read_token = |node: Node| async move {
        let connection = node
            .connect_socket(node.origin(), "code", "conf.sock", Vec::new())
            .await
            .unwrap();
        let SocketConnection::Local {
            mut stream,
            completion,
            ..
        } = connection
        else {
            panic!("a self-connection used the remote transport");
        };
        stream.shutdown().await.unwrap();
        let mut out = Vec::new();
        stream.read_to_end(&mut out).await.unwrap();
        completion.await.unwrap();
        String::from_utf8(out).unwrap()
    };

    let (_data, space, node) = node_with_space().await;
    write(space.path(), "conf.sock", &elf);
    node.socket_activate(&SocketActivation {
        config: vec![("token".into(), "one".into())],
        ..activation("code", "conf.sock")
    })
    .unwrap();
    node.scan_and_publish().unwrap();
    assert_eq!(read_token(node.clone()).await, "one");

    // Re-activate with a rotated config. The content root does not change, so
    // this exercises the activation read alone, not a deployment.
    node.socket_activate(&SocketActivation {
        config: vec![("token".into(), "two".into())],
        ..activation("code", "conf.sock")
    })
    .unwrap();
    assert_eq!(
        read_token(node.clone()).await,
        "two",
        "admission handed the guest a stale config after re-activation"
    );
    node.shutdown().await.unwrap();
}

/// Deployment end to end: content written over an activated path serves on
/// the next connection, no further ceremony, and the program that answers is
/// exactly the one the tree names.
#[cfg(all(
    any(target_os = "linux", target_os = "macos", target_os = "openbsd"),
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[tokio::test]
async fn a_deployment_serves_on_the_next_connection() {
    use synch_engine::sockets::SocketConnection;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const SPEAK: &str = r#"
#include <synch.h>
SY_MANIFEST("{\"manifest\":1,\"name\":\"speak\",\"max_streams\":4}");
SY_ENTRY sy_s64 entry(void) {
  sy_write_all(SY_SELF, SY_STR(ANSWER), 5000);
  sy_shutdown(SY_SELF);
  return 0;
}
"#;
    let compile = |answer: &str| {
        synch_cc::compile(
            SPEAK,
            "speak.c",
            &[("synch.h", synch_sock::sdk::HEADER)],
            &[("ANSWER", &format!("{answer:?}"))],
        )
        .unwrap()
    };
    let drive = |node: Node| async move {
        let connection = node
            .connect_socket(node.origin(), "code", "speak.sock", Vec::new())
            .await
            .unwrap();
        let SocketConnection::Local {
            mut stream,
            completion,
            program,
            ..
        } = connection
        else {
            panic!("a self-connection used the remote transport");
        };
        stream.shutdown().await.unwrap();
        let mut out = Vec::new();
        stream.read_to_end(&mut out).await.unwrap();
        completion.await.unwrap();
        (String::from_utf8(out).unwrap(), program)
    };

    let (_data, space, node) = node_with_space().await;
    write(space.path(), "speak.sock", &compile("one"));
    node.socket_activate(&activation("code", "speak.sock"))
        .unwrap();
    node.scan_and_publish().unwrap();
    let first_root = node
        .resolve_socket("code", "speak.sock")
        .unwrap()
        .unwrap()
        .root;
    let (answer, program) = drive(node.clone()).await;
    assert_eq!(answer, "one");
    assert_eq!(program, first_root, "the reply names the snapshot that ran");

    // The deployment: new bytes over the same activated path. No activation
    // change, no approval — a scan and the next connection serves them.
    write(space.path(), "speak.sock", &compile("two"));
    node.scan_and_publish().unwrap();
    let second_root = node
        .resolve_socket("code", "speak.sock")
        .unwrap()
        .unwrap()
        .root;
    assert_ne!(second_root, first_root);
    let (answer, program) = drive(node.clone()).await;
    assert_eq!(answer, "two", "the new invocation runs the latest root");
    assert_eq!(program, second_root);
    node.shutdown().await.unwrap();
}

/// The daemon uses a multi-thread Tokio runtime, whose workers must never open
/// the synchronous SQLite store. Keep the fixture setup outside that runtime,
/// then exercise the complete async admit/run path inside it. A regression
/// here aborts in debug builds at `Store::conn`, exactly as a real daemon does.
#[cfg(all(
    any(target_os = "linux", target_os = "macos", target_os = "openbsd"),
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[test]
fn a_daemon_style_runtime_can_activate_and_run_a_self_socket() {
    use synch_core::SockStatus;
    use synch_engine::sockets::SocketConnection;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let data = tempfile::tempdir().unwrap();
    let space = tempfile::tempdir().unwrap();
    Node::init(data.path(), None).unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let node = runtime
        .block_on(Node::open(NodeConfig::loopback(data.path())))
        .unwrap();

    // These are the daemon command handler's synchronous operations; that
    // handler already offloads them. Here setup happens outside any runtime.
    node.add_filesystem_source("code", space.path()).unwrap();
    let elf = synch_cc::compile(
        include_str!("../../synch-sock/examples/echo.c"),
        "echo.c",
        &[("synch.h", synch_sock::sdk::HEADER)],
        &[],
    )
    .unwrap();
    write(space.path(), "echo.sock", &elf);
    node.socket_activate(&activation("code", "echo.sock"))
        .unwrap();
    node.scan_and_publish().unwrap();

    runtime.block_on(async {
        let connection = node
            .connect_socket(node.origin(), "code", "echo.sock", Vec::new())
            .await
            .unwrap();
        let SocketConnection::Local {
            mut stream,
            completion,
            ..
        } = connection
        else {
            panic!("a self-connection used the remote transport");
        };
        stream.write_all(b"daemon runtime").await.unwrap();
        stream.shutdown().await.unwrap();
        let mut echoed = Vec::new();
        stream.read_to_end(&mut echoed).await.unwrap();
        assert_eq!(echoed, b"daemon runtime");
        assert_eq!(
            completion.await.unwrap(),
            SockStatus::Ok(echoed.len() as i64)
        );
        node.shutdown().await.unwrap();
    });
}

#[cfg(all(
    any(target_os = "linux", target_os = "macos", target_os = "openbsd"),
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[tokio::test]
async fn a_discovery_only_peer_can_connect_to_a_socket() {
    use iroh::address_lookup::memory::MemoryLookup;
    use synch_core::SockStatus;
    use synch_engine::sockets::SocketConnection;
    use tokio::io::AsyncWriteExt;

    let (_server_data, server_space, server) = node_with_space().await;
    let (_client_data, _client_space, client) = node_with_space().await;
    let elf = synch_cc::compile(
        include_str!("../../synch-sock/examples/echo.c"),
        "echo.c",
        &[("synch.h", synch_sock::sdk::HEADER)],
        &[],
    )
    .unwrap();
    write(server_space.path(), "echo.sock", &elf);
    server
        .socket_activate(&activation("code", "echo.sock"))
        .unwrap();
    server.scan_and_publish().unwrap();

    // Trust is present in both directions, but neither node records the
    // other's address in `peers_seen`. The client's iroh resolver is the only
    // source of the server's transport address.
    client
        .store()
        .put_binding(&common::binding(server.origin(), &server.node_id()))
        .unwrap();
    server
        .store()
        .put_binding(&common::binding(client.origin(), &client.node_id()))
        .unwrap();
    assert!(client.peer_addr(&server.node_id()).unwrap().is_none());
    client
        .net()
        .endpoint()
        .address_lookup()
        .unwrap()
        .add(MemoryLookup::from_endpoint_info([server
            .net()
            .direct_addr()]));

    let connection = client
        .connect_socket(server.origin(), "code", "echo.sock", Vec::new())
        .await
        .unwrap();
    let SocketConnection::Remote {
        client: socket_client,
        mut control,
        stream,
    } = connection
    else {
        panic!("a peer connection used the local transport");
    };
    let synch_net::sock::SockStream {
        mut send, mut recv, ..
    } = stream;
    send.write_all(b"discovered").await.unwrap();
    send.shutdown().await.unwrap();
    let echoed = recv.read_to_end(1024).await.unwrap();
    let closed = socket_client.next_closed(&mut control).await.unwrap();

    assert_eq!(echoed, b"discovered");
    assert_eq!(closed.status, SockStatus::Ok(echoed.len() as i64));
    common::shutdown(&[&client, &server]).await;
}

#[tokio::test]
async fn a_dropped_node_is_actually_dropped() {
    // A regression test for a reference cycle, not a hypothetical. The node
    // owns its endpoint, the endpoint's router owns the socket protocol
    // handler, and the handler holds the dispatcher — so a strong reference
    // from the dispatcher back to the node keeps every node ever opened alive,
    // with its database open. Nothing surfaces that until something reopens the
    // same data directory, which is a long way from the cause.
    let data = tempfile::tempdir().unwrap();
    Node::init(data.path(), None).unwrap();
    let node = Node::open(NodeConfig::loopback(data.path())).await.unwrap();
    node.shutdown().await.unwrap();
    drop(node);

    // Reopening the same directory is what a leaked node makes fail.
    let again = Node::open(NodeConfig::loopback(data.path())).await.unwrap();
    assert!(again.own_head().unwrap().is_none() || again.own_head().unwrap().is_some());
    again.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_socket_that_is_not_activated_here_never_resolves_however_it_is_named() {
    let (_data, space, node) = node_with_space().await;
    write(space.path(), "a/b/c.sock", b"\x7fELF");
    node.socket_activate(&activation("code", "a/b/c.sock"))
        .unwrap();
    node.scan_and_publish().unwrap();

    assert!(node.resolve_socket("code", "a/b/c.sock").unwrap().is_some());
    // A different space with the same path is a different socket, and this
    // node activates nothing there.
    assert!(node
        .resolve_socket("other", "a/b/c.sock")
        .unwrap()
        .is_none());
    assert!(node.resolve_socket("code", "a/b").unwrap().is_none());
}

#[tokio::test]
async fn the_activation_survives_a_restart() {
    let data = tempfile::tempdir().unwrap();
    let space = tempfile::tempdir().unwrap();
    Node::init(data.path(), None).unwrap();
    let root = {
        let node = Node::open(NodeConfig::loopback(data.path())).await.unwrap();
        node.add_filesystem_source("code", space.path()).unwrap();
        write(space.path(), "git.sock", b"\x7fELF");
        node.socket_activate(&SocketActivation {
            note: "kept across restarts".into(),
            ..activation("code", "git.sock")
        })
        .unwrap();
        node.scan_and_publish().unwrap();
        let resolved = node.resolve_socket("code", "git.sock").unwrap().unwrap();
        node.shutdown().await.unwrap();
        resolved.root
    };

    let node = Node::open(NodeConfig::loopback(data.path())).await.unwrap();
    let resolved = node.resolve_socket("code", "git.sock").unwrap().unwrap();
    assert_eq!(resolved.root, root);
    assert_eq!(
        resolved.activation.note, "kept across restarts",
        "an activation must survive a restart; it is operator state, not a cache"
    );
}

#[tokio::test]
async fn resolving_ignores_what_other_origins_publish() {
    // Connecting to a socket names an origin, and resolution reads that
    // origin's trie only. `newest` would otherwise let any member's mtime
    // decide whose program a connection lands on.
    let (_data, space, node) = node_with_space().await;
    write(space.path(), "git.sock", b"\x7fELF mine");
    node.socket_activate(&activation("code", "git.sock"))
        .unwrap();
    node.scan_and_publish().unwrap();
    let mine = node.resolve_socket("code", "git.sock").unwrap().unwrap();

    // A peer publishes a different program at the same path, with a later
    // mtime — which is what would win a `newest` selection.
    let peer = synch_core::OriginId::named("nas", "cluster.example").unwrap();
    let theirs = Hash::new(b"\x7fELF theirs");
    node.store()
        .put_entry(
            &peer,
            "code",
            "git.sock",
            &synch_core::FileEntry::socket(11, i64::MAX, theirs, 1),
        )
        .unwrap();

    let after = node.resolve_socket("code", "git.sock").unwrap().unwrap();
    assert_eq!(
        after.root, mine.root,
        "resolution followed a peer's entry instead of this node's own"
    );
}
