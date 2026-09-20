//! File snapshot export; destinations come from a platform file picker.
pub fn default_destination() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .map(|root| root.join("Downloads"))
        .unwrap_or_default()
}
pub fn save_snapshot(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("missing destination directory"))?;
    let temporary = parent.join(format!(".zork-save-{}", ulid::Ulid::new()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

/// Shared bounded-preview classification. Platforms only decode the selected format.
pub fn preview_mime(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "txt" | "md" | "json" | "jsonl" | "ndjson" | "toml" | "yaml" | "yml" | "rs" | "kt"
        | "java" | "js" | "ts" | "tsx" | "jsx" | "py" | "sh" | "log" | "css" | "html" | "svg"
        | "xml" | "csv" => "text/plain",
        _ => "application/octet-stream",
    }
}
