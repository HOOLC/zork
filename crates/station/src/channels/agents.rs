use super::*;
use crate::db::{
    agents::{AgentRole, NodeAgent},
    chats::configuration_revision,
};
use api::Rpc;
use futures_util::{stream, StreamExt};
use std::time::Duration;

pub(super) async fn discover(state: &AppState, who: &Subject, args: &Value) -> Result<Value> {
    let targets = node_access::targets(state)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let replies = stream::iter(targets.into_iter().map(|target| {
        let rpc = Rpc {
            subject: who.clone(),
            invocation_id: "discover".into(),
            tool: "agent.list".into(),
            arguments: args.clone(),
            prepared_key: String::new(),
            files: vec![],
            publication: None,
            interrupt: false,
        };
        async move {
            let result = tokio::time::timeout_at(deadline, api::route(state, &target, rpc)).await;
            (target, result)
        }
    }))
    .buffer_unordered(4)
    .collect::<Vec<_>>()
    .await;
    let mut nodes = Vec::new();
    let mut unavailable = Vec::new();
    for (target, result) in replies {
        match result {
            Ok(Ok(mut value)) if value["status"] != "rejected" => {
                value["target"] = json!(target);
                nodes.push(value);
            }
            Ok(Ok(value)) => unavailable.push(json!({"target":target,"error":value["error"]})),
            Ok(Err(error)) => {
                unavailable.push(json!({"target":target,"error":super::error(&error)}))
            }
            Err(_) => unavailable.push(json!({"target":target,"error":"node_timeout"})),
        }
    }
    nodes.sort_by_key(|value| value["target"].to_string());
    Ok(json!({"nodes":nodes,"unavailable_nodes":unavailable}))
}

fn summary(agent: &NodeAgent, manageable: bool) -> Result<Value> {
    Ok(
        json!({"id":agent.id,"name":agent.name,"avatar":agent.avatar,"role":agent.role,"manageable":manageable,"revision":configuration_revision(agent)?}),
    )
}
pub(super) async fn execute(
    state: &AppState,
    rpc: &Rpc,
    key: &str,
    object: &str,
    is_local: bool,
) -> Result<Value> {
    let args = &rpc.arguments;
    let manageable = node_access::manage(state, &rpc.subject, is_local).is_ok();
    match rpc.tool.as_str() {
        "agent.list" => {
            let scope = fingerprint(&("agents", args["query"].as_str()))?;
            let after = api::cursor(args, &scope)?;
            let query = args["query"].as_str().unwrap_or("").to_lowercase();
            let mut agents = state.db.node_agents()?;
            agents.sort_by(|a, b| a.id.cmp(&b.id));
            let agents = agents
                .into_iter()
                .filter(|a| {
                    after.as_ref().is_none_or(|p| a.id > *p)
                        && a.name.to_lowercase().contains(&query)
                })
                .take(api::limit(args))
                .collect::<Vec<_>>();
            let next = agents
                .last()
                .filter(|_| agents.len() == api::limit(args))
                .map(|a| api::encode_cursor(&scope, &a.id));
            Ok(
                json!({"items":agents.iter().map(|a|summary(a,manageable)).collect::<Result<Vec<_>>>()?,"next_cursor":next}),
            )
        }
        "agent.options" => {
            node_access::manage(state, &rpc.subject, is_local)?;
            let profiles = crate::agent::list_profiles(&state.agent).await?;
            let after = api::cursor(args, "agent-options")?
                .map(|s| s.parse::<usize>())
                .transpose()
                .context("invalid_chat_cursor")?
                .unwrap_or(0);
            let options=profiles.iter().filter(|p|p.auth_configured).flat_map(|p|p.models.iter().filter(|m|m.enabled&&m.limits.is_some()).map(move|m|json!({"profile_id":p.profile_id,"model":m.id,"thinking":m.thinking,"default_thinking":m.default_thinking}))).collect::<Vec<_>>();
            ensure!(after <= options.len(), "invalid_chat_cursor");
            let through = (after + api::limit(args)).min(options.len());
            Ok(
                json!({"items":&options[after..through],"next_cursor":(through<options.len()).then(||api::encode_cursor("agent-options",&through.to_string()))}),
            )
        }
        "agent.inspect" => {
            let id = field(args, "agent_id")?;
            let agent = state.db.node_agent(id)?.context("agent_not_found")?;
            let mut value = summary(&agent, manageable)?;
            if manageable {
                value["config"] = json!({"name":agent.name,"avatar":agent.avatar,"selection":{"profile_id":agent.profile_id,"model":agent.model,"thinking":agent.thinking},"instructions":agent.instructions,"skill_paths":agent.skill_paths,"allowed_leaders":agent.allowed_leaders});
                let mut sessions = Vec::new();
                for session in state.db.channel_agent_sessions(id)? {
                    if state.agent.service.contains(&session) {
                        let snapshot = state.agent.session_snapshot(&session).await?;
                        sessions.push(json!({"session_id":session,"status":snapshot.execution.status,"run":snapshot.execution.active_turn}));
                    }
                }
                value["sessions"] = json!(sessions);
            }
            Ok(value)
        }
        "agent.create" | "agent.update" => mutate(state, rpc, key, object, is_local).await,
        "agent.assign" => {
            ensure!(is_local, "work_owner_must_be_calling_node");
            let creator = state
                .db
                .node_agent(&rpc.subject.agent)?
                .context("agent_not_found")?;
            let value = crate::node::assign_chat(
                state,
                &creator,
                object,
                field(args, "worker_id")?,
                field(args, "goal")?,
            )
            .await?;
            state.db.finish_chat_outgoing(key, &value)?;
            Ok(value)
        }
        _ => anyhow::bail!("unknown_channel_tool"),
    }
}

pub(crate) async fn ensure_chat_runtime(
    state: &AppState,
    id: &str,
    target: &str,
    notice: &crate::db::chats::Notice,
) -> Result<String> {
    let definition = state.db.node_agent(id)?;
    // Long-term partners retain continuity across every channel.
    if definition
        .as_ref()
        .is_some_and(|agent| agent.role == AgentRole::Leader)
    {
        return ensure_runtime(state, id).await;
    }
    if let Some(work) = &notice.work {
        ensure!(
            work.assignment_id == format!("worker-{}", notice.message.chat_id),
            "chat_execution_mismatch"
        );
    }
    if let Some(session) = state
        .db
        .chat_execution(id, target, &notice.message.chat_id)?
    {
        let runtime = session.id.as_deref().context("chat_execution_pending")?;
        let definition = definition.context("agent_not_found")?;
        let session =
            crate::node::ensure_agent_session(state, &definition, &session.key, runtime).await?;
        return session.id.context("chat_execution_pending");
    }
    ensure!(notice.work.is_none(), "chat_execution_pending");
    ensure_runtime(state, id).await
}

pub(crate) async fn ensure_runtime(state: &AppState, id: &str) -> Result<String> {
    if let Some(key) = id.strip_prefix("session:") {
        let binding = state.db.get_binding(key)?.context("agent_not_found")?;
        return crate::agent::ensure_binding_session(&state.agent, &state.db, &binding).await;
    }
    let _guard = state.entries.lock_local_task(&format!("agent:{id}")).await;
    let agent = state.db.allocate_channel_agent(id)?;
    let key = agent
        .session_key
        .as_deref()
        .context("agent_runtime_missing")?;
    let runtime = agent
        .session_id
        .as_deref()
        .context("agent_runtime_missing")?;
    if state.agent.service.contains(runtime) {
        return Ok(runtime.into());
    }
    let parts = key.split(':').collect::<Vec<_>>();
    ensure!(parts.len() == 3, "invalid_agent_runtime");
    let session = state.db.ensure_session(crate::db::EnsureSession {
        connection_id: "local_gui",
        platform: "local_gui",
        channel_id: parts[1],
        root_thread_ts: parts[2],
        channel_type: Some("agent_control"),
        initiator_user_id: None,
        initiator_message_ts: None,
    })?;
    let selection = crate::agent::SessionSelection {
        profile_id: agent.profile_id.clone(),
        model: agent.model.clone(),
        thinking: agent.thinking.clone(),
    };
    crate::agent::ensure_allocated_session(
        &state.agent,
        &state.db,
        &crate::db::SessionBindingRow::Normal(session),
        runtime,
        &selection,
        &crate::node::agent_prompt(&agent),
    )
    .await?;
    Ok(runtime.into())
}

pub(crate) async fn open_home(state: &AppState, id: &str) -> Result<Value> {
    let _guard = state
        .entries
        .lock_local_task(&format!("agent-home:{id}"))
        .await;
    let agent = state.db.node_agent(id)?.context("agent_not_found")?;
    let channel = if let Some(channel) = state.db.agent_home(id)? {
        channel
    } else {
        let key = format!("agent-home-create:{id}");
        let receipt = state.db.chat_begin(&key, id)?;
        let channel = if let Some(value) = receipt.result {
            serde_json::from_value(value)?
        } else {
            state
                .db
                .create_chat(&key, &receipt.object_id, &agent.name)?
        };
        let key = format!("agent-home-subscribe:{id}");
        let receipt = state.db.chat_begin(&key, &channel.chat_id)?;
        if receipt.result.is_none() {
            state.db.update_chat_preferences(
                &key,
                &channel.chat_id,
                id,
                &zork_client_types::chat::UpdatePreferences {
                    changes: zork_client_types::chat::PreferenceChanges {
                        subscribed: Some(true),
                        ..Default::default()
                    },
                    expected_revision: None,
                    start: None,
                },
            )?;
        }
        state.db.set_agent_home(id, &channel.chat_id)?;
        channel
    };
    state.db.record_chat_source(id, "local")?;
    // An empty home channel need not start or allocate a model context.
    Ok(json!({"agent":agent,"session_id":channel.chat_id,"chat_id":channel.chat_id}))
}

/// Shared configuration validation for tool and confirmed-message entry points.
pub(crate) async fn apply_configuration(
    state: &AppState,
    agent: &mut NodeAgent,
    fields: &Value,
) -> Result<()> {
    if let Some(name) = fields["name"].as_str() {
        agent.name = name.trim().into();
    }
    if let Some(avatar) = fields.get("avatar") {
        agent.avatar = serde_json::from_value(avatar.clone())?;
    }
    if let Some(instructions) = fields["instructions"].as_str() {
        agent.instructions = instructions.into();
    }
    if let Some(paths) = fields.get("skill_paths") {
        agent.skill_paths = serde_json::from_value(paths.clone())?;
    }
    if let Some(allowed) = fields.get("allowed_leaders") {
        agent.allowed_leaders = serde_json::from_value(allowed.clone())?;
        crate::node::validate_agent_grants(
            state,
            &state.db.node_agents()?,
            &agent.allowed_leaders,
        )?;
    }
    if let Some(selection) = fields.get("selection") {
        let selection: crate::agent::SessionSelection = serde_json::from_value(selection.clone())?;
        let profiles = crate::agent::list_profiles(&state.agent).await?;
        let selection = crate::agent::resolve_selection(&profiles, &selection)
            .context("agent_selection_unavailable")?;
        agent.profile_id = selection.profile_id;
        agent.model = selection.model;
        agent.thinking = selection.thinking;
    }
    ensure!(
        !agent.name.is_empty()
            && agent.name.len() <= 160
            && !agent.name.chars().any(char::is_control),
        "invalid_agent_name"
    );
    ensure!(
        agent
            .avatar
            .as_deref()
            .is_none_or(crate::node::valid_avatar),
        "invalid_agent_avatar"
    );
    zork_config::validate_skill_paths(&agent.skill_paths)?;
    // Validate source resolution before accepting a configuration that all
    // of this Agent's existing execution contexts will consume.
    zork_config::load_config(&state.config.data_root)?
        .skills
        .sources(&state.config.data_root, &agent.skill_paths)?;
    Ok(())
}

fn configuration(agent: &NodeAgent) -> Value {
    json!({"name":agent.name,"avatar":agent.avatar,"selection":{"profile_id":agent.profile_id,"model":agent.model,"thinking":agent.thinking},
        "instructions":agent.instructions,"skill_paths":agent.skill_paths,"allowed_leaders":agent.allowed_leaders,"role":agent.role})
}
fn merge_configuration(base: &mut Value, changes: &Value) {
    for (key, value) in changes.as_object().into_iter().flatten() {
        if value.is_object() && base[key].is_object() {
            merge_configuration(&mut base[key], value);
        } else {
            base[key] = value.clone();
        }
    }
}
fn empty_agent(id: &str, actor: &str) -> NodeAgent {
    NodeAgent {
        id: id.into(),
        name: String::new(),
        avatar: Some(crate::node::default_avatar(id).into()),
        role: AgentRole::Worker,
        profile_id: String::new(),
        model: String::new(),
        thinking: String::new(),
        instructions: String::new(),
        skill_paths: vec![],
        allowed_leaders: vec![actor.into()],
        session_key: None,
        session_id: None,
    }
}
async fn review_choices(
    state: &AppState,
    subject: &Subject,
    values: &Value,
) -> Result<zork_agent_gateway_tools::agent_configuration::ReviewChoices> {
    use zork_agent_gateway_tools::agent_configuration::{selection_value, ReviewChoices};
    use zork_client_types::interaction::Choice;
    let profiles = crate::agent::list_profiles(&state.agent).await?;
    let mut choices = ReviewChoices::default();
    for profile in profiles.iter().filter(|p| p.auth_configured) {
        let provider_name = zork_profile::providers::get(&profile.provider)
            .map(|p| p.info().label)
            .unwrap_or(&profile.provider);
        for model in profile
            .models
            .iter()
            .filter(|m| m.enabled && m.limits.is_some())
        {
            for thinking in &model.thinking {
                let label = format!(
                    "{} · {}",
                    model.id,
                    profile.name.as_deref().unwrap_or(provider_name)
                );
                choices.models.push(Choice {
                    value: selection_value(&json!({"profile_id":profile.profile_id,"model":model.id,"thinking":thinking})),
                    label: if thinking == "off" { label } else { format!("{label} · {thinking}") },
                });
            }
        }
    }
    let agents = state.db.node_agents()?;
    let requester = actor(subject);
    for value in values["allowed_leaders"].as_array().into_iter().flatten() {
        let Some(id) = value.as_str() else { continue };
        let label = if id == requester {
            "本次请求的发起者".into()
        } else if let Some(agent) = agents.iter().find(|a| {
            id == a.id
                || id == format!("local/{}", a.id)
                || id == format!("{}/{}", node_access::identity(state), a.id)
        }) {
            agent.name.clone()
        } else {
            format!("{}（名称不可用）", id.rsplit('/').next().unwrap_or(id))
        };
        choices.leaders.push(Choice {
            value: id.into(),
            label,
        });
    }
    for value in values["skill_paths"].as_array().into_iter().flatten() {
        let Some(path) = value.as_str() else { continue };
        let file = std::path::Path::new(path);
        let name = if file.file_name().is_some_and(|n| n == "SKILL.md") {
            file.parent().and_then(|p| p.file_name())
        } else {
            file.file_name()
        };
        choices.skills.push(Choice {
            value: path.into(),
            label: name
                .and_then(|n| n.to_str())
                .unwrap_or("已配置的技能")
                .into(),
        });
    }
    Ok(choices)
}

async fn candidate(state: &AppState, rpc: &Rpc, id: &str, fields: &Value) -> Result<NodeAgent> {
    let creating = rpc.tool == "agent.create";
    let mut agent = if creating {
        empty_agent(id, &actor(&rpc.subject))
    } else {
        state.db.node_agent(id)?.context("agent_not_found")?
    };
    let schema = zork_agent_gateway_tools::agent_configuration::schema(creating, creating);
    ensure!(
        jsonschema::validator_for(&schema)?.is_valid(fields),
        "invalid_agent_configuration"
    );
    if creating {
        agent.role = fields
            .get("role")
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()?
            .unwrap_or(AgentRole::Worker);
    }
    let mut fields = fields.clone();
    if !creating && fields["selection"].is_object() {
        let mut selection = configuration(&agent)["selection"].clone();
        merge_configuration(&mut selection, &fields["selection"]);
        fields["selection"] = selection;
    }
    apply_configuration(state, &mut agent, &fields).await?;
    if creating && agent.avatar.as_deref().is_none_or(str::is_empty) {
        agent.avatar = Some(crate::node::default_avatar(id).to_string());
    }
    ensure!(
        !agent.profile_id.is_empty() && !agent.model.is_empty() && !agent.thinking.is_empty(),
        "agent_selection_required"
    );
    Ok(agent)
}
async fn mutate(
    state: &AppState,
    rpc: &Rpc,
    key: &str,
    object: &str,
    is_local: bool,
) -> Result<Value> {
    node_access::manage(state, &rpc.subject, is_local)?;
    let creating = rpc.tool == "agent.create";
    let id = if creating {
        object
    } else {
        field(&rpc.arguments, "agent_id")?
    };
    let original = if creating {
        &rpc.arguments["config"]
    } else {
        &rpc.arguments["changes"]
    };
    let initial = if creating {
        empty_agent(id, &actor(&rpc.subject))
    } else {
        state.db.node_agent(id)?.context("agent_not_found")?
    };
    // Creating an Agent is a user decision: a caller may prefill the
    // parameters but never creates one silently. Updates still run directly
    // unless the caller asks for review.
    let needs_input = creating || rpc.arguments["review"] == true;
    let mut reviewed = None;
    let result: Result<Value> = async {
        let fields = if needs_input {
            let request = if let Some(request) =
                state
                    .db
                    .business_card_for_call(&rpc.subject, &rpc.invocation_id, "parameters")?
            {
                request
            } else {
                let mut values = configuration(&initial);
                merge_configuration(&mut values, original);
                let choices = review_choices(state, &rpc.subject, &values).await?;
                let mut spec = zork_agent_gateway_tools::agent_configuration::form(
                    creating, &values, original, choices,
                );
                if let zork_client_types::interaction::Request::AgentConfiguration {
                    name, ..
                } = &mut spec
                {
                    *name = initial.name.clone();
                }
                crate::agent_configuration::begin(state, &rpc.subject, &rpc.invocation_id, &spec)?
            };
            reviewed = Some(request.request_id.clone());
            loop {
                let submitted =
                    crate::agent_configuration::next_submission(state, &request.request_id).await?;
                let parsed = zork_agent_gateway_tools::agent_configuration::submitted(
                    creating,
                    original,
                    &request.request,
                    &submitted.response.values,
                );
                let validated = async {
                    let fields = parsed?;
                    candidate(state, rpc, id, &fields).await?;
                    Ok::<_, anyhow::Error>(fields)
                }
                .await;
                match validated {
                    Ok(fields) => {
                        state.db.acknowledge_configuration_response(
                            &request.request_id,
                            &submitted,
                            json!({"values":submitted.response.values}),
                        )?;
                        break fields;
                    }
                    Err(error) => state.db.reject_configuration_response(
                        &request.request_id,
                        &submitted.response.response_id,
                        &error.to_string(),
                    )?,
                }
            }
        } else {
            original.clone()
        };
        // User waiting never owns the Agent resource lock or an open transaction.
        let _guard = state.entries.lock_local_task(&format!("agent:{id}")).await;
        node_access::manage(state, &rpc.subject, is_local)?;
        let agent = candidate(state, rpc, id, &fields).await?;
        state.db.save_channel_agent(
            key,
            &agent,
            if creating {
                None
            } else {
                Some(field(&rpc.arguments, "expected_revision")?)
            },
            reviewed.as_deref(),
        )
    }
    .await;
    if let (Err(error), Some(request)) = (&result, &reviewed) {
        let record = state.db.business_card(request)?;
        if let Some(resolution) = record.result.as_ref().filter(|r| r.outcome.terminal()) {
            return Ok(
                json!({"status":if resolution.outcome==zork_client_types::interaction::Outcome::Cancelled{"cancelled"}else{"rejected"},"request_id":request,"result":resolution}),
            );
        }
        let attempt = record
            .submission
            .as_ref()
            .map(|s| s.response.response_id.as_str())
            .unwrap_or("operation");
        state.db.finish_configuration_review(
            request,
            attempt,
            zork_client_types::interaction::Outcome::Failed,
            json!({"error":error.to_string()}),
            "agent",
        )?;
    }
    result
}
