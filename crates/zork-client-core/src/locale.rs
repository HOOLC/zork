//! Localized preference persistence, independent of rendering catalogs.
use std::path::{Path, PathBuf};
pub fn preferences_path() -> PathBuf {
    if let Some(path) = std::env::var_os("ZORK_GUI_PREFERENCES_PATH") {
        return path.into();
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/zork-gui/preferences.json")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("zork-gui/preferences.json")
    }
}

pub fn read_locale(path: &Path) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    value["locale"].as_str().map(str::to_owned)
}
pub fn save_locale(path: &Path, locale: &str) -> std::io::Result<()> {
    let mut value: serde_json::Value = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => return Err(e),
    };
    let object = value
        .as_object_mut()
        .ok_or_else(|| std::io::Error::other("preferences must be an object"))?;
    object.insert("locale".into(), locale.into());
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".preferences-{}.tmp", ulid::Ulid::new()));
    let result = (|| {
        std::fs::write(
            &temp,
            serde_json::to_vec_pretty(&value).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}
