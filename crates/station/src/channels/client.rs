//! The native client uses the same channel transaction as Agent tools.
use super::*;
use crate::db::VisibleMessageRow;
use zork_client_types::{
    chat::{Author, AuthorKind},
    files,
};

pub async fn post(
    state: &AppState,
    session: &crate::db::SessionRow,
    request: &str,
    content: &str,
    reply_to: Option<&str>,
    mentions: &[String],
    client_id: Option<&str>,
) -> Result<VisibleMessageRow> {
    let client_id = client_id
        .map(|id| -> Result<String> {
            ensure!(
                id.len() == 26 && ulid::Ulid::from_string(id).is_ok(),
                "invalid_client_id"
            );
            Ok(format!("{}/{id}", crate::node_access::identity(state)))
        })
        .transpose()?;
    let channel = state.db.chat(&session.key)?;
    // Core's already-persisted row and the source echo share this identity.
    let message_id = format!(
        "client-{}-{request}",
        session.id.as_deref().context("chat_session_missing")?
    );
    {
        let (text, refs) = files::decode(content).unwrap_or((content.into(), vec![]));
        ensure!(files::valid(&refs), "invalid_attachments");
        let files = refs
            .iter()
            .map(|file| {
                state.db.copy_chat_file(
                    &channel.channel.chat_id,
                    &channel.channel.chat_id,
                    &file.id,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            files.iter().map(|f| &f.reference).eq(refs.iter()),
            "attachment_reference_mismatch"
        );
        state.db.post_chat_content_from_client(
            None,
            &message_id,
            &channel.channel.chat_id,
            &Author {
                id: "local-user".into(),
                kind: AuthorKind::User,
                name: None,
            },
            &text,
            &files,
            reply_to,
            mentions,
            &[],
            None,
            client_id.as_deref(),
        )?;
    }
    let message = state.db.chat_visible_message(&message_id)?;
    state.entries.publish_visible_message(&message);
    Ok(message)
}
