use super::*;

pub async fn tool(State(state): State<AppState>, Json(input): Json<ToolRequest>) -> Response {
    response(tool_inner(&state, input).await)
}
async fn tool_inner(state: &AppState, input: ToolRequest) -> Result<Value> {
    identity_ready(state)?;
    ensure!(
        !input.invocation_id.is_empty() && input.invocation_id.len() <= 256,
        "mcp_invalid_invocation"
    );
    let who = crate::node_access::subject(state, &input.session_id)
        .map_err(|_| anyhow::anyhow!("mcp_unknown_session"))?;
    let request = input.request;
    if request.op == "recover" {
        let mut calls = Vec::new();
        for (invocation, owner, request) in state.mcp.store.pending(&who)?.into_iter().take(4) {
            if let Ok(Ok(value)) = tokio::time::timeout(
                Duration::from_secs(5),
                route(
                    state,
                    &owner,
                    Rpc {
                        interrupt: false,
                        subject: who.clone(),
                        invocation_id: invocation.clone(),
                        request,
                    },
                ),
            )
            .await
            {
                if let Some(id) = management::receipt(&value) {
                    state.mcp.store.bind_route(&who, &invocation, id)?;
                    calls.push(value);
                }
            }
        }
        return Ok(
            json!({"calls":calls,"pending_delivery":!state.mcp.store.pending(&who)?.is_empty()}),
        );
    }
    if request.op == "search" {
        return search(state, &who, &request).await;
    }
    let owner = if matches!(request.op.as_str(), "status" | "cancel" | "read") {
        state.mcp.store.owner(&who, field(&request.call_id)?)?
    } else if matches!(request.op.as_str(), "install" | "list") {
        request.owner.clone().unwrap_or_else(|| "local".into())
    } else {
        request
            .server_ref
            .as_ref()
            .context("mcp_missing_server")?
            .owner_origin
            .clone()
    };
    if let Some(expected) = request.owner.as_deref() {
        let canonical = |value: &str| {
            if value == "local" {
                own_origin(state)
            } else {
                value.to_owned()
            }
        };
        ensure!(canonical(expected) == canonical(&owner), "mcp_wrong_owner");
    }
    let invocation = input.invocation_id;
    let first_delivery = if request.op == "call" || management::is_mutation(&request.op) {
        state.mcp.store.route(
            &who,
            &invocation,
            &digest(&(&who, &request))?,
            &owner,
            &request,
        )?
    } else {
        false
    };
    let result = route(
        state,
        &owner,
        Rpc {
            interrupt: false,
            subject: who.clone(),
            invocation_id: invocation.clone(),
            request: request.clone(),
        },
    )
    .await;
    let mut result = match result {
        Ok(value) => value,
        Err(error)
            if first_delivery
                && ((management::is_mutation(&request.op) && management::rejected(&error))
                    || (request.op == "call"
                        && matches!(
                            safe_error(&error).as_str(),
                            "mcp_access_denied"
                                | "mcp_disabled"
                                | "mcp_tool_not_allowed"
                                | "mcp_missing_server"
                                | "mcp_not_found"
                                | "mcp_wrong_owner"
                                | "mcp_invalid_id"
                                | "mcp_missing_parameter"
                                | "mcp_request_limit"
                                | "mcp_invalid_arguments"
                        ))) =>
        {
            state.mcp.store.reject_route(&who, &invocation)?;
            return Err(crate::tool_stream::Rejected(error).into());
        }
        Err(error) if request.op == "call" || management::is_mutation(&request.op) => {
            return Ok(
                json!({"pending_delivery":true,"error":safe_error(&error),"recovery":"Read the original invocation with history.list; do not submit a new call."}),
            )
        }
        Err(error) => return Err(error),
    };
    if request.op == "call" || management::is_mutation(&request.op) {
        state.mcp.store.bind_route(
            &who,
            &invocation,
            management::receipt(&result).context("mcp_invalid_receipt")?,
        )?;
    }
    result["target"] = json!(if owner == "local" {
        own_origin(state)
    } else {
        owner
    });
    Ok(result)
}

// Direct HTTP management uses the existing authenticated admin plane. Agent
// management is dispatched separately through authenticated Session/Mesh tools.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    expected_revision: String,
    config: ServerInput,
}
pub async fn admin_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !crate::admin::authorize(&headers, &state) {
        return crate::admin::unauthorized();
    }
    if let Err(error) = identity_ready(&state) {
        return response(Err(error));
    }
    response(state.mcp.store.servers().map(|servers|json!({"items":servers.iter().map(|s|json!({"server":state.mcp.descriptor(s,&own_origin(&state)),"tool_allowlist":s.config.tool_allowlist,"transport":match s.config.transport{runtime::Transport::Stdio{..}=>"stdio",runtime::Transport::Http{..}=>"http"}})).collect::<Vec<_>>()})))
}
pub async fn admin_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(config): Json<ServerInput>,
) -> Response {
    if !crate::admin::authorize(&headers, &state) {
        return crate::admin::unauthorized();
    }
    if let Err(error) = identity_ready(&state) {
        return response(Err(error));
    }
    let _policy = state.mcp.policy.lock().expect("mcp policy");
    response(
        state
            .mcp
            .store
            .save(config, None, None)
            .map(|s| s.descriptor(&own_origin(&state))),
    )
}
pub async fn admin_update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<Update>,
) -> Response {
    if !crate::admin::authorize(&headers, &state) {
        return crate::admin::unauthorized();
    }
    if let Err(error) = identity_ready(&state) {
        return response(Err(error));
    }
    let _policy = state.mcp.policy.lock().expect("mcp policy");
    response(
        state
            .mcp
            .store
            .save(input.config, Some(&id), Some(&input.expected_revision))
            .map(|s| {
                state.mcp.invalidate(&id);
                s.descriptor(&own_origin(&state))
            }),
    )
}
#[derive(Deserialize)]
pub struct Remove {
    expected_revision: String,
}
pub async fn admin_remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<Remove>,
) -> Response {
    if !crate::admin::authorize(&headers, &state) {
        return crate::admin::unauthorized();
    }
    if let Err(error) = identity_ready(&state) {
        return response(Err(error));
    }
    let _policy = state.mcp.policy.lock().expect("mcp policy");
    response(
        state
            .mcp
            .store
            .remove(&id, &input.expected_revision)
            .map(|()| {
                state.mcp.invalidate(&id);
                json!({"ok":true})
            }),
    )
}

pub async fn admin_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !crate::admin::authorize(&headers, &state) {
        return crate::admin::unauthorized();
    }
    if let Err(error) = identity_ready(&state) {
        return response(Err(error));
    }
    response(
        state
            .mcp
            .store
            .server(&id)
            .and_then(|server| Ok(serde_json::to_value(server)?)),
    )
}
pub async fn admin_probe(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !crate::admin::authorize(&headers, &state) {
        return crate::admin::unauthorized();
    }
    if let Err(error) = identity_ready(&state) {
        return response(Err(error));
    }
    let request = serde_json::from_value(
        json!({"op":"inspect","server_ref":{"owner_origin":own_origin(&state),"server_id":id}}),
    );
    response(match request {
        Ok(request) => execute(
            &state,
            Rpc {
                interrupt: false,
                subject: Subject {
                    origin: "local".into(),
                    agent: "admin".into(),
                    session: "probe".into(),
                },
                invocation_id: new_id(),
                request,
            },
            true,
        )
        .await
        .and_then(|value| {
            ensure!(value["availability"] != "disabled", "mcp_disabled");
            if let Some(error) = value["error"].as_str() {
                anyhow::bail!("{error}");
            }
            Ok(value)
        }),
        Err(_) => Err(anyhow::anyhow!("mcp_invalid_request")),
    })
}

pub async fn interrupt(State(state): State<AppState>, Json(input): Json<ToolRequest>) -> Response {
    let result = async {
        let who = crate::node_access::subject(&state, &input.session_id)?;
        state.mcp.store.cancel(&who, &input.invocation_id)?;
        let Some((owner, request)) = state
            .mcp
            .store
            .invocation_route(&who, &input.invocation_id)?
        else {
            return Ok(json!({"state":"cancelled","effects_may_have_occurred":false}));
        };
        let value = route(
            &state,
            &owner,
            Rpc {
                interrupt: true,
                subject: who.clone(),
                invocation_id: input.invocation_id.clone(),
                request,
            },
        )
        .await?;
        if let Some(id) = management::receipt(&value) {
            state.mcp.store.bind_route(&who, &input.invocation_id, id)?;
        }
        Ok::<_, anyhow::Error>(value)
    }
    .await;
    response(result)
}
