use super::*;

pub(super) async fn execute(state: &AppState, rpc: Rpc, local: bool) -> Result<Value> {
    if rpc.interrupt {
        return interrupt(state, rpc, local).await;
    }
    if management::is_operation(&rpc.request.op) {
        return management::execute(state, rpc, local).await;
    }
    let subject = rpc.subject.clone();
    let who = &subject;
    let request = &rpc.request;
    if !local {
        ensure!(
            zork_config::load_config(&state.config.data_root)?
                .mesh
                .peers
                .iter()
                .any(|p| p.origin == who.origin),
            "mcp_access_denied"
        );
    }
    if request.op == "catalog" {
        let query = request.query.as_deref().unwrap_or("").to_lowercase();
        ensure!(query.len() <= 256, "mcp_query_limit");
        let items = state
            .mcp
            .store
            .servers()?
            .into_iter()
            .filter(|s| {
                s.allows(who, local, None)
                    && format!("{} {}", s.config.name, s.config.description)
                        .to_lowercase()
                        .contains(&query)
            })
            .map(|s| state.mcp.descriptor(&s, &own_origin(state)))
            .collect::<Vec<_>>();
        return page(items, &request.cursor);
    }
    if matches!(request.op.as_str(), "status" | "cancel" | "read") {
        let id = field(&request.call_id)?;
        let (server, status, result) = state.mcp.store.call(id, who)?;
        let tool = state.mcp.store.call_tool(id)?;
        check_permission(state, who, local, &server, Some(&tool))?;
        if request.op == "cancel"
            && matches!(status.as_str(), "accepted" | "dispatching" | "running")
        {
            if let Some(active) = state.mcp.active.lock().expect("mcp active").get(id) {
                active.stop.send_replace(true);
            }
            return Ok(json!({"call_id":id,"state":status,"cancel_requested":true}));
        }
        if request.op == "read" {
            let text = result.context("mcp_result_unavailable")?;
            let start = request.offset.unwrap_or(0);
            ensure!(
                start <= text.len() && text.is_char_boundary(start),
                "mcp_invalid_offset"
            );
            let mut end = (start + 32 * 1024).min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            return Ok(
                json!({"call_id":id,"encoding":"json_text","offset":start,"data":&text[start..end],"next_offset":if end<text.len(){Some(end)}else{None}}),
            );
        }
        return Ok(if result.as_ref().is_some_and(|r| r.len() > 64 * 1024) {
            json!({"call_id":id,"state":status,"result_bytes":result.as_ref().map(String::len),"read_required":true})
        } else {
            json!({"call_id":id,"state":status,"result":result.map(|r|serde_json::from_str::<Value>(&r)).transpose()?})
        });
    }
    let reference = request.server_ref.as_ref().context("mcp_missing_server")?;
    valid_id(&reference.server_id)?;
    ensure!(
        reference.owner_origin == own_origin(state) || (local && reference.owner_origin == "local"),
        "mcp_wrong_owner"
    );
    let server = check_permission(
        state,
        who,
        local,
        &reference.server_id,
        request.tool.as_deref(),
    )?;
    if request.op == "inspect" {
        let mut details = state.mcp.descriptor(&server, &own_origin(state));
        details["config"] = serde_json::to_value(&server.config)?;
        details["items"] = json!([]);
        if !server.config.enabled {
            return Ok(details);
        }
        let slot = state.mcp.slots.clone().try_acquire_owned();
        let Ok(_slot) = slot else {
            ensure!(
                request.tool.is_none() && request.cursor.is_none(),
                "mcp_busy"
            );
            details["error"] = json!("mcp_busy");
            return Ok(details);
        };
        let session = state.mcp.connection(&server, who)?;
        let operation = async {
            let mut connection = session
                .connection
                .try_lock()
                .map_err(|_| anyhow::anyhow!("mcp_busy"))?;
            let result = async {
                // Taking ownership makes cancellation discard a half-read protocol session.
                let mut client = match connection.take() {
                    Some(client) => client,
                    None => runtime::Client::connect(&server.config.transport).await?,
                };
                let tools = client.list_tools().await?;
                *connection = Some(client);
                check(state, who, local, &server.id, request.tool.as_deref())?;
                let current = state.mcp.store.server(&server.id)?;
                ensure!(
                    current.revision == server.revision,
                    "mcp_definition_changed"
                );
                let mut items = Vec::new();
                for tool in tools.into_iter().filter(|t| {
                    t["name"]
                        .as_str()
                        .is_some_and(|n| server.allows(who, local, Some(n)))
                }) {
                    if let Some(name) = &request.tool {
                        if tool["name"] != *name {
                            continue;
                        }
                    }
                    let version = digest(&(&server.revision, &tool))?;
                    items.push(if request.tool.is_some() {
                        json!({"definition":tool,"binding_revision":version})
                    } else {
                        json!({"name":tool["name"],"description":tool["description"]})
                    });
                }
                if request.tool.is_some() {
                    ensure!(!items.is_empty(), "mcp_tool_not_found");
                }
                page(items, &request.cursor)
            }
            .await;
            state.mcp.healthy(&server.id, &result);
            if result.is_err() {
                *connection = None;
            }
            *session.used.lock().expect("mcp used") = Instant::now();
            result
        };
        let result = match tokio::time::timeout(Duration::from_secs(10), operation).await {
            Ok(result) => result,
            Err(_) => {
                let result = Err(anyhow::anyhow!("mcp_inspection_timeout"));
                state.mcp.healthy(&server.id, &result);
                result
            }
        };
        let current = check_permission(state, who, local, &server.id, request.tool.as_deref())?;
        ensure!(
            current.revision == server.revision,
            "mcp_definition_changed"
        );
        match result {
            Ok(page) => details
                .as_object_mut()
                .unwrap()
                .extend(page.as_object().unwrap().clone()),
            Err(error) if request.tool.is_none() && request.cursor.is_none() => {
                details["error"] = json!(safe_error(&error));
            }
            Err(error) => return Err(error),
        }
        details["availability"] =
            state.mcp.descriptor(&server, &own_origin(state))["availability"].clone();
        return Ok(details);
    }
    ensure!(request.op == "call", "mcp_invalid_operation");
    ensure!(server.config.enabled, "mcp_disabled");
    field(&request.tool)?;
    field(&request.binding_revision)?;
    ensure!(
        serde_json::to_vec(request)?.len() <= 128 * 1024,
        "mcp_request_limit"
    );
    ensure!(
        request.arguments.as_ref().is_some_and(Value::is_object),
        "mcp_invalid_arguments"
    );
    let fingerprint = digest(&(&who, &request))?;
    if let Some(id) = state
        .mcp
        .store
        .existing(who, &rpc.invocation_id, &fingerprint)?
    {
        return Ok(json!({"call_id":id,"state":state.mcp.store.call(&id,who)?.1}));
    }
    let slot = tokio::select! {
        permit = state.mcp.slots.clone().acquire_owned() => permit.map_err(|_|anyhow::anyhow!("mcp_stopping"))?,
        cancelled = state.mcp.store.cancellation(&rpc.subject,&rpc.invocation_id) => {
            cancelled?;
            return interrupt(state,rpc,local).await;
        }
    };
    // Capacity waits can outlive a grant or configuration revision.
    let current = check(state, who, local, &server.id, request.tool.as_deref())?;
    ensure!(
        current.revision == server.revision,
        "mcp_definition_changed"
    );
    let mut active = state.mcp.active.lock().expect("mcp active");
    let (id, fresh) = state.mcp.store.submit(
        who,
        &rpc.invocation_id,
        &server.id,
        field(&request.tool)?,
        &fingerprint,
    )?;
    if fresh {
        let (stop, mut stopped) = watch::channel(false);
        if state.mcp.store.cancelled(who, &rpc.invocation_id)? {
            stop.send_replace(true);
        }
        active.insert(
            id.clone(),
            Active {
                server: server.id.clone(),
                stop,
            },
        );
        let state = state.clone();
        let call_id = id.clone();
        tokio::spawn(async move {
            let _slot = slot;
            let mut cleanup = None;
            let result = {
                let cancelled = *stopped.borrow();
                let operation = async {
                    ensure!(!cancelled, "mcp_cancelled");
                    run(&state, &rpc, local, &server, &call_id, &mut cleanup).await
                };
                let policy_error = || {
                    check(
                        &state,
                        &rpc.subject,
                        local,
                        &server.id,
                        rpc.request.tool.as_deref(),
                    )
                    .and_then(|current| {
                        ensure!(
                            current.revision == server.revision,
                            "mcp_definition_changed"
                        );
                        Ok(())
                    })
                    .err()
                };
                let mut policy = state
                    .mcp
                    .store
                    .definitions()
                    .merge(state.db.realtime.listen(crate::realtime::MESH));
                let revoked = policy.until(policy_error);
                tokio::select! {
                    error=revoked=>Err(error.unwrap_or_else(|| anyhow::anyhow!("mcp_policy_unavailable"))),
                    result=operation=>result,
                    _=stopped.changed()=>Err(policy_error().unwrap_or_else(||anyhow::anyhow!("mcp_outcome_unknown"))),
                }
            };
            let mut process_state = None;
            if result.is_err() {
                if let Some(cleanup) = cleanup {
                    process_state = Some(if runtime::await_cleanup(cleanup).await.is_ok() {
                        "exited"
                    } else {
                        "unknown"
                    });
                }
            }
            if let Err(error) = result {
                let status = state
                    .mcp
                    .store
                    .call(&call_id, &rpc.subject)
                    .map(|(_, s, _)| s)
                    .unwrap_or_default();
                let terminal = if matches!(status.as_str(), "dispatching" | "running") {
                    "outcome_unknown"
                } else {
                    "failed"
                };
                let _ = state.mcp.store.transition(
                    &call_id,
                    terminal,
                    Some(json!({"error":safe_error(&error),"process_state":process_state,"effects_may_have_occurred":matches!(status.as_str(),"dispatching"|"running")})),
                );
                state
                    .mcp
                    .sessions
                    .lock()
                    .expect("mcp sessions")
                    .remove(&format!(
                        "{}:{}:{}",
                        server.id,
                        server.revision,
                        digest(&rpc.subject).unwrap_or_default()
                    ));
            }
            state
                .mcp
                .active
                .lock()
                .expect("mcp active")
                .remove(&call_id);
        });
    }
    Ok(json!({"call_id":id,"state":state.mcp.store.call(&id,who)?.1}))
}
async fn run(
    state: &AppState,
    rpc: &Rpc,
    local: bool,
    server: &Server,
    id: &str,
    cleanup: &mut Option<runtime::Cleanup>,
) -> Result<()> {
    let request = &rpc.request;
    let who = &rpc.subject;
    let session = state.mcp.connection(server, who)?;
    let mut connection = session.connection.lock().await;
    let mut client = match connection.take() {
        Some(client) => {
            *cleanup = client.cleanup();
            client
        }
        None => runtime::Client::connect_tracked(&server.config.transport, cleanup).await?,
    };
    let tools = client.list_tools().await?;
    let definition = tools
        .into_iter()
        .find(|t| t["name"].as_str() == request.tool.as_deref())
        .context("mcp_tool_not_found")?;
    ensure!(
        Some(digest(&(&server.revision, &definition))?.as_str())
            == request.binding_revision.as_deref(),
        "mcp_definition_changed"
    );
    let validator = jsonschema::validator_for(&definition["inputSchema"])
        .map_err(|_| anyhow::anyhow!("mcp_invalid_schema"))?;
    ensure!(
        validator.is_valid(
            request
                .arguments
                .as_ref()
                .context("mcp_invalid_arguments")?
        ),
        "mcp_invalid_arguments"
    );
    {
        let _policy = state.mcp.policy.lock().expect("mcp policy");
        let current = check(state, who, local, &server.id, request.tool.as_deref())?;
        ensure!(
            current.revision == server.revision,
            "mcp_definition_changed"
        );
        state.mcp.store.transition(id, "dispatching", None)?;
    }
    let result = client
        .request(
            "tools/call",
            json!({"name":request.tool,"arguments":request.arguments}),
        )
        .await?;
    check(state, who, local, &server.id, request.tool.as_deref())?;
    ensure!(result["content"].is_array(), "mcp_invalid_result");
    if result["isError"] != true {
        if let Some(schema) = definition.get("outputSchema") {
            let v = jsonschema::validator_for(schema)
                .map_err(|_| anyhow::anyhow!("mcp_invalid_schema"))?;
            ensure!(
                result
                    .get("structuredContent")
                    .is_some_and(|r| v.is_valid(r)),
                "mcp_invalid_result"
            );
        }
    }
    state.mcp.store.transition(
        id,
        if result["isError"] == true {
            "tool_error"
        } else {
            "succeeded"
        },
        Some(result),
    )?;
    state.mcp.healthy(&server.id, &Ok(json!(null)));
    *connection = Some(client);
    *session.used.lock().expect("mcp used") = Instant::now();
    Ok(())
}

async fn interrupt(state: &AppState, rpc: Rpc, local: bool) -> Result<Value> {
    let request = &rpc.request;
    if request.op != "call" {
        crate::node_access::manage(state, &rpc.subject, local)?;
        let _policy = state.mcp.policy.lock().expect("mcp policy");
        state.mcp.store.cancel(&rpc.subject, &rpc.invocation_id)?;
        return Ok(state
            .mcp
            .store
            .management_receipt(
                &rpc.subject,
                &rpc.invocation_id,
                &digest(&(&rpc.subject, request))?,
            )?
            .unwrap_or(json!({"state":"cancelled","effects_may_have_occurred":false})));
    }
    let reference = request.server_ref.as_ref().context("mcp_missing_server")?;
    check(
        state,
        &rpc.subject,
        local,
        &reference.server_id,
        request.tool.as_deref(),
    )?;
    let active = state.mcp.active.lock().expect("mcp active");
    state.mcp.store.cancel(&rpc.subject, &rpc.invocation_id)?;
    let hash = digest(&(&rpc.subject, request))?;
    let (id, fresh) = state.mcp.store.submit(
        &rpc.subject,
        &rpc.invocation_id,
        &reference.server_id,
        field(&request.tool)?,
        &hash,
    )?;
    if fresh {
        state.mcp.store.transition(
            &id,
            "cancelled",
            Some(json!({"effects_may_have_occurred":false})),
        )?;
    } else if let Some(active) = active.get(&id) {
        active.stop.send_replace(true);
    }
    Ok(json!({"call_id":id,"state":state.mcp.store.call(&id,&rpc.subject)?.1}))
}
