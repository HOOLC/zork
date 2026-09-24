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

/// Bounded size for inline image thumbnails and previews.
pub const PREVIEW_BYTES: usize = 8 * 1024 * 1024;

/// Platform-neutral presentation of an immutable file reference. Clients show
/// these values instead of re-deriving type badges or sizes from names.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct FileView {
    pub id: String,
    pub name: String,
    pub byte_len: usize,
    pub mime: &'static str,
    /// `image`, `text` or `file`.
    pub kind: &'static str,
    pub badge: String,
    pub size: String,
    /// Small enough to decode inline as a thumbnail.
    pub thumbnail: bool,
}

pub fn view(file: &crate::files::FileRef) -> FileView {
    let mime = preview_mime(&file.name);
    let kind = if mime.starts_with("image/") {
        "image"
    } else if mime.starts_with("text/") {
        "text"
    } else {
        "file"
    };
    let badge = file
        .name
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .filter(|ext| {
            !ext.is_empty() && ext.len() <= 4 && ext.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .map(str::to_ascii_uppercase)
        .unwrap_or_else(|| "FILE".into());
    FileView {
        id: file.id.clone(),
        name: file.name.clone(),
        byte_len: file.byte_len,
        mime,
        kind,
        badge,
        size: byte_size(file.byte_len),
        thumbnail: kind == "image" && file.byte_len <= PREVIEW_BYTES,
    }
}

pub fn views(files: &[crate::files::FileRef]) -> Vec<FileView> {
    files.iter().map(view).collect()
}

pub fn byte_size(bytes: usize) -> String {
    const UNITS: [&str; 3] = ["KB", "MB", "GB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.;
    let mut unit = 0;
    while value >= 1024. && unit + 1 < UNITS.len() {
        value /= 1024.;
        unit += 1;
    }
    if value >= 10. {
        format!("{:.0} {}", value, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file(name: &str, byte_len: usize) -> crate::files::FileRef {
        crate::files::FileRef {
            id: "file-a".into(),
            name: name.into(),
            byte_len,
            content_root: "0".repeat(64),
        }
    }

    #[test]
    fn views_classify_badge_size_and_thumbnail() {
        let image = view(&file("首页草图.PNG", 1_258_291));
        assert_eq!(
            (
                image.kind,
                image.mime,
                image.badge.as_str(),
                image.size.as_str()
            ),
            ("image", "image/png", "PNG", "1.2 MB")
        );
        assert!(image.thumbnail);
        assert!(!view(&file("huge.jpg", PREVIEW_BYTES + 1)).thumbnail);
        let doc = view(&file("report.pdf", 14 * 1024));
        assert_eq!(
            (
                doc.kind,
                doc.badge.as_str(),
                doc.size.as_str(),
                doc.thumbnail
            ),
            ("file", "PDF", "14 KB", false)
        );
        assert_eq!(view(&file("notes.md", 12)).kind, "text");
        assert_eq!(view(&file("notes.md", 12)).size, "12 B");
        assert_eq!(view(&file("README", 12)).badge, "FILE");
        assert_eq!(view(&file("archive.backup", 12)).badge, "FILE");
        assert_eq!(byte_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }
}
