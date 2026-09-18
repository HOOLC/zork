use super::*;
use crate::db::{
    chats::{ChatRow, PreparedFile},
    StationDb,
};
use api::{FileRequest, Rpc};
use base64::Engine;
use std::path::{Path, PathBuf};
use zork_client_types::files::{self, FileRef};

pub(super) async fn prepare(
    state: &AppState,
    who: &Subject,
    target: &str,
    args: &Value,
) -> Result<Vec<PreparedFile>> {
    let binding = state
        .db
        .get_binding_by_id(&who.session)?
        .context("node_unknown_session")?;
    let workspace = Path::new(binding.workspace_path());
    let mut result = Vec::new();
    for attachment in args["attachments"].as_array().into_iter().flatten() {
        let mut file = if let Some(path) = attachment["file_path"].as_str() {
            StationDb::prepare_chat_file(workspace, Path::new(path))?
        } else if attachment["source_target"]
            .as_str()
            .is_some_and(|source| !local(state, source))
        {
            let source = field(attachment, "source_target")?;
            let chat = field(attachment, "source_chat_id")?;
            let id = field(attachment, "attachment_id")?;
            let (mut reference, media_type, content) = fetch(
                state,
                source,
                |offset| FileRequest::Published {
                    chat_id: chat.into(),
                    attachment_id: id.into(),
                    offset,
                },
                None,
            )
            .await?;
            reference.id = format!("file-{}", ulid::Ulid::new());
            PreparedFile {
                reference,
                media_type,
                content,
                source_path: format!("chat:{source}/{chat}/{id}"),
                workspace: workspace.to_string_lossy().into_owned(),
            }
        } else {
            state.db.copy_chat_file(
                field(attachment, "source_chat_id")?,
                field(args, "chat_id")?,
                field(attachment, "attachment_id")?,
            )?
        };
        if !local(state, target) {
            file.reference.id = format!("file-{}", ulid::Ulid::new());
        }
        ensure!(
            result.len() < files::MAX_FILES
                && result
                    .iter()
                    .map(|file: &PreparedFile| file.content.len())
                    .sum::<usize>()
                    + file.content.len()
                    <= files::MAX_MESSAGE_BYTES,
            "invalid_attachments"
        );
        result.push(file);
    }
    ensure!(
        files::valid(
            &result
                .iter()
                .map(|f| f.reference.clone())
                .collect::<Vec<_>>()
        ),
        "invalid_attachments"
    );
    Ok(result)
}

pub(super) fn chunk(state: &AppState, peer: &str, request: FileRequest) -> Result<Value> {
    match request {
        FileRequest::Prepared {
            key,
            attachment_id,
            offset,
        } => state
            .db
            .prepared_chat_chunk(&key, peer, &attachment_id, offset),
        FileRequest::Published {
            chat_id,
            attachment_id,
            offset,
        } => state
            .db
            .published_chat_chunk(&chat_id, &attachment_id, offset),
    }
}

async fn fetch(
    state: &AppState,
    origin: &str,
    make: impl Fn(usize) -> FileRequest,
    expected: Option<&FileRef>,
) -> Result<(FileRef, String, Vec<u8>)> {
    let mut bytes = Vec::new();
    let mut reference = None;
    let mut media = String::new();
    loop {
        let offset = bytes.len();
        let value = if local(state, origin) {
            chunk(state, "local", make(offset))?
        } else {
            access(state, origin)?;
            state
                .mesh
                .get()
                .context("node_starting")?
                .channel_file(origin, make(offset))
                .await?
        };
        let file: FileRef = serde_json::from_value(value["reference"].clone())?;
        ensure!(
            file.valid()
                && expected.is_none_or(|expected| expected == &file)
                && reference.as_ref().is_none_or(|saved| saved == &file),
            "attachment_reference_mismatch"
        );
        ensure!(
            value["offset"].as_u64() == Some(offset as u64),
            "invalid_attachment_offset"
        );
        let data = base64::engine::general_purpose::STANDARD.decode(
            value["base64"]
                .as_str()
                .context("invalid_attachment_chunk")?,
        )?;
        ensure!(
            data.len() <= files::CHUNK_BYTES
                && offset + data.len() <= file.byte_len
                && value["next_offset"].as_u64() == Some((offset + data.len()) as u64),
            "invalid_attachment_chunk"
        );
        ensure!(
            !data.is_empty() || offset == file.byte_len,
            "invalid_attachment_chunk"
        );
        let current = field(&value, "media_type")?;
        ensure!(
            current.len() <= 128
                && !current.chars().any(char::is_control)
                && (media.is_empty() || media == current),
            "invalid_attachment_media_type"
        );
        media = current.into();
        bytes.extend_from_slice(&data);
        reference = Some(file.clone());
        if bytes.len() == file.byte_len {
            ensure!(
                zork_mesh::content_root(&bytes) == file.content_root,
                "attachment_reference_mismatch"
            );
            return Ok((file, media, bytes));
        }
    }
}

pub(super) async fn receive(
    state: &AppState,
    rpc: &Rpc,
    chat: &ChatRow,
) -> Result<Vec<PreparedFile>> {
    ensure!(files::valid(&rpc.files), "invalid_attachments");
    let workspace = state
        .db
        .get_session(&chat.session_key)?
        .context("chat_not_found")?
        .workspace_path;
    let mut result = Vec::new();
    for file in &rpc.files {
        let (reference, media_type, content) = fetch(
            state,
            &rpc.subject.origin,
            |offset| FileRequest::Prepared {
                key: rpc.prepared_key.clone(),
                attachment_id: file.id.clone(),
                offset,
            },
            Some(file),
        )
        .await?;
        result.push(PreparedFile {
            source_path: format!("chat-send:{}/{}", rpc.prepared_key, reference.id),
            reference,
            workspace: workspace.clone(),
            media_type,
            content,
        });
    }
    Ok(result)
}

pub(super) async fn materialize(
    state: &AppState,
    session: &str,
    target: &str,
    chat: &str,
    id: &str,
) -> Result<Value> {
    let binding = state
        .db
        .get_binding_by_id(session)?
        .context("node_unknown_session")?;
    let (reference, _, bytes) = fetch(
        state,
        target,
        |offset| FileRequest::Published {
            chat_id: chat.into(),
            attachment_id: id.into(),
            offset,
        },
        None,
    )
    .await?;
    let root = PathBuf::from(binding.workspace_path());
    let copy = reference.clone();
    let path = tokio::task::spawn_blocking(move || write_snapshot(&root, &copy, &bytes)).await??;
    Ok(json!({"reference":reference,"path":path}))
}

fn write_snapshot(workspace: &Path, file: &FileRef, bytes: &[u8]) -> Result<PathBuf> {
    use std::io::Write;
    ensure!(file.valid(), "invalid_attachment");
    let workspace = workspace.canonicalize()?;
    let mut directory = workspace;
    for component in [".zork", "chat-files", &file.id, &file.content_root] {
        directory.push(component);
        match std::fs::create_dir(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let metadata = std::fs::symlink_metadata(&directory)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "attachment_path_outside_workspace"
        );
    }
    let path = directory.join(&file.name);
    if path.try_exists()? {
        ensure!(
            !std::fs::symlink_metadata(&path)?.file_type().is_symlink()
                && std::fs::read(&path)? == bytes,
            "attachment_snapshot_changed"
        );
        return Ok(path);
    }
    let temporary = directory.join(format!(".{}.partial", ulid::Ulid::new()));
    let result = (|| -> Result<()> {
        let mut out = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        out.write_all(bytes)?;
        out.sync_all()?;
        std::fs::rename(&temporary, &path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result?;
    Ok(path)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn materialization_rejects_parent_symlinks_before_creating_outside_directories() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), workspace.path().join(".zork")).unwrap();
        let file = FileRef {
            id: "file-test".into(),
            name: "report.txt".into(),
            byte_len: 1,
            content_root: "a".repeat(64),
        };
        assert!(write_snapshot(workspace.path(), &file, b"a").is_err());
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }
}
