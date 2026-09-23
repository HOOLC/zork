//! Release/dev/test identity is selected by installation metadata, never path substrings.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Dev,
    Test,
    #[default]
    Release,
}
impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Test => "test",
            Self::Release => "release",
        }
    }
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim() {
            "dev" => Ok(Self::Dev),
            "test" => Ok(Self::Test),
            "release" => Ok(Self::Release),
            _ => anyhow::bail!("invalid_build_channel"),
        }
    }
}
static HOST_CHANNEL: OnceLock<Channel> = OnceLock::new();

fn installed() -> Result<Option<Channel>> {
    let exe = std::env::current_exe()?;
    let parent = exe.parent().context("executable directory")?;
    let mut candidates = vec![parent.join("channel")];
    // Helpers live inside nested signed bundles; the outer app owns the channel.
    for ancestor in parent.ancestors().take(7) {
        if ancestor.file_name().is_some_and(|name| name == "Contents") {
            candidates.push(ancestor.join("Resources/channel"));
        }
    }
    let mut selected = None;
    for path in candidates {
        match std::fs::read_to_string(path) {
            Ok(value) => {
                let channel = Channel::parse(&value)?;
                ensure!(
                    selected.is_none_or(|previous| previous == channel),
                    "conflicting_build_channels"
                );
                selected = Some(channel);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(selected)
}

/// A platform may report its signed package channel before starting any core IO.
pub fn set_host_channel(channel: Channel) -> Result<()> {
    if let Some(installed) = installed()? {
        ensure!(installed == channel, "build_channel_mismatch");
    }
    if let Some(previous) = HOST_CHANNEL.get() {
        ensure!(*previous == channel, "build_channel_already_selected");
    } else if HOST_CHANNEL.set(channel).is_err() {
        ensure!(
            HOST_CHANNEL.get() == Some(&channel),
            "build_channel_already_selected"
        );
    }
    Ok(())
}
pub fn current() -> Result<Channel> {
    if let Some(installed) = installed()? {
        return Ok(installed);
    }
    if let Some(channel) = HOST_CHANNEL.get() {
        return Ok(*channel);
    }
    if let Ok(value) = std::env::var("ZORK_CHANNEL") {
        return Channel::parse(&value);
    }
    Ok(Channel::Release)
}
/// Unbundled service helpers adopt an explicitly selected data root's channel.
/// A signed app's channel remains authoritative.
pub fn activate_for_data(root: &Path) -> Result<Channel> {
    let recorded = recorded(root)?;
    if let Some(channel) = installed()? {
        ensure!(
            recorded.is_none_or(|value| value == channel),
            "data_channel_mismatch"
        );
        return Ok(channel);
    }
    if let Some(channel) = recorded {
        if let Some(selected) = HOST_CHANNEL.get() {
            ensure!(*selected == channel, "data_channel_mismatch");
        } else if let Ok(value) = std::env::var("ZORK_CHANNEL") {
            ensure!(Channel::parse(&value)? == channel, "data_channel_mismatch");
        }
        set_host_channel(channel)?;
        return Ok(channel);
    }
    current()
}
/// Worktree test apps carry a signed instance so Finder launches stay isolated.
pub fn test_instance() -> Result<Option<String>> {
    let exe = std::env::current_exe()?;
    for parent in exe.ancestors().take(9) {
        if parent.file_name().is_some_and(|name| name == "Contents") {
            match std::fs::read_to_string(parent.join("Resources/test-instance")) {
                Ok(value) => {
                    let value = value.trim();
                    ensure!(
                        !value.is_empty()
                            && value.len() <= 64
                            && value
                                .bytes()
                                .all(|c| c.is_ascii_alphanumeric() || c == b'-'),
                        "invalid_test_instance"
                    );
                    return Ok(Some(value.to_owned()));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(None)
}

/// The runtime owns all channel paths; clients and node discovery share this contract.
pub struct Paths {
    pub node: PathBuf,
    pub client: PathBuf,
    pub registry: PathBuf,
}
fn paths_at(home: &Path, channel: Channel, instance: Option<&str>) -> Paths {
    let suffix = match channel {
        Channel::Release => "",
        Channel::Dev => "-dev",
        Channel::Test => "-test",
    };
    let mut paths = Paths {
        node: home.join(format!(".zork{suffix}")),
        client: home.join(if channel == Channel::Release {
            "Library/Application Support/Zork/client".to_owned()
        } else {
            format!("Zork/client{suffix}")
        }),
        registry: home.join(format!(".local/state/zork{suffix}/nodes")),
    };
    if channel == Channel::Test {
        if let Some(instance) = instance {
            paths.node.push(instance);
            paths.client.push(instance);
            paths.registry.push(instance);
        }
    }
    paths
}
pub fn paths(channel: Channel) -> Paths {
    let instance = if channel == Channel::Test {
        test_instance().expect("valid test instance")
    } else {
        None
    };
    paths_at(&crate::home_dir(), channel, instance.as_deref())
}
pub fn default_root(channel: Channel) -> PathBuf {
    paths(channel).node
}
pub fn recorded(root: &Path) -> Result<Option<Channel>> {
    match std::fs::read_to_string(root.join("channel")) {
        Ok(value) => Ok(Some(Channel::parse(&value)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
/// A data root cannot be opened by a different channel after its first claim.
fn resolved(path: &Path) -> PathBuf {
    use std::path::Component;
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .expect("working directory")
            .join(path)
    };
    let mut result = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            _ => {
                result.push(component.as_os_str());
                if let Ok(canonical) = result.canonicalize() {
                    result = canonical;
                }
            }
        }
    }
    result
}

fn same_location(left: &Path, right: &Path) -> bool {
    let (left, right) = (resolved(left), resolved(right));
    left.starts_with(&right) || right.starts_with(&left)
}
pub fn validate_path(root: &Path, channel: Channel) -> Result<()> {
    let home = crate::home_dir();
    for other in [Channel::Release, Channel::Dev, Channel::Test] {
        if other == channel {
            continue;
        }
        let paths = paths_at(&home, other, None);
        let client = if other == Channel::Release {
            paths
                .client
                .parent()
                .expect("legacy client directory")
                .to_path_buf()
        } else {
            paths.client
        };
        for protected in [paths.node, client, paths.registry] {
            ensure!(!same_location(root, &protected), "data_channel_mismatch");
        }
    }
    for ancestor in resolved(root).ancestors() {
        if ancestor.is_file() {
            continue;
        }
        if let Some(previous) = recorded(ancestor)? {
            ensure!(previous == channel, "data_channel_mismatch");
        }
    }
    Ok(())
}
pub fn claim(root: &Path, channel: Channel) -> Result<()> {
    validate_path(root, channel)?;
    if let Some(previous) = recorded(root)? {
        ensure!(previous == channel, "data_channel_mismatch");
        return Ok(());
    }
    std::fs::create_dir_all(root)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(root.join("channel.lock"))?;
    fs2::FileExt::lock_exclusive(&lock)?;
    if let Some(previous) = recorded(root)? {
        ensure!(previous == channel, "data_channel_mismatch");
        return Ok(());
    }
    let temporary = root.join(format!(".channel-{}", crate::random_token()));
    let result = (|| -> Result<()> {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        writeln!(file, "{}", channel.as_str())?;
        file.sync_all()?;
        // The file lock serializes competing channel claims; readers only see
        // a complete marker. Android app storage does not permit hard links.
        std::fs::rename(&temporary, root.join("channel"))?;
        Ok(())
    })();
    let _ = std::fs::remove_file(temporary);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn channel_paths_preserve_daily_data_and_isolate_test_instances() {
        let home = Path::new("/home/example");
        assert_eq!(
            paths_at(home, Channel::Release, None).client,
            home.join("Library/Application Support/Zork/client")
        );
        assert_eq!(
            paths_at(home, Channel::Dev, None).client,
            home.join("Zork/client-dev")
        );
        assert_eq!(
            paths_at(home, Channel::Test, None).client,
            home.join("Zork/client-test")
        );
        let a = paths_at(home, Channel::Test, Some("task-a"));
        let b = paths_at(home, Channel::Test, Some("task-b"));
        assert_ne!(a.client, b.client);
        assert_ne!(a.node, b.node);
        assert_ne!(a.registry, b.registry);
    }
    #[test]
    fn all_channels_reject_each_others_data() {
        for owner in [Channel::Release, Channel::Dev, Channel::Test] {
            let root = tempfile::tempdir().unwrap();
            claim(root.path(), owner).unwrap();
            for visitor in [Channel::Release, Channel::Dev, Channel::Test] {
                assert_eq!(claim(root.path(), visitor).is_ok(), owner == visitor);
                assert_eq!(
                    claim(&root.path().join("child"), visitor).is_ok(),
                    owner == visitor
                );
            }
        }
    }
    #[test]
    fn test_cannot_claim_daily_roots_or_their_children() {
        let home = crate::home_dir();
        for root in [
            default_root(Channel::Release),
            default_root(Channel::Dev),
            home.join("Zork/client-dev"),
            home.join("Library/Application Support/Zork/client"),
        ] {
            assert!(claim(&root, Channel::Test).is_err());
            assert!(claim(&root.join("test-fixture"), Channel::Test).is_err());
        }
        assert_eq!(Channel::parse("test").unwrap(), Channel::Test);
    }
    #[test]
    fn concurrent_first_open_publishes_one_complete_channel() {
        let root = tempfile::tempdir().unwrap();
        std::thread::scope(|scope| {
            for _ in 0..16 {
                scope.spawn(|| claim(root.path(), Channel::Dev).unwrap());
            }
        });
        assert_eq!(recorded(root.path()).unwrap(), Some(Channel::Dev));
        assert!(claim(root.path(), Channel::Release).is_err());
        assert!(!std::fs::read_dir(root.path()).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".channel-")));
    }
    #[cfg(unix)]
    #[test]
    fn aliases_cannot_bypass_data_root_isolation() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        std::fs::create_dir(&data).unwrap();
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&data, &alias).unwrap();
        assert!(same_location(&data, &alias));
        assert!(same_location(&data, &alias.join("missing/../child")));
        claim(&data, Channel::Release).unwrap();
        assert!(claim(&alias, Channel::Dev).is_err());
    }
}
