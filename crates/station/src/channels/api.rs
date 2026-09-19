use super::*;
use crate::db::chats::PreparedFile;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use zork_client_types::{
    chat::{Author, AuthorKind, UpdatePreferences},
    files::FileRef,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rpc {
    pub subject: Subject,
    pub invocation_id: String,
    pub tool: String,
    pub arguments: Value,
    pub prepared_key: String,
    #[serde(default)]
    pub files: Vec<FileRef>,
    #[serde(default)]
    pub publication: Option<crate::db::chats::cards::publication::Grant>,
    #[serde(default)]
    pub interrupt: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileRequest {
    Prepared {
        key: String,
        attachment_id: String,
        offset: usize,
    },
    Published {
        chat_id: String,
        attachment_id: String,
        offset: usize,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolRequest {
    session_id: String,
    invocation_id: String,
    tool: String,
    arguments: Value,
    #[serde(default)]
    interrupt: bool,
}
fn validate(tool: &str, args: &Value) -> Result<()> {
    let definition = zork_agent_station_tools::channels::definitions()
        .into_iter()
        .find(|d| d.name == tool)
        .context("unknown_channel_tool")?;
    ensure!(
        jsonschema::validator_for(&definition.schema)?.is_valid(args),
        "invalid_channel_arguments"
    );
    Ok(())
}
pub async fn tool(State(state): State<AppState>, Json(request): Json<ToolRequest>) -> Response {
    match api(&state, request).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => (StatusCode::BAD_REQUEST, Json(json!({"error":error(&err)}))).into_response(),
    }
}

async fn api(state: &AppState, input: ToolRequest) -> Result<Value> {
    ensure!(
        !input.invocation_id.is_empty() && input.invocation_id.len() <= 256,
        "invalid_channel_invocation"
    );
    let who = node_access::subject(state, &input.session_id)?;
    validate(&input.tool, &input.arguments)?;
    if input.tool == "agent.list" && input.arguments.get("target").is_none() {
        return agents::discover(state, &who, &input.arguments).await;
    }
    let target = input.arguments["target"]
        .as_str()
        .unwrap_or("local")
        .to_owned();
    if !local(state, &target) {
        // Local channel reads and replies use the authenticated Station and
        // its message store; restoring Mesh peers must not block them.
        node_access::ready(state)?;
        access(state, &target)?;
    }
    let mut args = input.arguments.clone();
    args.as_object_mut().unwrap().remove("target");
    if zork_agent_station_tools::channels::sends_message(&input.tool) && args["chat_id"].is_null() {
        // A publishing call without an explicit destination goes to the caller's own
        // Chat. Recording it here keeps the persisted request self-describing.
        let home = state
            .db
            .agent_home(&who.agent)?
            .context("chat_id is required: this Agent has no Chat of its own")?;
        args["chat_id"] = json!(home.chat_id);
    }
    let key = format!("outgoing-{}", fingerprint(&(&who, &input.invocation_id))?);
    let mutation = zork_agent_station_tools::channels::mutating(&input.tool);
    if ordinary_send(&input.tool, &args) {
        if input.interrupt {
            return Ok(json!({"status":"delivery_unknown","operation_id":input.invocation_id}));
        }
        let key = format!("send-{}", ulid::Ulid::new());
        let files = if zork_agent_station_tools::channels::carries_files(&input.tool) {
            attachments::prepare(state, &who, &target, &args).await?
        } else {
            vec![]
        };
        state.db.prepare_send_files(
            &key,
            if local(state, &target) {
                "local"
            } else {
                &target
            },
            &files,
        )?;
        let _files = SendFiles {
            db: &state.db,
            key: key.clone(),
        };
        args.as_object_mut().unwrap().remove("attachments");
        let rpc = Rpc {
            subject: who,
            invocation_id: input.invocation_id,
            tool: input.tool,
            arguments: args,
            prepared_key: key.clone(),
            files: files.iter().map(|file| file.reference.clone()).collect(),
            publication: None,
            interrupt: false,
        };
        return dispatch(state, &key, &target, rpc).await;
    }

    if !mutation {
        let rpc = Rpc {
            subject: who,
            invocation_id: input.invocation_id,
            tool: input.tool,
            arguments: args,
            prepared_key: key,
            files: vec![],
            publication: None,
            interrupt: input.interrupt,
        };
        let mut value = route(state, &target, rpc).await?;
        if input.arguments["attachment_id"].is_string()
            && !matches!(value["status"].as_str(), Some("rejected" | "cancelled"))
        {
            value = attachments::materialize(
                state,
                &input.session_id,
                &target,
                field(&input.arguments, "chat_id")?,
                field(&input.arguments, "attachment_id")?,
            )
            .await?;
        }
        value["target"] = json!(if local(state, &target) {
            node_access::identity(state)
        } else {
            target
        });
        return Ok(value);
    }
    if input.interrupt {
        let rpc = if let Some(saved) = state.db.chat_outgoing(&key)? {
            let mut rpc: Rpc = serde_json::from_value(saved["rpc"].clone())?;
            rpc.interrupt = true;
            rpc
        } else {
            Rpc {
                subject: who,
                invocation_id: input.invocation_id,
                tool: input.tool,
                arguments: args,
                prepared_key: key.clone(),
                files: vec![],
                publication: None,
                interrupt: true,
            }
        };
        return dispatch(state, &key, &target, rpc).await;
    }
    let _guard = state.entries.lock_local_task(&key).await;
    let receipt = state
        .db
        .chat_begin(&key, &fingerprint(&(&input.tool, &input.arguments))?)?;
    if let Some(result) = receipt.result {
        return Ok(result);
    }
    let mut rpc: Rpc = if let Some(saved) = state.db.chat_outgoing(&key)? {
        ensure!(saved["target"] == target, "idempotency_conflict");
        serde_json::from_value(saved["rpc"].clone())?
    } else if input.interrupt || receipt.cancelled {
        let result =
            json!({"status":"cancelled","operation_id":input.invocation_id,"target":target});
        state.db.finish_chat_outgoing(&key, &result)?;
        return Ok(result);
    } else {
        let publication = if input.tool == "chat.post_message" {
            crate::business_cards::prepare(state, &who, &target, &args).await?
        } else {
            None
        };
        let files = if zork_agent_station_tools::channels::carries_files(&input.tool) {
            attachments::prepare(state, &who, &target, &args).await?
        } else {
            Vec::<PreparedFile>::new()
        };
        args.as_object_mut().unwrap().remove("attachments");
        let rpc = Rpc {
            subject: who,
            invocation_id: input.invocation_id,
            tool: input.tool,
            arguments: args,
            prepared_key: key.clone(),
            files: files.iter().map(|f| f.reference.clone()).collect(),
            publication,
            interrupt: false,
        };
        let destination = if local(state, &target) {
            "local"
        } else {
            &target
        };
        state.db.save_chat_outgoing(
            &key,
            destination,
            &json!({"target":target,"rpc":rpc}),
            &files,
        )?;
        rpc
    };
    rpc.interrupt = input.interrupt;
    if rpc.tool == "chat.update_preferences" {
        state.db.record_chat_source(
            &rpc.subject.agent,
            if local(state, &target) {
                "local"
            } else {
                &target
            },
        )?;
    }
    dispatch(state, &key, &target, rpc).await
}

async fn dispatch(state: &AppState, key: &str, target: &str, rpc: Rpc) -> Result<Value> {
    let persist = !ordinary_send(&rpc.tool, &rpc.arguments);
    let operation = rpc.invocation_id.clone();
    let preferences = (rpc.tool == "chat.update_preferences").then(|| {
        (
            rpc.subject.agent.clone(),
            rpc.arguments["chat_id"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        )
    });
    match route(state, target, rpc).await {
        Ok(mut result) => {
            if let Some((agent, chat)) = preferences {
                let chat = result["chat_id"].as_str().unwrap_or(&chat).to_owned();
                if let (Some(generation), Some(subscribed)) =
                    (result["revision"].as_u64(), result["subscribed"].as_bool())
                {
                    if let Err(error) = state.db.record_receiving_policy(
                        &agent,
                        if local(state, target) {
                            "local"
                        } else {
                            target
                        },
                        &chat,
                        generation,
                        subscribed,
                    ) {
                        return Ok(
                            json!({"status":"delivery_unknown","operation_id":operation,"target":target,"error":super::error(&error)}),
                        );
                    }
                }
            }
            if result.get("status").is_none() {
                result["status"] = json!("committed");
            }
            result["operation_id"] = json!(operation);
            result["target"] = json!(if local(state, target) {
                node_access::identity(state)
            } else {
                target.to_owned()
            });
            if persist {
                if let Err(error) = state.db.finish_chat_outgoing(key, &result) {
                    tracing::warn!(error=%super::error(&error),"Committed channel receipt awaits local recovery");
                }
            }
            Ok(result)
        }
        Err(err) => Ok(
            json!({"status":"delivery_unknown","operation_id":operation,"target":target,"error":error(&err)}),
        ),
    }
}
pub(super) async fn route(state: &AppState, target: &str, mut rpc: Rpc) -> Result<Value> {
    if local(state, target) {
        return Ok(match execute(state, rpc, true).await {
            Ok(value) => value,
            Err(err) => json!({"status":"rejected","error":error(&err)}),
        });
    }
    access(state, target)?;
    rpc.subject.origin = node_access::identity(state);
    state
        .mesh
        .get()
        .context("node_starting")?
        .channel_call(target, rpc)
        .await
}

pub(super) async fn execute(state: &AppState, rpc: Rpc, is_local: bool) -> Result<Value> {
    if !is_local {
        access(state, &rpc.subject.origin)?;
    }
    ensure!(
        rpc.invocation_id.len() <= 256
            && !rpc.invocation_id.is_empty()
            && rpc.subject.agent.len() <= 256
            && !rpc.subject.agent.contains('/'),
        "invalid_channel_identity"
    );
    ensure!(
        serde_json::to_vec(&rpc)?.len() <= 192 * 1024,
        "channel_request_too_large"
    );
    let mut schema_args = rpc.arguments.clone();
    if zork_agent_station_tools::channels::carries_files(&rpc.tool) {
        // Frozen files replace the original attachment arguments; a placeholder
        // of the same arity keeps the replayed request valid against the schema.
        schema_args["attachments"] = json!(rpc
            .files
            .iter()
            .map(|_| json!({"file_path":"frozen"}))
            .collect::<Vec<_>>());
    }
    validate(&rpc.tool, &schema_args)?;
    let command = format!(
        "command-{}",
        fingerprint(&(&rpc.subject, &rpc.invocation_id))?
    );
    let ordinary = ordinary_send(&rpc.tool, &rpc.arguments);
    let mutation = zork_agent_station_tools::channels::mutating(&rpc.tool) && !ordinary;
    if rpc.interrupt {
        if mutation {
            let signature =
                fingerprint(&(&rpc.tool, &rpc.arguments, &rpc.files, &rpc.publication))?;
            if let Some(result) = state.db.chat_cancel(&command, &signature)? {
                return Ok(result);
            }
        }
        crate::interaction_registry::cancel(state, &rpc.subject, &rpc.invocation_id).await?;
        return Ok(json!({"status":"cancelled"}));
    }
    let _guard = state.entries.lock_local_task(&command).await;
    let object = if mutation {
        if rpc.tool.starts_with("agent.") {
            node_access::manage(state, &rpc.subject, is_local)?;
        }
        let signature = fingerprint(&(&rpc.tool, &rpc.arguments, &rpc.files, &rpc.publication))?;
        let receipt = state.db.chat_begin(&command, &signature)?;
        if let Some(result) = receipt.result {
            return Ok(result);
        }
        if rpc.interrupt || receipt.cancelled {
            if let Some(result) = state.db.chat_cancel(&command, &signature)? {
                return Ok(result);
            }
            return Ok(json!({"status":"cancelled"}));
        }
        receipt.object_id
    } else if ordinary {
        ulid::Ulid::new().to_string()
    } else {
        String::new()
    };
    if rpc.tool.starts_with("agent.") {
        return agents::execute(state, &rpc, &command, &object, is_local).await;
    }
    let args = &rpc.arguments;
    if rpc.tool == "chat.list" {
        let after = cursor(args, "chats")?;
        let limit = limit(args);
        let rows = state.db.chats(after.as_deref(), limit)?;
        let next = rows
            .last()
            .filter(|_| rows.len() == limit)
            .map(|c| encode_cursor("chats", &c.chat_id));
        return Ok(json!({"items":rows,"next_cursor":next}));
    }
    if rpc.tool == "chat.create" {
        let creator = Author {
            id: actor(&rpc.subject),
            kind: AuthorKind::Agent,
            name: if is_local {
                state
                    .db
                    .node_agent(&rpc.subject.agent)?
                    .map(|agent| agent.name)
            } else {
                None
            },
        };
        return Ok(serde_json::to_value(state.db.create_chat_as(
            &command,
            &object,
            field(args, "title")?,
            Some(&creator),
        )?)?);
    }
    let channel = state.db.chat(field(args, "chat_id")?)?;
    let chat = &channel.channel.chat_id;
    match rpc.tool.as_str() {
        "chat.inspect" => {
            let scope = fingerprint(&("participants", chat))?;
            let after = cursor(args, &scope)?
                .map(|s| s.parse::<usize>())
                .transpose()
                .context("invalid_chat_cursor")?
                .unwrap_or(0);
            let participants = state.db.chat_participants(chat)?;
            ensure!(after <= participants.len(), "invalid_chat_cursor");
            let through = (after + limit(args)).min(participants.len());
            Ok(
                json!({"channel":channel.channel,"participants":&participants[after..through],
                "next_cursor":(through<participants.len()).then(||encode_cursor(&scope,&through.to_string()))}),
            )
        }
        "chat.preferences" => Ok(serde_json::to_value(
            state.db.chat_preferences(chat, &actor(&rpc.subject))?,
        )?),
        "chat.update_preferences" => {
            let mut update = args.clone();
            update.as_object_mut().unwrap().remove("chat_id");
            let update: UpdatePreferences = serde_json::from_value(update)?;
            let mut value = serde_json::to_value(state.db.update_chat_preferences(
                &command,
                chat,
                &actor(&rpc.subject),
                &update,
            )?)?;
            value["chat_id"] = json!(chat);
            Ok(value)
        }
        "chat.post_message" | "chat.post_file" | "chat.post_message.android_script" => {
            if !args["oauth"].is_null() {
                ensure!(
                    args["interaction"].is_null() && rpc.files.is_empty(),
                    "oauth_card_cannot_include_interaction_or_attachments"
                );
                node_access::manage(state, &rpc.subject, is_local)?;
                return crate::provider_login::execute(state, &rpc, &command, &object).await;
            }
            let author = Author {
                id: actor(&rpc.subject),
                kind: AuthorKind::Agent,
                name: if is_local {
                    state.db.node_agent(&rpc.subject.agent)?.map(|a| a.name)
                } else {
                    None
                },
            };
            let files = attachments::receive(state, &rpc, &channel).await?;
            // Authorization is checked again after file transfer, before the commit.
            if !is_local {
                access(state, &rpc.subject.origin)?;
            }
            let mentions: Vec<String> = args
                .get("mentions")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let reference = args["interaction"]["request_id"].as_str();
            let interaction = if let Some(id) = reference {
                if let Some(grant) = &rpc.publication {
                    ensure!(
                        grant.request_id == id
                            && grant.chat == args["chat_id"].as_str().unwrap_or_default(),
                        "interaction_publication_denied"
                    );
                    let existing = state.db.business_card_message(id, chat)?;
                    let root = existing
                        .as_ref()
                        .map_or(object.as_str(), |m| m.message_id.as_str());
                    crate::business_cards::publish(state, grant, chat, root, &author.id).await?;
                }
                let request = state.db.business_card(id)?;
                Some(serde_json::to_value(
                    zork_client_types::interaction::MessageContent::linked(
                        id.into(),
                        request.handler,
                        request.request,
                        None,
                    ),
                )?)
            } else if rpc.tool == "chat.post_message.android_script" {
                let script = zork_client_types::local_script::Card {
                    kind: zork_client_types::local_script::Kind::LocalScript,
                    version: zork_client_types::local_script::VERSION,
                    platform: "android".into(),
                    title: field(args, "title")?.into(),
                    description: args["description"].as_str().map(str::to_owned),
                    source: field(args, "source")?.into(),
                };
                script.validate().map_err(anyhow::Error::msg)?;
                Some(serde_json::to_value(script)?)
            } else {
                None
            };
            let message = state.db.post_chat_content(
                (!ordinary).then_some(command.as_str()),
                &object,
                chat,
                &author,
                args["text"].as_str().unwrap_or(""),
                &files,
                args["reply_to"].as_str(),
                &mentions,
                &[],
                interaction.as_ref(),
            )?;
            state
                .entries
                .publish_visible_message(&state.db.chat_visible_message(&message.message_id)?);
            Ok(serde_json::to_value(message)?)
        }
        "chat.history" => {
            let query = None::<&str>;
            let scope = fingerprint(&(chat, state.db.chat_source_epoch(chat)?, query))?;
            let before = cursor(args, &scope)?
                .map(|s| s.parse::<i64>())
                .transpose()
                .context("invalid_chat_cursor")?;
            let rows = state.db.chat_messages(chat, before, limit(args))?;
            let total = rows.len();
            let mut items = Vec::new();
            let mut bytes = 0;
            let mut position = None;
            for (sequence, m) in rows {
                let truncated = m.text.len() > 2048;
                let mut value = serde_json::to_value(&m).expect("message");
                if truncated {
                    value["text"] = json!(m.text.chars().take(512).collect::<String>());
                    value["truncated"] = json!(true);
                }
                let size = serde_json::to_vec(&value)?.len();
                ensure!(size <= 128 * 1024, "chat_message_metadata_too_large");
                if bytes + size > 160 * 1024 {
                    break;
                }
                bytes += size;
                items.push(value);
                position = Some(sequence);
            }
            let next = position
                .filter(|_| items.len() < total || total == limit(args))
                .map(|seq| encode_cursor(&scope, &seq.to_string()));
            Ok(json!({"items":items,"next_cursor":next}))
        }
        "chat.read" => {
            ensure!(
                args["message_id"].is_string() != args["attachment_id"].is_string(),
                "select_message_or_attachment"
            );
            if let Some(id) = args["message_id"].as_str() {
                let mut message = state.db.chat_message(chat, id)?;
                let offset: usize = args["offset"]
                    .as_u64()
                    .unwrap_or(0)
                    .try_into()
                    .context("invalid_message_offset")?;
                ensure!(
                    offset <= message.text.len() && message.text.is_char_boundary(offset),
                    "invalid_message_offset"
                );
                let total = message.text.len();
                let mut through = (offset + 32 * 1024).min(total);
                while !message.text.is_char_boundary(through) {
                    through -= 1;
                }
                message.text = message.text[offset..through].to_owned();
                let mut value = serde_json::to_value(message)?;
                value["offset"] = json!(offset);
                value["text_bytes"] = json!(total);
                value["next_offset"] = json!((through < total).then_some(through));
                ensure!(
                    serde_json::to_vec(&value)?.len() <= 192 * 1024,
                    "chat_message_metadata_too_large"
                );
                Ok(value)
            } else {
                Ok(
                    json!({"reference":state.db.conversation_file_ref(&channel.session_key,field(args,"attachment_id")?)?}),
                )
            }
        }
        _ => anyhow::bail!("unknown_channel_tool"),
    }
}

pub(super) fn limit(args: &Value) -> usize {
    args["limit"].as_u64().unwrap_or(25) as usize
}
pub(super) fn encode_cursor(scope: &str, position: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&(scope, position)).expect("cursor"))
}
pub(super) fn cursor(args: &Value, scope: &str) -> Result<Option<String>> {
    use base64::Engine;
    args["cursor"]
        .as_str()
        .map(|s| {
            let (saved, position): (String, String) = serde_json::from_slice(
                &base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(s)
                    .context("invalid_chat_cursor")?,
            )
            .context("invalid_chat_cursor")?;
            ensure!(saved == scope, "invalid_chat_cursor");
            Ok(position)
        })
        .transpose()
}

fn ordinary_send(tool: &str, args: &Value) -> bool {
    zork_agent_station_tools::channels::ordinary_send(tool, args)
}
struct SendFiles<'a> {
    db: &'a crate::db::StationDb,
    key: String,
}
impl Drop for SendFiles<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.db.clear_send_files(&self.key) {
            tracing::warn!(%error,"Could not clear completed message file transfer");
        }
    }
}
