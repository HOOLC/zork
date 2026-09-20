//! Transfer only Station-owned capture files into model image parts.
use serde_json::{json, Value};
use std::{
    io::Read,
    path::{Path, PathBuf},
};
use zork_agent::session::{events::ToolOutcome, tools::ToolExecution, wire::ToolImage};

fn owned_capture(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let valid_name = parent
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("zork-computer-"));
    let valid_file = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, "capture-0" | "capture-1" | "capture-2" | "capture-3"));
    valid_name
        && valid_file
        && parent.parent().and_then(|p| p.canonicalize().ok())
            == std::env::temp_dir().canonicalize().ok()
        && std::fs::symlink_metadata(parent)
            .is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink())
        && std::fs::symlink_metadata(path)
            .is_ok_and(|meta| meta.is_file() && !meta.file_type().is_symlink())
}

pub(super) fn attach_captures(result: &mut ToolExecution) {
    use base64::Engine as _;
    const LIMIT: u64 = 8 * 1024 * 1024;
    let entries = result
        .data
        .get_mut("images")
        .map(Value::take)
        .unwrap_or(Value::Null);
    let Some(entries) = entries.as_array() else {
        return;
    };
    let mut failed = entries.len() > 4;
    let mut attached = 0;
    for entry in entries {
        let path = PathBuf::from(entry["path"].as_str().unwrap_or(""));
        if !owned_capture(&path) {
            failed = true;
            continue;
        }
        let mime = entry["mime_type"].as_str().unwrap_or("");
        let mut bytes = Vec::new();
        let read = std::fs::File::open(&path)
            .and_then(|file| file.take(LIMIT + 1).read_to_end(&mut bytes));
        // Even a rejected/oversized image must not leave a desktop capture on disk.
        let removed = std::fs::remove_file(&path).is_ok();
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
        let magic = (mime == "image/png" && bytes.starts_with(b"\x89PNG\r\n\x1a\n"))
            || (mime == "image/jpeg" && bytes.starts_with(b"\xff\xd8\xff"));
        if read.is_err() || !removed || bytes.len() as u64 > LIMIT || !magic || attached >= 4 {
            failed = true;
            continue;
        }
        result.images.push(ToolImage {
            media_type: mime.into(),
            base64: base64::engine::general_purpose::STANDARD
                .encode(bytes)
                .into(),
        });
        attached += 1;
    }
    result.data["images"] = json!({"count":attached});
    if failed {
        result.outcome = ToolOutcome::Failed;
        result.data["state"] = json!("failed");
        result.data["capture_error"] = json!("computer_capture_transfer_failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn capture(bytes: &[u8]) -> (PathBuf, Value) {
        let dir = std::env::temp_dir().join(format!("zork-computer-{}", ulid::Ulid::new()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("capture-0");
        std::fs::write(&path, bytes).unwrap();
        (path.clone(), json!({"path":path, "mime_type":"image/png"}))
    }
    #[test]
    fn image_attached_and_file_removed() {
        let (path, image) = capture(b"\x89PNG\r\n\x1a\nfixture");
        let mut result = ToolExecution::success(json!({"images":[image]}));
        attach_captures(&mut result);
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.data["images"]["count"], 1);
        assert!(!path.exists());
        assert!(!path.parent().unwrap().exists());
        assert!(!result.data.to_string().contains("zork-computer-"));
    }
    #[test]
    fn oversized_image_fails_and_is_cleaned() {
        let (path, image) = capture(b"\x89PNG\r\n\x1a\n");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(8 * 1024 * 1024 + 1)
            .unwrap();
        let mut result = ToolExecution::success(json!({"images":[image]}));
        attach_captures(&mut result);
        assert_eq!(result.outcome, ToolOutcome::Failed);
        assert!(result.images.is_empty());
        assert!(!path.exists());
    }
    #[test]
    fn arbitrary_files_are_never_read_or_deleted() {
        let path = std::env::temp_dir().join(format!("unrelated-{}", ulid::Ulid::new()));
        std::fs::write(&path, b"private").unwrap();
        let mut result =
            ToolExecution::success(json!({"images":[{"path":path,"mime_type":"image/png"}]}));
        attach_captures(&mut result);
        assert!(result.images.is_empty());
        assert_eq!(result.outcome, ToolOutcome::Failed);
        assert_eq!(std::fs::read(&path).unwrap(), b"private");
        std::fs::remove_file(path).unwrap();
    }
}
