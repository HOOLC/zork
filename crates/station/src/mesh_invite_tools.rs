//! Agent-facing Mesh invitations. The Station mints the one-time join command
//! through the same enrollment path as the clients and returns it only in the
//! `mesh.invite` result. Any Session bound to a Station conversation may mint:
//! connected IM such as Slack is trusted like the Zork clients.
use crate::{db::StationDb, state::AppState};
use anyhow::{Context, Result};
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const LEDGER_LIMIT: usize = 200;
const LEDGER_RETENTION_SECONDS: u64 = 7 * 86400;
const SECURITY: &str = "Secret one-time Mesh admission command. Run it on the new computer, or show it only to the requesting user in this Zork Chat. Never post it to external channels, tickets or files that sync outward. Revoke it with mesh.revoke if it was exposed or is no longer needed.";
static LEDGER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    session_id: String,
    invocation_id: String,
    tool: String,
    arguments: Value,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct InviteArgs {
    label: Option<String>,
}
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    #[serde(default)]
    include_inactive: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RevokeArgs {
    id: String,
}

/// Station-local record of which Session requested an invitation. It never
/// contains the ticket, secret or command.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
struct Entry {
    id: String,
    session_id: String,
    invocation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    created_at: u64,
    expires_at: u64,
}

#[derive(Debug)]
struct Refusal {
    code: &'static str,
    message: String,
}
impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Refusal {}
fn refuse(code: &'static str, message: impl Into<String>) -> anyhow::Error {
    Refusal {
        code,
        message: message.into(),
    }
    .into()
}

pub async fn tool(State(state): State<AppState>, Json(input): Json<Request>) -> Response {
    match api(&state, input).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => {
            let (code, message) = describe(&error);
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"ok":false,"error":code,"message":message})),
            )
                .into_response()
        }
    }
}

fn describe(error: &anyhow::Error) -> (String, String) {
    if let Some(refusal) = error.downcast_ref::<Refusal>() {
        return (refusal.code.into(), refusal.message.clone());
    }
    let code = error.to_string();
    let message = match code.as_str() {
        "mesh_not_ready" => "Mesh is not running on this Station yet. Check `zork mesh status` or the device settings, then retry.",
        "mesh_disabled" => "Mesh is switched off on this Station, so no invitation was created. Ask the user to turn on device connections (允许设备连接) in this device's settings, or to run `zork mesh invite` in a terminal on this Station, which enables Mesh first.",
        "too_many_active_invites" | "too_many_recent_invites" => "Too many open invitations. Revoke unused ones with mesh.revoke or wait for them to expire.",
        "invite_already_used_remove_device_instead" => "This invitation was already used to join a device. Remove that device from the Mesh instead of revoking the invitation.",
        "invite_not_found" | "invite_expired_or_restart" | "invite_expired_or_unknown" => "Invitation not found. It may have expired or been pruned after expiry.",
        "device_removed_from_mesh" => "This Station was removed from the Mesh and cannot invite devices.",
        _ if code.contains("membership") || code.contains("authority") || code.contains("connect") => "The Mesh managing Station is unreachable. Invitations are issued by it; retry when it is online.",
        _ => "Mesh invitation operation failed.",
    };
    let code = if code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') && code.len() <= 64 {
        code
    } else {
        "mesh_invite_failed".into()
    };
    (code, message.into())
}

async fn api(state: &AppState, input: Request) -> Result<Value> {
    anyhow::ensure!(
        !input.invocation_id.is_empty() && input.invocation_id.len() <= 256,
        "invalid_invocation"
    );
    let arguments = if input.arguments.is_null() {
        json!({})
    } else {
        input.arguments
    };
    let args_error = |error: serde_json::Error| {
        refuse(
            "invalid_arguments",
            format!(
                "Invalid arguments for {}: {error}. Use tool.help for the parameter definition.",
                input.tool
            ),
        )
    };
    let root = state.config.data_root.clone();
    match input.tool.as_str() {
        "mesh.invite" => {
            let args: InviteArgs = serde_json::from_value(arguments).map_err(args_error)?;
            let label = clean_label(args.label)?;
            admit(&state.db, &input.session_id)?;
            let _guard = LEDGER.lock().await;
            if let Some(entry) = load(&root)?.into_iter().find(|e| {
                e.session_id == input.session_id && e.invocation_id == input.invocation_id
            }) {
                return Ok(replayed(&entry, now()));
            }
            // Never hand out an invitation that cannot be redeemed: a Station
            // with Mesh switched off refuses instead of minting one.
            let service = state
                .mesh
                .get()
                .ok_or_else(|| anyhow::anyhow!(crate::node::mesh_unavailable(state)))?;
            let created = service.enrollment.create(state).await?;
            let entry = Entry {
                id: created["id"].as_str().context("mesh_invite_failed")?.into(),
                session_id: input.session_id,
                invocation_id: input.invocation_id,
                label,
                created_at: now(),
                expires_at: created["expires_at"]
                    .as_u64()
                    .context("mesh_invite_failed")?,
            };
            record(&root, entry.clone())?;
            Ok(invite_result(&created, &entry, now()))
        }
        "mesh.invites" => {
            let args: ListArgs = serde_json::from_value(arguments).map_err(args_error)?;
            state
                .db
                .get_binding_by_id(&input.session_id)?
                .context("unknown_session")?;
            let items = match state.mesh.get() {
                Some(service) => service.enrollment.list(state).await?,
                None => json!({"items":[]}),
            };
            let ledger = {
                let _guard = LEDGER.lock().await;
                load(&root)?
            };
            Ok(project(
                &items,
                &ledger,
                &input.session_id,
                args.include_inactive,
                now(),
            ))
        }
        "mesh.revoke" => {
            let args: RevokeArgs = serde_json::from_value(arguments).map_err(args_error)?;
            if args.id.len() != 32 || !args.id.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(refuse(
                    "invalid_invite_id",
                    "Use an invitation id returned by mesh.invite or mesh.invites.",
                ));
            }
            state
                .db
                .get_binding_by_id(&input.session_id)?
                .context("unknown_session")?;
            let service = state.mesh.get().context("mesh_not_ready")?;
            service.enrollment.revoke(state, &args.id).await?;
            Ok(json!({"id":args.id,"state":"revoked"}))
        }
        other => Err(refuse(
            "unknown_tool",
            format!("{other} is not a Mesh invitation tool."),
        )),
    }
}

/// Minting requires a Session bound to a Station conversation. Every
/// connected entry (Zork clients, Slack, proactive bindings, delegated tasks)
/// is trusted: an IM is only connected when the user trusts it.
fn admit(db: &StationDb, session_id: &str) -> Result<()> {
    db.get_binding_by_id(session_id)?.ok_or_else(|| {
        refuse(
            "unknown_session",
            "This Session is not bound to a Station Chat.",
        )
    })?;
    Ok(())
}

fn clean_label(label: Option<String>) -> Result<Option<String>> {
    let Some(label) = label else { return Ok(None) };
    let label = label.trim();
    if label.is_empty() {
        return Ok(None);
    }
    if label.chars().count() > 80 || label.chars().any(char::is_control) {
        return Err(refuse(
            "invalid_label",
            "label must be a short single line (at most 80 characters).",
        ));
    }
    Ok(Some(label.into()))
}

fn invite_result(created: &Value, entry: &Entry, now: u64) -> Value {
    let mut value = json!({
        "id": entry.id,
        "state": "created",
        "command": created["command"],
        "expires_at": entry.expires_at,
        "expires_in_seconds": entry.expires_at.saturating_sub(now),
        "single_use": true,
        "security": SECURITY,
    });
    if let Some(url) = created["install_url"].as_str() {
        value["install_url"] = json!(url);
    }
    if let Some(label) = &entry.label {
        value["label"] = json!(label);
    }
    value
}

fn replayed(entry: &Entry, now: u64) -> Value {
    json!({
        "id": entry.id,
        "state": if entry.expires_at <= now { "expired" } else { "created" },
        "expires_at": entry.expires_at,
        "single_use": true,
        "command_shown": false,
        "note": "This invocation already created the invitation; its one-time command is not shown again. Check mesh.invites, revoke it with mesh.revoke if unused, and call mesh.invite again if a new command is needed.",
    })
}

/// Invitation states for the Agent. Secrets, ticket fragments and claim
/// challenges never appear here.
fn project(
    listed: &Value,
    ledger: &[Entry],
    session_id: &str,
    include_inactive: bool,
    now: u64,
) -> Value {
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in listed["items"].as_array().into_iter().flatten() {
        let Some(id) = item["id"].as_str() else {
            continue;
        };
        seen.insert(id.to_owned());
        let expires_at = item["expires_at"].as_u64().unwrap_or_default();
        let (state, joining) = match item["status"].as_str() {
            Some("revoked") => ("revoked", false),
            Some("joined") => ("used", false),
            Some("expired") => ("expired", false),
            Some("connecting") => ("created", true),
            _ if expires_at <= now => ("expired", false),
            _ => ("created", false),
        };
        let entry = ledger.iter().find(|e| e.id == id);
        let mut value = json!({"id":id,"state":state,"expires_at":expires_at,"requested_in_this_chat":entry.is_some_and(|e| e.session_id == session_id)});
        if joining {
            value["joining"] = json!(true);
        }
        if state == "used" || joining {
            let device = &item["device"];
            if let Some(origin) = device["origin"].as_str().or(item["origin"].as_str()) {
                value["device"] = json!({"id":origin,"name":device["name"]});
            }
        }
        if let Some(label) = entry.and_then(|e| e.label.as_ref()) {
            value["label"] = json!(label);
        }
        items.push(value);
    }
    // Unused invitations live in Station memory; after expiry or a restart only
    // the local request record remains.
    for entry in ledger.iter().filter(|e| !seen.contains(&e.id)) {
        let mut value = json!({"id":entry.id,"state":"expired","expires_at":entry.expires_at,"requested_in_this_chat":entry.session_id == session_id});
        if entry.expires_at > now {
            value["note"] = json!(
                "No longer known to the managing Station (restarted); it can no longer be used."
            );
        }
        if let Some(label) = &entry.label {
            value["label"] = json!(label);
        }
        items.push(value);
    }
    let active = |v: &Value| matches!(v["state"].as_str(), Some("created"));
    let relevant = |v: &Value| include_inactive || active(v) || v["requested_in_this_chat"] == true;
    let mut items: Vec<_> = items.into_iter().filter(relevant).collect();
    items.sort_by(|a, b| b["expires_at"].as_u64().cmp(&a["expires_at"].as_u64()));
    items.truncate(50);
    json!({"items":items})
}

fn path(root: &Path) -> PathBuf {
    root.join("mesh/agent-invites.json")
}
fn load(root: &Path) -> Result<Vec<Entry>> {
    match std::fs::read(path(root)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes).unwrap_or_default()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}
fn record(root: &Path, entry: Entry) -> Result<()> {
    let mut entries = load(root)?;
    entries.retain(|e| e.created_at.saturating_add(LEDGER_RETENTION_SECONDS) > entry.created_at);
    entries.push(entry);
    let excess = entries.len().saturating_sub(LEDGER_LIMIT);
    entries.drain(..excess);
    crate::enrollment::private_json(&path(root), &entries)
}
fn now() -> u64 {
    zork_mesh::enrollment::now()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::EnsureSession;

    fn session(db: &StationDb, connection: &str, platform: &str, kind: &str) -> String {
        let row = db
            .ensure_session(EnsureSession {
                connection_id: connection,
                platform,
                channel_id: kind,
                root_thread_ts: kind,
                channel_type: Some(kind),
                initiator_user_id: Some("user"),
                initiator_message_ts: None,
            })
            .unwrap();
        let id = format!("session-{kind}");
        db.set_agent_session(&row.key, &id, &row.workspace_path, "p", "m", "high")
            .unwrap();
        id
    }

    #[test]
    fn every_bound_session_may_mint_invitations() {
        let dir = tempfile::tempdir().unwrap();
        let db = StationDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        let chat = session(&db, "local_gui", "local_gui", "leader_chat");
        let slack = session(&db, "work", "slack", "channel");
        let worker = session(&db, "local_gui", "local_gui", "worker_task");
        let binding = db.ensure_proactive_binding("work", "slack").unwrap();
        db.set_proactive_agent_session(
            "work",
            "proactive-session",
            &binding.workspace_path,
            "p",
            "m",
            "high",
        )
        .unwrap();
        for id in [chat, slack, worker, "proactive-session".to_owned()] {
            admit(&db, &id).unwrap();
        }
        assert_eq!(
            admit(&db, "missing")
                .unwrap_err()
                .downcast_ref::<Refusal>()
                .unwrap()
                .code,
            "unknown_session"
        );
    }

    fn entry(id: &str, session: &str, invocation: &str, expires_at: u64) -> Entry {
        Entry {
            id: id.into(),
            session_id: session.into(),
            invocation_id: invocation.into(),
            label: Some("Office Mac".into()),
            created_at: 1_000,
            expires_at,
        }
    }

    #[test]
    fn invite_result_carries_command_once_and_replay_hides_it() {
        let created = json!({"id":"a".repeat(32),"expires_at":1_900,"invitation":"TICKET","command":"zork mesh join 'TICKET' --channel stable","install_url":"https://x/install#ticket=TICKET","scope":"personal_mesh"});
        let record = entry(&"a".repeat(32), "s", "call", 1_900);
        let value = invite_result(&created, &record, 1_000);
        assert_eq!(value["command"], "zork mesh join 'TICKET' --channel stable");
        assert_eq!(value["state"], "created");
        assert_eq!(value["single_use"], true);
        assert_eq!(value["expires_in_seconds"], 900);
        assert_eq!(value["label"], "Office Mac");
        assert!(value["security"].as_str().unwrap().contains("mesh.revoke"));
        assert!(value.get("invitation").is_none());
        let replay = replayed(&record, 1_100);
        assert!(!replay.to_string().contains("TICKET"));
        assert_eq!(replay["command_shown"], false);
        assert_eq!(replayed(&record, 2_000)["state"], "expired");
    }

    #[test]
    fn listing_maps_states_without_secrets() {
        let id = |c: char| c.to_string().repeat(32);
        let listed = json!({"items":[
            {"id":id('a'),"expires_at":2_000,"status":"waiting","claim_id":"CLAIMHASH","device":null,"origin":null},
            {"id":id('b'),"expires_at":2_000,"status":"connecting","claim_id":"CLAIMHASH","device":{"origin":"key:new","name":"Laptop","addr":null},"origin":null},
            {"id":id('c'),"expires_at":900,"status":"joined","claim_id":"CLAIMHASH","device":{"origin":"key:new","name":"Laptop","addr":"1.2.3.4:5"},"origin":"key:new"},
            {"id":id('d'),"expires_at":2_000,"status":"revoked","claim_id":null,"device":null,"origin":null},
            {"id":id('e'),"expires_at":900,"status":"expired","claim_id":null,"device":null,"origin":null},
            {"id":id('f'),"expires_at":1_000,"status":"waiting","claim_id":null,"device":null,"origin":null},
        ]});
        let ledger = vec![
            entry(&id('c'), "chat", "1", 900),
            entry(&id('d'), "chat", "2", 2_000),
            entry(&id('9'), "chat", "3", 2_000),
            entry(&id('8'), "other", "4", 800),
        ];
        let value = project(&listed, &ledger, "chat", true, 1_500);
        let text = value.to_string();
        assert!(!text.contains("CLAIMHASH") && !text.contains("1.2.3.4"));
        let state = |c: char| {
            value["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|i| i["id"] == id(c))
                .cloned()
                .unwrap()
        };
        assert_eq!(state('a')["state"], "created");
        assert_eq!(state('a')["requested_in_this_chat"], false);
        assert_eq!(state('b')["state"], "created");
        assert_eq!(state('b')["joining"], true);
        assert_eq!(state('c')["state"], "used");
        assert_eq!(
            state('c')["device"],
            json!({"id":"key:new","name":"Laptop"})
        );
        assert_eq!(state('c')["label"], "Office Mac");
        assert_eq!(state('d')["state"], "revoked");
        assert_eq!(state('e')["state"], "expired");
        assert_eq!(state('f')["state"], "expired");
        assert_eq!(state('9')["state"], "expired");
        assert!(state('9')["note"].is_string());
        assert_eq!(state('8')["requested_in_this_chat"], false);
        // Default view: active invitations and this Chat's history only.
        let focused = project(&listed, &ledger, "chat", false, 1_500);
        let ids: Vec<_> = focused["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().chars().next().unwrap())
            .collect();
        assert!(
            ids.contains(&'a')
                && ids.contains(&'b')
                && ids.contains(&'c')
                && ids.contains(&'d')
                && ids.contains(&'9')
        );
        assert!(!ids.contains(&'e') && !ids.contains(&'f') && !ids.contains(&'8'));
    }

    #[test]
    fn ledger_is_bounded_and_secret_free() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..(LEDGER_LIMIT + 5) {
            record(
                dir.path(),
                entry(&format!("{index:032x}"), "s", &index.to_string(), 2_000),
            )
            .unwrap();
        }
        let entries = load(dir.path()).unwrap();
        assert_eq!(entries.len(), LEDGER_LIMIT);
        assert_eq!(
            entries.last().unwrap().invocation_id,
            (LEDGER_LIMIT + 4).to_string()
        );
        let mut late = entry(&"f".repeat(32), "s", "late", 999_999);
        late.created_at = 1_000 + LEDGER_RETENTION_SECONDS + 1;
        record(dir.path(), late).unwrap();
        assert_eq!(load(dir.path()).unwrap().len(), 1);
        let bytes = std::fs::read_to_string(path(dir.path())).unwrap();
        assert!(!bytes.contains("command") && !bytes.contains("secret"));
    }

    #[test]
    fn disabled_mesh_is_a_clear_refusal_not_a_dead_invitation() {
        let (code, message) = describe(&anyhow::anyhow!("mesh_disabled"));
        assert_eq!(code, "mesh_disabled");
        assert!(message.contains("switched off") && message.contains("zork mesh invite"));
        let (code, message) = describe(&anyhow::anyhow!("invite_expired_or_unknown"));
        assert_eq!(code, "invite_expired_or_unknown");
        assert!(!message.contains("restart"));
    }

    #[test]
    fn labels_are_short_single_lines() {
        assert_eq!(clean_label(Some("  ".into())).unwrap(), None);
        assert_eq!(
            clean_label(Some(" 书房 iMac ".into())).unwrap().as_deref(),
            Some("书房 iMac")
        );
        assert!(clean_label(Some("a\nb".into())).is_err());
        assert!(clean_label(Some("x".repeat(81))).is_err());
    }
}
