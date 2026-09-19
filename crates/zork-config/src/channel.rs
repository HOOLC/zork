//! Release/dev identity is selected by installation metadata, never path substrings.
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
    #[default]
    Release,
}
impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Release => "release",
        }
    }
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim() {
            "dev" => Ok(Self::Dev),
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
pub fn default_root(channel: Channel) -> PathBuf {
    crate::home_dir().join(if channel == Channel::Dev {
        ".zork-dev"
    } else {
        ".zork"
    })
}
pub fn recorded(root: &Path) -> Result<Option<Channel>> {
    match std::fs::read_to_string(root.join("channel")) {
        Ok(value) => Ok(Some(Channel::parse(&value)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
/// A data root cannot be opened by a different channel after its first claim.
fn same_location(left: &Path, right: &Path) -> bool {
    left == right
        || left
            .canonicalize()
            .ok()
            .zip(right.canonicalize().ok())
            .is_some_and(|(a, b)| a == b)
}
pub fn claim(root: &Path, channel: Channel) -> Result<()> {
    ensure!(
        channel != Channel::Dev || !same_location(root, &default_root(Channel::Release)),
        "development_must_not_open_release_data"
    );
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
        claim(&data, Channel::Release).unwrap();
        assert!(claim(&alias, Channel::Dev).is_err());
    }
}
