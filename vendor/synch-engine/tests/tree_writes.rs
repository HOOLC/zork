//! Tree writes from socket programs, end to end through a real node
//! (`docs/TREE-WRITES.md`).
//!
//! Everything here runs a compiled program against the engine's own gates: a
//! commit is an ordinary local publish through the `Adoption` seam, a delete
//! is this node's tombstone, and an activated socket path is never writable.
//! The runtime's own lifecycle tests live in `synch-sock/tests/tree_writes.rs`.

#![cfg(all(
    any(target_os = "linux", target_os = "macos", target_os = "openbsd"),
    any(target_arch = "x86_64", target_arch = "aarch64")
))]

use std::path::Path;

use synch_core::{EntryKind, Hash, SockStatus};
use synch_engine::{sockets::SocketConnection, Node, NodeConfig};
use synch_store::SocketActivation;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

fn compile(source: &str, name: &str) -> Vec<u8> {
    synch_cc::compile(source, name, &[("synch.h", synch_sock::sdk::HEADER)], &[]).unwrap()
}

/// Activates a path, writes one compiled program to it, and publishes it.
async fn install(node: &Node, space_dir: &Path, path: &str, source: &str) {
    let elf = compile(source, "prog.c");
    write(space_dir, path, &elf);
    node.socket_activate(&SocketActivation::new("code", path, synch_core::now_ns()))
        .unwrap();
    node.scan_and_publish().unwrap();
}

/// One local invocation: sends `payload`, half-closes, reads the reply.
async fn drive(node: &Node, path: &str, payload: &[u8]) -> (SockStatus, Vec<u8>) {
    let connection = node
        .connect_socket(node.origin(), "code", path, Vec::new())
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
    stream.write_all(payload).await.unwrap();
    stream.shutdown().await.unwrap();
    let mut out = Vec::new();
    stream.read_to_end(&mut out).await.unwrap();
    (completion.await.unwrap(), out)
}

/// The drop-box from the design document, minus the identity trimmings:
/// splice the caller's stream into `code/inbox/drop.bin`, commit, answer with
/// the published root in hex.
const DROP: &str = r#"
#include <synch.h>

SY_MANIFEST("{\"manifest\":1,\"tree_writes\":[{\"id\":1,\"prefix\":\"code/inbox\",\"allow\":[\"create\",\"replace\"]}]}");

SY_ENTRY sy_s64 entry(void) {
  sy_s64 w = sy_put_open(1, SY_STR("code/inbox/drop.bin"));
  if (w < 0) return w;
  for (;;) {
    sy_s64 n = sy_put_splice(w, SY_SELF, 65536);
    if (n == 0) break;
    if (n == SY_EAGAIN) {
      struct sy_pollfd fds[2] = { { SY_SELF, SY_POLL_IN, 0 },
                                  { w, SY_POLL_OUT, 0 } };
      if (sy_poll(fds, 2, 10000) <= 0) return 100;
      if ((fds[0].revents | fds[1].revents) & SY_POLL_ERR) return 101;
    } else if (n < 0) return n;
  }
  sy_u8 root[32];
  sy_s64 rc;
  while ((rc = sy_put_commit(w, root)) == SY_EAGAIN) {
    struct sy_pollfd fd = { w, SY_POLL_IN, 0 };
    if (sy_poll(&fd, 1, 10000) <= 0) return 102;
  }
  if (rc < 0) return rc;
  char hex[65];
  sy_hex_encode(root, sizeof root, hex, sizeof hex, 0);
  sy_write_all(SY_SELF, hex, 64, 5000);
  sy_close(w);
  return 0;
}
"#;

#[tokio::test]
async fn a_socket_write_publishes_this_nodes_own_version() {
    let (_data, space, node) = node_with_space().await;
    install(&node, space.path(), "drop.sock", DROP).await;

    let payload = b"a file arriving over the socket fabric";
    let (status, reply) = drive(&node, "drop.sock", payload).await;
    assert_eq!(status, SockStatus::Ok(0));

    let expected = Hash::new(payload);
    assert_eq!(
        String::from_utf8(reply).unwrap(),
        expected.to_hex(),
        "the receipt names the root of exactly what the caller sent"
    );

    // Published as this node's own ordinary file version...
    let entry = node
        .store()
        .entry(node.origin(), "code", "inbox/drop.bin")
        .unwrap()
        .expect("the commit published an entry");
    assert_eq!(entry.kind, EntryKind::File);
    assert_eq!(entry.content, Some(expected));

    // ...and landed on disk in the filesystem-source directory, like an S3 PUT.
    assert_eq!(
        std::fs::read(space.path().join("inbox/drop.bin")).unwrap(),
        payload
    );
    node.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_activated_socket_path_is_never_writable() {
    const SELF_WRITE: &str = r#"
#include <synch.h>

SY_MANIFEST("{\"manifest\":1,\"tree_writes\":[{\"id\":1,\"prefix\":\"code\",\"allow\":[\"create\",\"replace\",\"delete\"]}]}");

SY_ENTRY sy_s64 entry(void) {
  /* The whole space is granted, and the socket's own path is still refused:
     writing an ELF over an activated socket path would be remote code
     persistence in two moves. */
  return sy_put_open(1, SY_STR("code/self.sock")) == SY_EPERM ? 0 : 1;
}
"#;
    let (_data, space, node) = node_with_space().await;
    install(&node, space.path(), "self.sock", SELF_WRITE).await;
    let (status, _) = drive(&node, "self.sock", b"").await;
    assert_eq!(status, SockStatus::Ok(0));
    node.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_socket_delete_publishes_this_nodes_tombstone() {
    const DELETER: &str = r#"
#include <synch.h>

SY_MANIFEST("{\"manifest\":1,\"tree_writes\":[{\"id\":1,\"prefix\":\"code/inbox\",\"allow\":[\"delete\"]}]}");

SY_ENTRY sy_s64 entry(void) {
  sy_s64 w = sy_put_open(1, SY_STR("code/inbox/gone.txt"));
  if (w < 0) return w;
  sy_s64 rc;
  while ((rc = sy_put_delete(w)) == SY_EAGAIN) {
    struct sy_pollfd fd = { w, SY_POLL_IN, 0 };
    if (sy_poll(&fd, 1, 10000) <= 0) return 100;
  }
  return rc;
}
"#;
    let (_data, space, node) = node_with_space().await;
    write(space.path(), "inbox/gone.txt", b"present for now");
    install(&node, space.path(), "reaper.sock", DELETER).await;
    assert_eq!(
        node.store()
            .entry(node.origin(), "code", "inbox/gone.txt")
            .unwrap()
            .unwrap()
            .kind,
        EntryKind::File
    );

    let (status, _) = drive(&node, "reaper.sock", b"").await;
    assert_eq!(status, SockStatus::Ok(0));

    assert_eq!(
        node.store()
            .entry(node.origin(), "code", "inbox/gone.txt")
            .unwrap()
            .expect("a tombstone is a record, not an absence")
            .kind,
        EntryKind::Tombstone
    );
    assert!(
        !space.path().join("inbox/gone.txt").exists(),
        "the local file goes with the published version"
    );
    node.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_create_only_grant_cannot_replace() {
    const CREATOR: &str = r#"
#include <synch.h>

SY_MANIFEST("{\"manifest\":1,\"tree_writes\":[{\"id\":1,\"prefix\":\"code/inbox\",\"allow\":[\"create\"]}]}");

SY_ENTRY sy_s64 entry(void) {
  sy_s64 w = sy_put_open(1, SY_STR("code/inbox/once.txt"));
  if (w < 0) return w;
  if (sy_put_write(w, "v", 1) != 1) return 100;
  sy_u8 root[32];
  sy_s64 rc;
  while ((rc = sy_put_commit(w, root)) == SY_EAGAIN) {
    struct sy_pollfd fd = { w, SY_POLL_IN, 0 };
    if (sy_poll(&fd, 1, 10000) <= 0) return 101;
  }
  return rc;
}
"#;
    let (_data, space, node) = node_with_space().await;
    install(&node, space.path(), "once.sock", CREATOR).await;

    let (first, _) = drive(&node, "once.sock", b"").await;
    assert_eq!(first, SockStatus::Ok(0), "the first commit creates");

    let (second, _) = drive(&node, "once.sock", b"").await;
    assert_eq!(
        second,
        SockStatus::Ok(-4),
        "the second finds a live version and the grant cannot replace (SY_EPERM)"
    );
    node.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_socket_writes_into_an_api_source() {
    const DETACHED: &str = r#"
#include <synch.h>

SY_MANIFEST("{\"manifest\":1,\"tree_writes\":[{\"id\":1,\"prefix\":\"archive\",\"allow\":[\"create\"]}]}");

SY_ENTRY sy_s64 entry(void) {
  sy_s64 w = sy_put_open(1, SY_STR("archive/kept.bin"));
  if (w < 0) return w;
  if (sy_put_write(w, "held with no checkout", 21) != 21) return 100;
  sy_u8 root[32];
  sy_s64 rc;
  while ((rc = sy_put_commit(w, root)) == SY_EAGAIN) {
    struct sy_pollfd fd = { w, SY_POLL_IN, 0 };
    if (sy_poll(&fd, 1, 10000) <= 0) return 101;
  }
  return rc;
}
"#;
    let (_data, space, node) = node_with_space().await;
    node.add_api_source("archive").unwrap();
    install(&node, space.path(), "archiver.sock", DETACHED).await;

    let (status, _) = drive(&node, "archiver.sock", b"").await;
    assert_eq!(status, SockStatus::Ok(0));

    let entry = node
        .store()
        .entry(node.origin(), "archive", "kept.bin")
        .unwrap()
        .expect("the API-source commit published an entry");
    assert_eq!(entry.kind, EntryKind::File);
    let root = entry.content.expect("a file version has a root");
    assert_eq!(root, Hash::new(b"held with no checkout"));
    assert!(
        node.store().blob(&root).unwrap().is_some(),
        "the bytes went straight to the CAS: an API source has no disk to hold them"
    );
    node.shutdown().await.unwrap();
}
