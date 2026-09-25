//! Shared identity and node-management boundary for device tools.
use crate::state::AppState;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Subject {
    pub origin: String,
    pub agent: String,
    pub session: String,
}

/// Request identity is independent of JSON object insertion order, including
/// builds where serde_json's preserve_order feature is enabled.
pub fn fingerprint(value: &impl Serialize) -> Result<String> {
    fn canonical(value: Value) -> Value {
        match value {
            Value::Object(fields) => Value::Object(
                fields
                    .into_iter()
                    .map(|(key, value)| (key, canonical(value)))
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(values) => Value::Array(values.into_iter().map(canonical).collect()),
            value => value,
        }
    }
    Ok(
        blake3::hash(&serde_json::to_vec(&canonical(serde_json::to_value(
            value,
        )?))?)
        .to_hex()
        .to_string(),
    )
}
pub fn identity(state: &AppState) -> String {
    state
        .mesh
        .get()
        .map(|m| m.origin().to_owned())
        .unwrap_or_else(|| "local".into())
}
pub fn ready(state: &AppState) -> Result<()> {
    ensure!(
        !zork_config::load_config(&state.config.data_root)?
            .mesh
            .enabled
            || state.mesh.get().is_some(),
        "node_starting"
    );
    Ok(())
}
pub fn subject(state: &AppState, session: &str) -> Result<Subject> {
    let binding = state
        .db
        .get_binding_by_id(session)?
        .context("node_unknown_session")?;
    let agent = state
        .db
        .agent_id_for_session(session)?
        .unwrap_or_else(|| format!("session:{}", binding.key()));
    Ok(Subject {
        origin: "local".into(),
        agent,
        session: session.into(),
    })
}
pub fn manage(state: &AppState, who: &Subject, local: bool) -> Result<()> {
    if !local {
        let mesh = zork_config::load_config(&state.config.data_root)?.mesh;
        ensure!(
            mesh.enabled && mesh.peers.iter().any(|p| p.origin == who.origin),
            "node_management_denied"
        );
    }
    Ok(())
}
pub fn remote(who: &Subject, peer: &str) -> Result<()> {
    ensure!(
        who.origin == peer
            && !who.agent.is_empty()
            && who.agent.len() <= 256
            && !who.session.is_empty()
            && who.session.len() <= 256,
        "node_invalid_subject"
    );
    Ok(())
}
pub fn targets(state: &AppState) -> Result<Vec<String>> {
    let config = zork_config::load_config(&state.config.data_root)?.mesh;
    let mut targets = vec![identity(state)];
    if config.enabled {
        targets.extend(config.peers.into_iter().map(|p| p.origin));
    }
    targets.sort();
    targets.dedup();
    Ok(targets)
}
/// This Mesh's devices with the display names every client shows and the
/// machine names they registered with. Empty outside a Mesh.
pub fn device_names(state: &AppState) -> Vec<zork_config::membership::ResolvedName> {
    let root = &state.config.data_root;
    let Some(group) = zork_config::load_config(root)
        .ok()
        .and_then(|config| config.mesh.group)
    else {
        return vec![];
    };
    let names = zork_config::membership::load_names(root).ok().flatten();
    zork_config::membership::resolve(&group, names.as_ref())
}

/// Adds `display_name` and `machine_name` to an Agent-facing entry whose
/// `field` holds a Station identity.
pub fn name_entry(
    devices: &[zork_config::membership::ResolvedName],
    value: &mut Value,
    field: &str,
) {
    let Some(origin) = value[field].as_str() else {
        return;
    };
    if let Some(device) = devices.iter().find(|d| d.origin == origin) {
        value["display_name"] = json!(device.display);
        value["machine_name"] = json!(device.machine);
    }
}

/// Accepts a Station identity, `local`, or a device's display or machine
/// name, and returns the identity tools route to.
pub fn resolve_target(state: &AppState, target: &str) -> Result<String> {
    let target = target.trim();
    if target == "local" || target.starts_with("key:") {
        return Ok(target.into());
    }
    let devices = device_names(state);
    if let Ok(device) = zork_config::membership::find_device(&devices, target) {
        return Ok(device.origin.clone());
    }
    // Manually paired Stations outside the Mesh keep their own names.
    let config = zork_config::load_config(&state.config.data_root)?.mesh;
    let peers: Vec<_> = config
        .peers
        .iter()
        .filter(|p| p.name.trim().eq_ignore_ascii_case(target))
        .collect();
    match peers.as_slice() {
        [peer] => Ok(peer.origin.clone()),
        [] if config.name.trim().eq_ignore_ascii_case(target) => Ok(identity(state)),
        [] => anyhow::bail!("device_unknown_target"),
        _ => anyhow::bail!("device_ambiguous_target"),
    }
}

pub fn executable(name: &str) -> Option<PathBuf> {
    let usable = |p: &Path| {
        if !p.is_file() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            p.metadata()
                .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
        }
        #[cfg(not(unix))]
        {
            true
        }
    };
    if Path::new(name).is_absolute() {
        return usable(Path::new(name)).then(|| PathBuf::from(name));
    }
    if name.contains(['/', '\\']) || name.is_empty() {
        return None;
    }
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.is_absolute())
        .map(|p| p.join(name))
        .find(|p| usable(p))
}
pub fn environment(state: &AppState) -> Value {
    let commands = ["sh", "node", "npm", "npx", "python3", "uv", "uvx", "git"]
        .into_iter()
        .filter_map(|name| executable(name).map(|path| (name.to_owned(), json!(path))))
        .collect::<serde_json::Map<_, _>>();
    let own = identity(state);
    let display = device_names(state)
        .into_iter()
        .find(|d| d.origin == own)
        .map(|d| d.display);
    json!({"target":own,"owner":own,"name":zork_config::load_config(&state.config.data_root).map(|c|c.mesh.name).unwrap_or_default(),"display_name":display,"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"commands":commands,"managed_workspace_root":zork_config::files_root(&state.config.data_root).join("device-workspaces")})
}
