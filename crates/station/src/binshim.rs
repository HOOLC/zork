use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::RuntimeConfig;

pub fn install(config: &mut RuntimeConfig) -> Result<()> {
    fs::create_dir_all(&config.zork_bin_dir).context("create zork bin dir")?;
    let Ok(zork_gh_src) = resolve_named_bin("zork-gh", config.zork_gh_path.as_deref()) else {
        return Ok(());
    };
    let gh_dest = config.zork_bin_dir.join("gh");
    copy_bin(&zork_gh_src, &gh_dest)?;
    if config.real_gh_path.is_none() {
        config.real_gh_path = find_real_gh(&config.zork_bin_dir);
    }
    if let Some(path) = &config.real_gh_path {
        env::set_var("BROKER_REAL_GH_PATH", path);
    }
    Ok(())
}

pub(crate) fn resolve_named_bin(name: &str, explicit: Option<&Path>) -> Result<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = explicit {
        candidates.push(path.to_path_buf());
    }
    if let Ok(current) = env::current_exe() {
        if let Some(dir) = current.parent() {
            candidates.push(dir.join(name));
        }
    }
    candidates.push(PathBuf::from("/usr/local/bin").join(name));
    candidates.push(PathBuf::from("/data/bin").join(name));
    if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path) {
            candidates.push(dir.join(name));
        }
    }
    for candidate in candidates {
        if is_usable_bin(&candidate) {
            return Ok(candidate);
        }
    }
    anyhow::bail!("{name} not found")
}

fn copy_bin(src: &Path, dest: &Path) -> Result<()> {
    if same_path(src, dest) {
        return Ok(());
    }
    fs::copy(src, dest).with_context(|| format!("copy {} -> {}", src.display(), dest.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dest, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn find_real_gh(skip_dir: &Path) -> Option<PathBuf> {
    let skip = skip_dir.canonicalize().ok();
    let path_value = env::var_os("PATH")?;
    for dir in env::split_paths(&path_value) {
        if skip
            .as_ref()
            .zip(dir.canonicalize().ok().as_ref())
            .is_some_and(|(skip, current)| skip == current)
        {
            continue;
        }
        let candidate = dir.join("gh");
        if is_usable_bin(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_usable_bin(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    if matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("js" | "ts" | "mjs" | "cjs")
    ) {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}
