//! Transfer bounded in-memory desktop captures into model image parts.
use base64::Engine as _;
use serde_json::{json, Value};
use zork_agent::session::{events::ToolOutcome, tools::ToolExecution, wire::ToolImage};

// Four 8 MiB images after base64 expansion, plus the bounded textual result.
// Keep the same wire budget as Station's native desktop IPC response.
pub(super) const MAX_RESPONSE_BYTES: usize = 48 * 1024 * 1024;
const IMAGE_LIMIT: usize = 8 * 1024 * 1024;

fn image(mut entry: Value) -> Option<ToolImage> {
    let mime = entry["mime_type"].as_str()?.to_owned();
    if !matches!(mime.as_str(), "image/png" | "image/jpeg") {
        return None;
    }
    let Value::String(encoded) = entry["base64"].take() else {
        return None;
    };
    if encoded.len() > 12 * 1024 * 1024 {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&encoded)
        .ok()?;
    let magic = (mime == "image/png" && bytes.starts_with(b"\x89PNG\r\n\x1a\n"))
        || (mime == "image/jpeg" && bytes.starts_with(b"\xff\xd8\xff"));
    if bytes.len() > IMAGE_LIMIT || !magic {
        return None;
    }
    Some(ToolImage {
        media_type: mime,
        base64: encoded.into(),
    })
}

pub(super) fn attach_captures(result: &mut ToolExecution) {
    // Remove payloads even on rejection; they must never leak into model text.
    let Some(entries) = result
        .data
        .as_object_mut()
        .and_then(|data| data.remove("images"))
    else {
        return;
    };
    let mut failed = false;
    match entries {
        Value::Array(entries) if entries.len() <= 4 => {
            for entry in entries {
                match image(entry) {
                    Some(image) => result.images.push(image),
                    None => failed = true,
                }
            }
        }
        _ => failed = true,
    }
    if failed {
        result.outcome = ToolOutcome::Failed;
        result.data["state"] = json!("failed");
        result.data["capture_error"] = json!("computer_capture_transfer_failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn capture(bytes: &[u8]) -> Value {
        json!({"mime_type":"image/png", "base64":base64::engine::general_purpose::STANDARD.encode(bytes)})
    }
    #[test]
    fn images_are_removed_from_text_without_resetting_tool_failure() {
        let entry = capture(b"\x89PNG\r\n\x1a\nfixture");
        let mut result = ToolExecution::success(json!({"state":"failed","images":[entry.clone()]}));
        result.outcome = ToolOutcome::Failed;
        attach_captures(&mut result);
        assert_eq!(result.images.len(), 1);
        assert_eq!(
            result.images[0].base64.as_ref(),
            entry["base64"].as_str().unwrap()
        );
        assert_eq!(result.outcome, ToolOutcome::Failed);
        assert!(result.data.get("images").is_none());
        assert!(!result.data.to_string().contains("base64"));
    }
    #[test]
    fn size_boundary_and_count_are_enforced() {
        let mut bytes = vec![0; IMAGE_LIMIT];
        bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        assert!(image(capture(&bytes)).is_some());
        bytes.push(0);
        assert!(image(capture(&bytes)).is_none());
        let entry = capture(b"\x89PNG\r\n\x1a\nfixture");
        for count in [4, 5] {
            let mut result = ToolExecution::success(json!({"images":vec![entry.clone();count]}));
            attach_captures(&mut result);
            assert_eq!(result.images.len(), if count == 4 { 4 } else { 0 });
            assert_eq!(result.outcome == ToolOutcome::Failed, count == 5);
            assert!(result.data.get("images").is_none());
        }
    }
    #[test]
    fn malformed_images_fail_without_retaining_payloads_or_using_paths() {
        for entry in [
            json!({"path":"/private/unused","mime_type":"image/png"}),
            json!({"base64":"invalid!","mime_type":"image/png"}),
            json!({"base64":"eA==","mime_type":"text/plain"}),
            capture(b"not a PNG"),
        ] {
            let mut result = ToolExecution::success(json!({"images":[entry]}));
            attach_captures(&mut result);
            assert_eq!(result.outcome, ToolOutcome::Failed);
            assert!(result.images.is_empty());
            assert!(result.data.get("images").is_none());
        }
    }
}
