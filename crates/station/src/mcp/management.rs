//! Effective Mesh membership authorizes administration. Mutations and receipts commit together.
use super::*;

pub fn is_mutation(op: &str) -> bool {
    matches!(op, "install" | "update" | "uninstall")
}
pub fn is_operation(op: &str) -> bool {
    is_mutation(op) || op == "list"
}
pub fn receipt(value: &Value) -> Option<&str> {
    value["call_id"]
        .as_str()
        .or_else(|| value["operation_id"].as_str())
}
fn allowed(state: &AppState, who: &Subject, local: bool) -> Result<()> {
    crate::node_access::manage(state, who, local)
        .map_err(|_| anyhow::anyhow!("mcp_management_denied"))
}
use crate::node_access::executable;
fn prepare(state: &AppState, mut config: ServerInput) -> Result<ServerInput> {
    if let runtime::Transport::Stdio {
        command, cwd, env, ..
    } = &mut config.transport
    {
        *command = executable(command)
            .context("mcp_command_not_found_on_target")?
            .to_string_lossy()
            .into_owned();
        if cwd.is_empty() {
            let directory = state.config.data_root.join("mcp/workspace");
            std::fs::create_dir_all(&directory)?;
            *cwd = directory.canonicalize()?.to_string_lossy().into_owned();
        }
        ensure!(
            std::path::Path::new(cwd).is_dir(),
            "mcp_working_directory_not_found"
        );
        if let Ok(path) = std::env::var("PATH") {
            env.entry("PATH".into()).or_insert(path);
        }
    }
    config.validate()?;
    Ok(config)
}
fn update_config(state: &AppState, request: &Operation) -> Result<ServerInput> {
    let patch = request
        .config
        .as_ref()
        .and_then(Value::as_object)
        .context("mcp_invalid_config")?;
    ensure!(!patch.is_empty(), "mcp_invalid_config");
    let reference = request.server_ref.as_ref().context("mcp_missing_server")?;
    let previous = state.mcp.store.server(&reference.server_id)?;
    ensure!(
        request.expected_revision.as_deref() == Some(previous.revision.as_str()),
        "mcp_revision_conflict"
    );
    let mut config = serde_json::to_value(previous.config)?;
    config.as_object_mut().unwrap().extend(patch.clone());
    let config: ServerInput =
        serde_json::from_value(config).map_err(|_| anyhow::anyhow!("mcp_invalid_config"))?;
    // Toggling or renaming a broken installation must not require its old
    // executable to remain available. Resolve only a newly supplied transport.
    if patch.contains_key("transport") {
        prepare(state, config)
    } else {
        config.validate()?;
        Ok(config)
    }
}
pub async fn execute(state: &AppState, rpc: Rpc, local: bool) -> Result<Value> {
    allowed(state, &rpc.subject, local)?;
    let request = &rpc.request;
    if matches!(request.op.as_str(), "install" | "list") {
        ensure!(
            request
                .owner
                .as_ref()
                .is_none_or(|o| o == &own_origin(state) || (local && o == "local")),
            "mcp_wrong_owner"
        );
    } else {
        let reference = request.server_ref.as_ref().context("mcp_missing_server")?;
        ensure!(
            reference.owner_origin == own_origin(state)
                || (local && reference.owner_origin == "local"),
            "mcp_wrong_owner"
        );
        valid_id(&reference.server_id)?;
    }
    match request.op.as_str() {
        "list"=>page(state.mcp.store.servers()?.iter().map(|s|json!({"server":state.mcp.descriptor(s,&own_origin(state)),"enabled":s.config.enabled,"tool_allowlist":s.config.tool_allowlist})).collect(),&request.cursor),
        op if is_mutation(op)=>{
            let _policy=state.mcp.policy.lock().expect("mcp policy");
            allowed(state,&rpc.subject,local)?;
            // Resolve after checking for an existing receipt: a removed command
            // or changed PATH must not prevent recovery of a committed install.
            let fingerprint=digest(&(&rpc.subject,request))?;
            if let Some(result)=state.mcp.store.management_receipt(&rpc.subject,&rpc.invocation_id,&fingerprint)? {return Ok(result);}
            let config = match request.op.as_str() {
                "install" => Some(prepare(state, serde_json::from_value(request.config.clone().context("mcp_missing_config")?).map_err(|_| anyhow::anyhow!("mcp_invalid_config"))?)?),
                "update" => Some(update_config(state, request)?),
                _ => None,
            };
            let result=state.mcp.store.manage(&rpc.subject,&rpc.invocation_id,&fingerprint,&own_origin(state),request,config)?;
            if let Some(id)=result["server_ref"]["server_id"].as_str(){state.mcp.invalidate(id);}
            Ok(result)
        },
        _=>anyhow::bail!("mcp_invalid_operation"),
    }
}

pub fn rejected(error: &anyhow::Error) -> bool {
    matches!(
        safe_error(error).as_str(),
        "mcp_management_denied"
            | "mcp_revision_conflict"
            | "mcp_command_not_found_on_target"
            | "mcp_working_directory_not_found"
            | "mcp_missing_config"
            | "mcp_invalid_config"
            | "mcp_missing_grant"
            | "mcp_invalid_name"
            | "mcp_config_limit"
            | "mcp_invalid_id"
            | "mcp_wrong_owner"
            | "mcp_https_required"
            | "mcp_reserved_header"
            | "mcp_invalid_endpoint"
            | "mcp_absolute_command_and_cwd_required"
            | "mcp_not_found"
            | "mcp_management_history_limit"
            | "mcp_server_limit"
    )
}
