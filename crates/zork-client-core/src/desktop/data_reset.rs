//! Reset only this installation's owned data, after its old process exits.
//! An external lease and journal survive the directory rename and allow a
//! failed cleanup to resume before any new store or background worker opens.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{ChildStdin, Command, Stdio},
    sync::{Arc, Mutex, OnceLock, Weak},
};

const RESET_CHILD: &str = "ZORK_DATA_RESET_CHILD";
static PARENT_PIPE: OnceLock<ChildStdin> = OnceLock::new();
static LEASES: OnceLock<Mutex<HashMap<PathBuf, Weak<Lease>>>> = OnceLock::new();

#[derive(Clone)]
struct Layout {
    client: PathBuf,
    root: PathBuf,
    ancillary_files: Vec<PathBuf>,
    lock: PathBuf,
    journal: PathBuf,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    id: String,
}

pub struct Lease {
    layout: Layout,
    _file: File,
}

fn metadata(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

impl Layout {
    fn for_client(client: &Path) -> Result<Self> {
        ensure!(client.is_absolute(), "客户端数据目录必须是绝对路径");
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let installed = home
            .as_ref()
            .map(|home| home.join("Library/Application Support/Zork/client"));
        // The installation parent can also contain independent CLI Stations.
        // Never remove that parent: own the client tree and named app files only.
        let ancillary_files = if installed.as_deref() == Some(client) {
            let parent = client.parent().context("客户端数据目录无效")?;
            vec![
                parent.join("preferences.json"),
                parent.join("logs/client.log"),
                home.as_ref()
                    .unwrap()
                    .join("Library/Application Support/zork-gui/preferences.json"),
            ]
        } else {
            Vec::new()
        };
        let root = client;
        ensure!(
            root.parent().is_some() && home.as_deref() != Some(root),
            "不能清空此目录"
        );
        ensure!(
            !root
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)),
            "客户端数据目录无效"
        );
        let parent = root.parent().context("客户端数据目录无效")?;
        fs::create_dir_all(parent)?;
        let parent = parent.canonicalize()?;
        let root = parent.join(root.file_name().context("客户端数据目录无效")?);
        ensure!(
            !std::env::current_dir()
                .ok()
                .is_some_and(|cwd| cwd.starts_with(&root)),
            "不能清空工作目录或其上级目录"
        );
        ensure!(
            !home
                .as_ref()
                .and_then(|home| home.canonicalize().ok())
                .is_some_and(|home| home.starts_with(&root)),
            "不能清空用户目录或其上级目录"
        );
        ensure!(
            std::env::temp_dir().canonicalize().ok().as_ref() != Some(&root),
            "不能清空临时目录根目录"
        );
        if let Some(meta) = metadata(&root)? {
            ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "客户端数据目录不能是符号链接"
            );
        }
        let key = zork_mesh::content_root(root.as_os_str().as_encoded_bytes());
        Ok(Self {
            client: root.clone(),
            ancillary_files,
            lock: parent.join(format!(".zork-client-{}.lock", &key[..16])),
            journal: parent.join(format!(".zork-reset-{}.json", &key[..16])),
            root,
        })
    }
    fn quarantine(&self, id: &str) -> Result<PathBuf> {
        let id: ulid::Ulid = id.parse().context("清空数据请求无效")?;
        Ok(self
            .root
            .parent()
            .unwrap()
            .join(format!(".zork-clearing-{id}")))
    }
    fn read_request(&self) -> Result<Option<Request>> {
        match fs::symlink_metadata(&self.journal) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
            Ok(meta) => ensure!(
                meta.is_file() && !meta.file_type().is_symlink() && meta.len() < 1024,
                "清空数据请求无效"
            ),
        }
        let request: Request = serde_json::from_slice(&fs::read(&self.journal)?)?;
        ensure!(request.version == 1, "不支持的清空数据请求版本");
        self.quarantine(&request.id)?;
        Ok(Some(request))
    }
    fn prepare(&self) -> Result<Request> {
        ensure!(
            self.read_request()?.is_none(),
            "已有清空数据请求，请重新打开客户端"
        );
        let request = Request {
            version: 1,
            id: ulid::Ulid::new().to_string(),
        };
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&self.journal)?;
        let saved = (|| -> Result<()> {
            file.write_all(&serde_json::to_vec(&request)?)?;
            file.sync_all()?;
            File::open(self.journal.parent().unwrap())?.sync_all()?;
            Ok(())
        })();
        if let Err(error) = saved {
            let _ = fs::remove_file(&self.journal);
            return Err(error);
        }
        Ok(request)
    }
    fn finish(&self, request: &Request) -> Result<()> {
        let quarantine = self.quarantine(&request.id)?;
        if let Some(meta) = metadata(&self.root)? {
            ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "客户端数据目录已变化"
            );
            ensure!(
                metadata(&quarantine)?.is_none(),
                "清空过程中出现新数据，已停止清理"
            );
            fs::rename(&self.root, &quarantine)?;
            File::open(self.root.parent().unwrap())?.sync_all()?;
        }
        if let Some(meta) = metadata(&quarantine)? {
            ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "清理目录已变化"
            );
            fs::remove_dir_all(&quarantine).context("未能清空全部数据，请重新打开客户端重试")?;
        }
        for path in &self.ancillary_files {
            if let Some(meta) = metadata(path)? {
                ensure!(
                    meta.is_file() || meta.file_type().is_symlink(),
                    "客户端设置文件已变化"
                );
                fs::remove_file(path).context("未能清空客户端设置，请重新打开客户端重试")?;
            }
        }
        fs::remove_file(&self.journal)?;
        File::open(self.journal.parent().unwrap())?.sync_all()?;
        Ok(())
    }
}

/// Keep this lease for the client lifetime. Multiple handles in this process
/// share it; a second process cannot race a live store or an unfinished reset.
pub fn acquire(client: &Path) -> Result<Arc<Lease>> {
    let layout = Layout::for_client(client)?;
    let mut leases = LEASES.get_or_init(Default::default).lock().unwrap();
    leases.retain(|_, lease| lease.strong_count() > 0);
    if let Some(lease) = leases.get(&layout.root).and_then(Weak::upgrade) {
        return Ok(lease);
    }
    if let Some(meta) = metadata(&layout.lock)? {
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "客户端锁文件无效"
        );
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&layout.lock)?;
    file.try_lock()
        .context("此数据目录正在被另一个 Zork 客户端使用")?;
    let lease = Arc::new(Lease {
        layout,
        _file: file,
    });
    leases.insert(lease.layout.root.clone(), Arc::downgrade(&lease));
    Ok(lease)
}

/// Native entry point: run before logs, preferences, SQLite or UI are opened.
pub fn initialize() -> Result<Arc<Lease>> {
    let ticket = std::env::var(RESET_CHILD).ok();
    if ticket.is_some() {
        std::env::remove_var(RESET_CHILD);
        // The parent keeps the writer in a static until OS process teardown,
        // so EOF also fences late saves, open SQLite handles and old callbacks.
        let mut byte = [0u8; 1];
        ensure!(
            std::io::stdin().read(&mut byte)? == 0,
            "清空数据的进程交接无效"
        );
    }
    let lease = acquire(&super::client_root())?;
    if let Some(request) = lease.layout.read_request()? {
        ensure!(
            ticket.as_ref().is_none_or(|id| id == &request.id),
            "清空数据请求已变化"
        );
        if lease.layout.client.join("node/config.json").is_file() {
            super::node::LocalNode::new(lease.layout.client.join("node")).stop()?;
        }
        lease.layout.finish(&request)?;
    } else {
        ensure!(ticket.is_none(), "清空数据请求不存在");
    }
    Ok(lease)
}

/// The old process must have stopped its local node before handing off.
pub(super) fn restart(lease: &Lease) -> Result<()> {
    ensure!(PARENT_PIPE.get().is_none(), "正在清空数据");
    let request = lease.layout.prepare()?;
    let child = Command::new(std::env::current_exe()?)
        .args(std::env::args_os().skip(1))
        .env(RESET_CHILD, &request.id)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn();
    match child {
        Ok(mut child) => {
            let writer = child.stdin.take().context("清空数据的进程交接失败")?;
            PARENT_PIPE
                .set(writer)
                .map_err(|_| anyhow::anyhow!("正在清空数据"))?;
            Ok(())
        }
        Err(error) => {
            fs::remove_file(&lease.layout.journal)?;
            Err(error).context("无法重新启动客户端")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_waits_for_process_exit_and_reopens_without_any_old_data() {
        let temp = tempfile::tempdir().unwrap();
        let client = temp.path().join("client");
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "desktop::data_reset::tests::reset_subprocess_entry",
                "--nocapture",
            ])
            .env("ZORK_CLIENT_DATA", &client)
            .env("ZORK_RESET_TEST", temp.path())
            .status()
            .unwrap();
        assert!(status.success());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let report = temp.path().join("reset-complete");
        while !report.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(fs::read_to_string(report).unwrap(), "fresh");
        assert!(!client.join("late-write").exists());
        assert!(!client.join("node").exists());
        assert!(temp.path().join("outside").is_file());
    }

    #[test]
    fn reset_subprocess_entry() {
        let Some(outer) = std::env::var_os("ZORK_RESET_TEST").map(PathBuf::from) else {
            return;
        };
        let successor = std::env::var_os(RESET_CHILD).is_some();
        let lease = initialize().unwrap();
        let root = super::super::client_root();
        if successor {
            assert!(!root.exists(), "old tree survived reset");
            let store = crate::store::ClientStore::open(&root).unwrap();
            assert!(store.nodes().unwrap().is_empty());
            assert!(store.get::<String>("device", "draft").unwrap().is_none());
            fs::write(outer.join("reset-complete"), "fresh").unwrap();
        } else {
            let store = crate::store::ClientStore::open(&root).unwrap();
            store.put("device", "draft", &"old draft").unwrap();
            fs::create_dir_all(root.join("node/profiles")).unwrap();
            fs::write(root.join("node/profiles/old.json"), "old credentials").unwrap();
            fs::write(outer.join("outside"), "keep").unwrap();
            restart(&lease).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(100));
            assert!(
                root.join("node/profiles/old.json").is_file(),
                "helper cleared a live process"
            );
            store.put("device", "draft", &"late draft").unwrap();
            fs::write(root.join("late-write"), "old callback").unwrap();
            // Simulate OS teardown with SQLite and the reset pipe still open.
            std::process::exit(0);
        }
    }

    #[test]
    fn reset_removes_the_whole_owned_tree_and_resumes_after_interruption() {
        let temp = tempfile::tempdir().unwrap();
        let client = temp.path().join("client");
        let layout = Layout::for_client(&client).unwrap();
        fs::create_dir_all(client.join("node/state")).unwrap();
        fs::write(
            client.join("node/state/station.sqlite"),
            "old source, cursor, mailbox",
        )
        .unwrap();
        fs::write(client.join("client.db"), "old cache").unwrap();
        let neighbor = temp.path().join("unrelated");
        fs::write(&neighbor, "keep").unwrap();
        let request = layout.prepare().unwrap();
        fs::rename(&client, layout.quarantine(&request.id).unwrap()).unwrap();
        layout
            .finish(&layout.read_request().unwrap().unwrap())
            .unwrap();
        assert!(!client.exists());
        assert!(!layout.journal.exists());
        assert_eq!(fs::read_to_string(neighbor).unwrap(), "keep");
        let fresh = crate::store::ClientStore::open(&client).unwrap();
        assert!(fresh.nodes().unwrap().is_empty());
    }

    #[test]
    fn missing_or_invalid_confirmation_cannot_delete_data() {
        let temp = tempfile::tempdir().unwrap();
        let client = temp.path().join("client");
        fs::create_dir(&client).unwrap();
        let layout = Layout::for_client(&client).unwrap();
        assert!(layout.read_request().unwrap().is_none());
        fs::write(&layout.journal, r#"{"version":99,"id":"../../unrelated"}"#).unwrap();
        assert!(layout.read_request().is_err());
        assert!(client.is_dir());
    }

    #[test]
    fn reset_clears_named_preferences_but_preserves_independent_stations() {
        let temp = tempfile::tempdir().unwrap();
        let client = temp.path().join("client");
        fs::create_dir(&client).unwrap();
        let station = temp.path().join("independent-station");
        fs::create_dir(&station).unwrap();
        fs::write(station.join("state.db"), "independent data").unwrap();
        let preference = temp.path().join("preferences.json");
        fs::write(&preference, "old app preferences").unwrap();
        let mut layout = Layout::for_client(&client).unwrap();
        layout.ancillary_files.push(preference.clone());
        let request = layout.prepare().unwrap();
        layout.finish(&request).unwrap();
        assert!(!client.exists());
        assert!(!preference.exists());
        assert_eq!(
            fs::read_to_string(station.join("state.db")).unwrap(),
            "independent data"
        );
    }

    #[test]
    fn live_process_lease_prevents_another_process_opening_the_same_data() {
        let temp = tempfile::tempdir().unwrap();
        let client = temp.path().join("client");
        let lease = acquire(&client).unwrap();
        assert!(Arc::ptr_eq(&lease, &acquire(&client).unwrap()));
        let other = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lease.layout.lock)
            .unwrap();
        assert!(other.try_lock().is_err());
        drop(lease);
        assert!(other.try_lock().is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn reset_never_follows_a_link_out_of_the_owned_tree() {
        let temp = tempfile::tempdir().unwrap();
        let client = temp.path().join("client");
        let outside = temp.path().join("outside");
        fs::create_dir(&client).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), "keep").unwrap();
        std::os::unix::fs::symlink(&outside, client.join("shared")).unwrap();
        let layout = Layout::for_client(&client).unwrap();
        let request = layout.prepare().unwrap();
        layout.finish(&request).unwrap();
        assert_eq!(fs::read_to_string(outside.join("keep")).unwrap(), "keep");
        std::os::unix::fs::symlink(&outside, &client).unwrap();
        assert!(Layout::for_client(&client).is_err());
    }
}
