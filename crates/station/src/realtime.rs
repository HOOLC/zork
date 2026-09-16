//! Commit-driven desktop invalidations. Transport reads do not invalidate state.
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::sync::watch;

pub const SESSIONS: u64 = 1;
pub const TASKS: u64 = 2;
pub const AGENTS: u64 = 4;
pub const ARTIFACTS: u64 = 8;
pub const PROFILES: u64 = 16;
pub const MESH: u64 = 32;
pub const ACTIVITY: u64 = 64;
pub const WORK: u64 = 128;
pub const SYNC: u64 = 256;
pub const NODE: u64 = 512;

#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    pub sessions: u64,
    pub tasks: u64,
    pub agents: u64,
    pub artifacts: u64,
    pub profiles: u64,
    pub mesh: u64,
    pub activity: u64,
    pub work: u64,
    pub sync: u64,
}
#[derive(Clone)]
pub struct Realtime {
    epoch: String,
    revisions: watch::Sender<Revision>,
    topics: zork_notify::Hub<u64>,
}
impl Default for Realtime {
    fn default() -> Self {
        Self {
            epoch: ulid::Ulid::new().to_string(),
            revisions: watch::channel(Revision::default()).0,
            topics: Default::default(),
        }
    }
}
impl Realtime {
    pub fn subscribe(&self) -> watch::Receiver<Revision> {
        self.revisions.subscribe()
    }
    /// Opaque invalidation token; equality only, never a durable replay cursor.
    pub fn token(&self, revision: u64) -> String {
        format!("{}:{revision}", self.epoch)
    }
    pub fn current(&self) -> Revision {
        self.revisions.borrow().clone()
    }
    /// Filter before waking business readers; process-local revisions are not
    /// durable sync cursors and never authorize access to a topic.
    pub fn listen(&self, flags: u64) -> zork_notify::Changes {
        self.topics.subscribe(Self::topics(flags))
    }
    fn topics(flags: u64) -> impl Iterator<Item = u64> {
        (0..64)
            .map(|bit| 1u64 << bit)
            .filter(move |bit| flags & bit != 0)
    }
    pub fn notify(&self, flags: u64) {
        if flags == 0 {
            return;
        }
        self.revisions.send_modify(|v| {
            for (flag, field) in [
                (SESSIONS, &mut v.sessions),
                (TASKS, &mut v.tasks),
                (AGENTS, &mut v.agents),
                (ARTIFACTS, &mut v.artifacts),
                (PROFILES, &mut v.profiles),
                (MESH, &mut v.mesh),
                (ACTIVITY, &mut v.activity),
                (WORK, &mut v.work),
                (SYNC, &mut v.sync),
            ] {
                if flags & flag != 0 {
                    *field = field.wrapping_add(1)
                }
            }
        });
        self.topics.publish(Self::topics(flags));
    }
    pub fn install(&self, conn: &rusqlite::Connection) {
        let pending = Arc::new(AtomicU64::new(0));
        let writes = pending.clone();
        conn.update_hook(Some(
            move |_: rusqlite::hooks::Action, _: &str, table: &str, _: i64| {
                let flags = match table {
                    // Channel subscriptions signal exact channel/recipient topics
                    // after commit. Cursor and receipt writes never wake workers.
                    table if table.starts_with("chat_") => return,
                    // SQLite update hooks do not identify changed columns. The
                    // transactional marker excludes retention-only meta writes.
                    "sync_change_signal" => SYNC,
                    table if table.starts_with("sync_") => return,
                    // Snapshot cursor advancement alone is bookkeeping. A run
                    // count change refreshes product_tasks in the same transaction.
                    "task_event_cursors" => return,
                    "conversation_uploads"
                    | "conversation_upload_chunks"
                    | "conversation_file_origins" => return,
                    "sessions" | "visible_messages" => SESSIONS,
                    "node_agents" => AGENTS | SESSIONS,
                    "product_tasks" | "task_runs" => TASKS | SESSIONS,
                    "task_file_snapshots" | "conversation_file_snapshots" => ARTIFACTS,
                    _ => 0,
                };
                writes.fetch_or(
                    if flags == SYNC { SYNC } else { flags | WORK },
                    Ordering::Relaxed,
                );
            },
        ));
        let commits = pending.clone();
        let events = self.clone();
        conn.commit_hook(Some(move || {
            events.notify(commits.swap(0, Ordering::Relaxed));
            false
        }));
        conn.rollback_hook(Some(move || {
            pending.store(0, Ordering::Relaxed);
        }));
    }
}

/// File-backed authorities use the same change source on macOS, Linux,
/// Android and Windows. The caller owns the watch for the node's lifetime.
pub fn watch_config(
    root: std::path::PathBuf,
    events: Realtime,
) -> anyhow::Result<zork_notify::files::FileWatch> {
    type Fingerprint = (
        Vec<u8>,
        std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
        Option<Vec<u8>>,
    );
    fn snapshot(root: &std::path::Path) -> std::io::Result<Fingerprint> {
        let config = std::fs::read(root.join("config.json"))?;
        let mut profiles = std::collections::BTreeMap::new();
        match std::fs::read_dir(root.join("profiles")) {
            Ok(files) => {
                for file in files {
                    let file = file?;
                    if file.path().extension().is_some_and(|e| e == "json") {
                        profiles.insert(file.path(), std::fs::read(file.path())?);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let update = match std::fs::read(root.join("run/update.json")) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        Ok((config, profiles, update))
    }
    let selected = root.clone();
    let data = root.clone();
    let notify = events.clone();
    let mut previous = snapshot(&root).ok();
    let watch = zork_notify::files::watch_paths(
        vec![
            (root.clone(), false),
            (root.join("profiles"), false),
            (root.join("run"), false),
        ],
        move |path| {
            path == selected.join("config.json")
                || path.starts_with(selected.join("profiles"))
                || path == selected.join("run/update.json")
        },
        move |rescan| {
            let Ok(current) = snapshot(&data) else {
                // Preserve the last good fingerprint; the authority reader owns
                // bounded failure retries and never replaces data with empty state.
                notify.notify(MESH | PROFILES | NODE);
                return;
            };
            let mut flags = 0;
            if rescan
                || previous
                    .as_ref()
                    .is_none_or(|old: &Fingerprint| old.0 != current.0)
            {
                flags |= MESH | PROFILES | SESSIONS | WORK;
            }
            if previous.as_ref().is_none_or(|old| old.1 != current.1) {
                flags |= PROFILES | SESSIONS | WORK;
            }
            if rescan || previous.as_ref().is_none_or(|old| old.2 != current.2) {
                flags |= NODE;
            }
            previous = Some(current);
            notify.notify(flags);
        },
    )?;
    // Registration precedes this invalidation, including startup races with
    // the first catalog read and edits made by external processes.
    events.notify(MESH | PROFILES | SESSIONS | WORK | NODE);
    Ok(watch)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn native_atomic_config_replacement_and_update_progress_publish_without_polling() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("config.json"), b"{}").unwrap();
        std::fs::create_dir(root.path().join("run")).unwrap();
        let events = Realtime::default();
        let watch = watch_config(root.path().to_owned(), events.clone()).unwrap();
        let mut config = events.listen(MESH);
        let mut progress = events.listen(NODE);
        std::fs::write(
            root.path().join("config.tmp"),
            br#"{"mesh":{"name":"After"}}"#,
        )
        .unwrap();
        std::fs::rename(
            root.path().join("config.tmp"),
            root.path().join("config.json"),
        )
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), config.changed())
            .await
            .unwrap()
            .unwrap();
        zork_config::update::write_state(root.path(), "complete", "1.2.3", "done").unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), progress.changed())
            .await
            .unwrap()
            .unwrap();
        // Allow the kernel's atomic-replacement event batch to finish first.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        config.checkpoint();
        for _ in 0..10 {
            std::fs::read(root.path().join("config.json")).unwrap();
        }
        std::fs::write(
            root.path().join("config.json"),
            br#"{"mesh":{"name":"After"}}"#,
        )
        .unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(600), config.changed())
                .await
                .is_err()
        );
        drop(watch);
    }

    #[tokio::test]
    async fn replaced_profile_directory_is_rearmed_and_watch_can_close_in_callback() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("config.json"), b"{}").unwrap();
        let events = Realtime::default();
        let watch = watch_config(root.path().to_owned(), events.clone()).unwrap();
        let mut changes = events.listen(PROFILES);
        std::fs::create_dir(root.path().join("profiles")).unwrap();
        std::fs::write(root.path().join("profiles/a.json"), b"{}").unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), changes.changed())
            .await
            .unwrap()
            .unwrap();
        std::fs::rename(
            root.path().join("profiles"),
            root.path().join("old-profiles"),
        )
        .unwrap();
        std::fs::create_dir(root.path().join("profiles")).unwrap();
        std::fs::write(root.path().join("profiles/b.json"), b"{}").unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), changes.changed())
            .await
            .unwrap()
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        changes.checkpoint();
        std::fs::write(root.path().join("profiles/b.json"), br#"{"changed":true}"#).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), changes.changed())
            .await
            .unwrap()
            .unwrap();
        drop(watch);

        let owner = Arc::new(std::sync::Mutex::new(None::<zork_notify::files::FileWatch>));
        let weak = Arc::downgrade(&owner);
        let (closed, done) = tokio::sync::oneshot::channel();
        let mut closed = Some(closed);
        let watch = zork_notify::files::watch(
            root.path(),
            |_| true,
            move |_| {
                if let Some(owner) = weak.upgrade() {
                    if let Some(watch) = owner.lock().unwrap().take() {
                        drop(watch);
                        let _ = closed.take().unwrap().send(());
                    }
                }
            },
        )
        .unwrap();
        *owner.lock().unwrap() = Some(watch);
        std::fs::write(root.path().join("close"), b"close").unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), done)
            .await
            .unwrap()
            .unwrap();
    }
    #[test]
    fn only_committed_changes_invalidate_subscribers() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE product_tasks(id INTEGER PRIMARY KEY)")
            .unwrap();
        let events = Realtime::default();
        events.install(&conn);
        let mut rx = events.subscribe();
        conn.execute_batch("BEGIN; INSERT INTO product_tasks VALUES(1); ROLLBACK")
            .unwrap();
        assert!(!rx.has_changed().unwrap());
        conn.execute("INSERT INTO product_tasks VALUES(1)", [])
            .unwrap();
        assert!(rx.has_changed().unwrap());
        assert_eq!(rx.borrow_and_update().tasks, 1);
        conn.query_row("SELECT COUNT(*) FROM product_tasks", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap();
        conn.execute("INSERT OR IGNORE INTO product_tasks VALUES(1)", [])
            .unwrap();
        assert!(!rx.has_changed().unwrap());
    }
}
