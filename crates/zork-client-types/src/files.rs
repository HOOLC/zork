//! Conversation-owned immutable file references. Bytes never travel in messages.
use serde::{Deserialize, Serialize};

pub const MAX_FILE_BYTES: usize = 300 * 1024 * 1024;
pub const MAX_MESSAGE_BYTES: usize = 4 * MAX_FILE_BYTES;
pub const MAX_FILES: usize = 16;
pub const CHUNK_BYTES: usize = 24 * 1024;
const PREFIX: &str = "<zork-files version=\"1\">\n";
const SUFFIX: &str = "\n</zork-files>";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRef {
    pub id: String,
    pub name: String,
    pub byte_len: usize,
    pub content_root: String,
}
impl FileRef {
    pub fn valid(&self) -> bool {
        (self.id.starts_with("file-")
            || self.id.starts_with("artifact-")
            || self.id.starts_with("mesh-"))
            && self.id.len() <= 200
            && self
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !self.name.is_empty()
            && self.name.len() <= 255
            && !matches!(self.name.as_str(), "." | "..")
            && !self
                .name
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
            && self.byte_len <= MAX_FILE_BYTES
            && self.content_root.len() == 64
            && self
                .content_root
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    }
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    text: String,
    files: Vec<FileRef>,
}

pub fn compose(text: &str, files: &[FileRef]) -> String {
    // A quoted wire envelope is ordinary authored text. Preserve that distinction
    // by wrapping it once, even when there are no actual attachments.
    if files.is_empty() && decode(text).is_none() {
        return text.to_owned();
    }
    format!(
        "{PREFIX}{}{SUFFIX}",
        serde_json::to_string(&Envelope {
            text: text.into(),
            files: files.into()
        })
        .expect("file envelope")
    )
}
pub fn decode(text: &str) -> Option<(String, Vec<FileRef>)> {
    let value: Envelope =
        serde_json::from_str(text.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?).ok()?;
    Some((value.text, value.files))
}
pub fn valid(files: &[FileRef]) -> bool {
    files.len() <= MAX_FILES
        && files.iter().all(FileRef::valid)
        && files
            .iter()
            .try_fold(0usize, |total, file| total.checked_add(file.byte_len))
            .is_some_and(|total| total <= MAX_MESSAGE_BYTES)
        && files
            .iter()
            .enumerate()
            .all(|(i, f)| !files[..i].iter().any(|other| other.id == f.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_and_message_limits_accept_300_mib_without_total_overflow() {
        let mut file = FileRef {
            id: "file-large".into(),
            name: "large.bin".into(),
            byte_len: 300 * 1024 * 1024,
            content_root: "a".repeat(64),
        };
        assert!(file.valid());
        assert!(valid(std::slice::from_ref(&file)));
        file.byte_len += 1;
        assert!(!file.valid());
        file.byte_len -= 1;
        let mut files: Vec<_> = (0..4)
            .map(|i| FileRef {
                id: format!("file-{i}"),
                ..file.clone()
            })
            .collect();
        assert!(valid(&files));
        files.push(FileRef {
            id: "file-extra".into(),
            byte_len: 1,
            ..file.clone()
        });
        assert!(!valid(&files));
        let files: Vec<_> = (0..MAX_FILES)
            .map(|i| FileRef {
                id: format!("file-{i}"),
                ..file.clone()
            })
            .collect();
        assert!(!valid(&files));
    }

    #[test]
    fn literal_file_markup_does_not_create_attachment_references() {
        let reference = FileRef {
            id: "file-example".into(),
            name: "report.txt".into(),
            byte_len: 1,
            content_root: "a".repeat(64),
        };
        let literal = compose("quoted file markup", &[reference]);
        let (text, attachments) = decode(&compose(&literal, &[])).unwrap();
        assert_eq!(text, literal);
        assert!(attachments.is_empty());
        assert_eq!(compose("ordinary text", &[]), "ordinary text");
    }
}
