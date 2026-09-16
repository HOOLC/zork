use super::*;
use zork_client_types::files::{self, FileRef};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UploadChunk {
    file: FileRef,
    offset: usize,
    bytes: Vec<u8>,
}

pub(super) async fn post_file(state: &AppState, key: &str, body: &Value) -> Response {
    let path = read_string(body, &["file_path", "filePath"]);
    let attachment = read_string(body, &["attachment_id", "attachmentId"]);
    if path.is_some() == attachment.is_some() {
        return fail(
            StatusCode::BAD_REQUEST,
            "provide_file_path_or_attachment_id",
        );
    }
    let request_id = read_string(body, &["request_id", "requestId"]);
    if request_id
        .as_ref()
        .is_some_and(|id| id.is_empty() || id.len() > 200)
    {
        return fail(StatusCode::BAD_REQUEST, "invalid_request_id");
    }
    let caption = read_string(body, &["initial_comment", "initialComment"]);
    if caption.as_ref().is_some_and(|s| s.len() > 16 * 1024) {
        return fail(StatusCode::BAD_REQUEST, "artifact_caption_too_large");
    }
    let source_task = read_string(body, &["source_task_id", "sourceTaskId"]);
    if source_task.is_some() && attachment.is_none() {
        return fail(
            StatusCode::BAD_REQUEST,
            "source_task_requires_attachment_id",
        );
    }
    let result: anyhow::Result<Value> = async {
        let session = state
            .db
            .get_session(key)?
            .context("conversation_not_found")?;
        let task = state.db.product_task_for_session(key)?;
        anyhow::ensure!(
            task.as_ref().is_none_or(|t| !t.state.is_closed()),
            "task_closed_reopen_required"
        );
        let db = state.db.clone();
        let owned_key = key.to_owned();
        let text = caption.clone().unwrap_or_default();
        let (artifact, reference) = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let key = owned_key;
            let id = if let Some(id) = attachment {
                let source = if let Some(task_id) = source_task {
                    let leader = db.agent_for_session(&key)?.context("leader_required")?;
                    let (owner, _, _) = db
                        .worker_task_owner(&task_id)?
                        .context("assigned_task_required")?;
                    anyhow::ensure!(owner == leader.id, "task_not_owned_by_leader");
                    let task = db.product_task(&task_id)?.context("task_not_found")?;
                    db.get_session_by_id(
                        task.session_id.as_deref().context("task_session_missing")?,
                    )?
                    .context("task_session_missing")?
                    .key
                } else {
                    key.clone()
                };
                let reference = db.conversation_file_ref(&source, &id)?;
                if source == key {
                    id
                } else {
                    let copied = db.copy_message_files(
                        &source,
                        &session,
                        &files::compose("", &[reference]),
                    )?;
                    files::decode(&copied)
                        .context("copied_attachment_missing")?
                        .1[0]
                        .id
                        .clone()
                }
            } else {
                let path = path.context("file_path_required")?;
                let artifact = if let Some(task) = task {
                    db.register_artifact(&task.task_id, StdPath::new(&path), caption.as_deref())?
                } else {
                    db.register_conversation_artifact(
                        &key,
                        StdPath::new(&path),
                        caption.as_deref(),
                    )?
                };
                artifact.artifact_id
            };
            let reference = db.conversation_file_ref(&key, &id)?;
            let artifact = db
                .list_artifacts(None)?
                .into_iter()
                .find(|a| a.artifact_id == id)
                .context("artifact_not_found")?;
            Ok((artifact, reference))
        })
        .await??;
        let payload = files::compose(&text, &[reference]);
        let message_id = format!(
            "file-message-{}",
            zork_mesh::content_root(&serde_json::to_vec(&(
                key,
                request_id.as_deref().unwrap_or(&payload)
            ))?)
        );
        let session = state
            .db
            .get_session(key)?
            .context("conversation_not_found")?;
        let message = state.db.record_visible_message(
            &message_id,
            key,
            &session.connection_id,
            &session.channel_id,
            &session.root_thread_ts,
            "assistant",
            &payload,
            Some("file"),
        )?;
        state.entries.publish_visible_message(&message);
        Ok(json!({"ok":true,"artifact":artifact,"message":state.entries.message_json(&message)}))
    }
    .await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => fail(StatusCode::BAD_REQUEST, &error.to_string()),
    }
}

pub(super) async fn upload(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<UploadChunk>,
) -> Response {
    let _guard = state.entries.lock_local_task(&session_id).await;
    let session = match local_im_session(&state, &session_id) {
        Ok(session) => session,
        Err(response) => return *response,
    };
    if !can_send_local_message(&state, &session) {
        return fail(StatusCode::CONFLICT, "conversation_cannot_accept_files");
    }
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || {
        db.receive_file_chunk(&session, &body.file, body.offset, &body.bytes)
    })
    .await
    {
        Ok(Ok(received)) => Json(json!({"ok":true,"received":received})).into_response(),
        Ok(Err(error)) => fail(StatusCode::BAD_REQUEST, &error.to_string()),
        Err(error) => db_error(error.into()),
    }
}

/// Validate ownership and expose the existing immutable body to the Agent.
/// The persisted message contains only file references; no second copy is made.
pub(crate) async fn agent_content(
    state: &AppState,
    session_key: &str,
    content: &str,
) -> anyhow::Result<String> {
    let Some((text, references)) = files::decode(content) else {
        return Ok(content.into());
    };
    anyhow::ensure!(files::valid(&references), "invalid_attachments");
    let db = state.db.clone();
    let key = session_key.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut descriptions = Vec::new();
        for file in references {
            let path = db.conversation_file_path(&key,&file)?;
            descriptions.push(
                json!({"attachment_id":file.id,"name":file.name,"bytes":file.byte_len,"path":path}),
            );
        }
        Ok(format!(
            "{text}\n\nConversation attachments (local snapshots, available to file tools):\n{}",
            serde_json::to_string(&descriptions)?
        ))
    })
    .await?
}
