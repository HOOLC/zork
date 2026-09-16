//! Named device and MCP capabilities use the ordinary tool lifecycle.
use super::*;

struct Named {
    base: String,
    http: reqwest::Client,
    name: String,
    fields: Vec<String>,
}

fn add(
    registry: &Arc<ToolRegistry>,
    base: &str,
    name: &str,
    description: &str,
    properties: Value,
    required: Vec<&str>,
    http: &reqwest::Client,
) -> anyhow::Result<()> {
    let compatibility: Arc<dyn ToolCompatibility> = Arc::new(history::Results);
    let fields = properties
        .as_object()
        .expect("tool properties")
        .keys()
        .cloned()
        .collect();
    let owned = name.to_owned();
    let activity_name = owned.clone();
    registry.register(Arc::new(ToolInstance::new(ToolContract{name:owned.clone(),version:ToolVersion::new(if name.starts_with("mcp.") { "node-tools-6" } else { "node-tools-4" })?,initial_description:description.into(),detailed_description:format!("{description} target is the exact Gateway identity returned by device.list, not a display name. Omit target for this execution node. Identity and delivery deduplication come from ToolContext. Operations complete through ordinary tool completion events. Use tool.cancel with the invocation ID to interrupt pending work. Live output is available at .zork/live-<invocation_id>.log in this session workspace; read it with file.read. The completion result includes output_path. Do not repeat effects when the result says outcome_unknown."),input_schema:json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})},Arc::new(Named{base:base.into(),http:http.clone(),name:owned,fields}),compatibility)?.with_activity(move|args|activity(&activity_name,args))));
    Ok(())
}
pub fn register(
    registry: &Arc<ToolRegistry>,
    base: &str,
    http: &reqwest::Client,
) -> anyhow::Result<()> {
    let string = || json!({"type":"string","minLength":1});
    let target = json!({"target":string()});
    for (name,description,extra,required) in [
        ("device.list","Discover Mesh Gateways, their identity, environment and connectivity. Use this before selecting a device.",json!({}),vec![]),
        ("device.inspect","Inspect the target Gateway's OS, commands and managed workspace location.",json!({}),vec![]),
    ]{let mut props=target.clone();props.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());add(registry,base,name,description,props,required,http)?;}
    let properties = mcp::properties();
    for (op,description,fields,required) in [
        ("list","List MCP installations on a target node, including disabled installations and current configuration revisions.",vec!["cursor"],vec![]),
        ("install","Install MCP configuration on a selected device. Prepare dependencies on the target node, then use inspect to verify connectivity. Mesh members share its enabled tools.",vec!["config"],vec!["config"]),
        ("update","Update the supplied MCP configuration fields using the current expected_revision. Omitted fields are preserved; a supplied transport replaces the entire transport. Set config.enabled to enable or disable the installation.",vec!["server_id","config","expected_revision"],vec!["server_id","config","expected_revision"]),
        ("uninstall","Uninstall an MCP server using its current expected_revision.",vec!["server_id","expected_revision"],vec!["server_id","expected_revision"]),
        ("search","Search usable MCP services across Mesh; optionally filter target. Results include target and server_id.",vec!["query","cursor"],vec![]),
        ("inspect","Inspect MCP configuration, credential references and connection status on the selected target/server_id. Enabled installations are checked live and return tool summaries; supply tool for its TypeScript parameters and binding_revision. Disabled, busy or unreachable installations still return their configuration so they can be updated. Secret values are never resolved into the result.",vec!["server_id","tool","cursor"],vec!["server_id"]),
        ("call","Call the inspected MCP tool with its binding_revision; completes with the actual MCP result. mcp_disabled means the server is disabled; mcp_tool_not_allowed means the tool is excluded; mcp_definition_changed means inspect the current definition before a new call. Only not_dispatched confirms no dispatch. An interrupted call may have effects even when the error identifies a policy change; uncertain outcomes must not be repeated.",vec!["server_id","tool","binding_revision","arguments"],vec!["server_id","tool","binding_revision","arguments"]),
    ]{let mut props=target.clone();for field in fields{props[field]=properties.get(field).cloned().unwrap_or_else(string);}if op=="update"{props["config"].as_object_mut().unwrap().remove("required");props["config"]["minProperties"]=json!(1);}add(registry,base,&format!("mcp.{op}"),description,props,required,http)?;}
    Ok(())
}
impl ToolImplementation for Named {
    fn execute<'a>(
        &'a self,
        context: &'a ToolContext,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = ToolExecution> + Send + 'a>> {
        Box::pin(async move {
            let mut execution = self.execution(context, args, false).await;
            execution.result_schema_version = 2;
            execution
        })
    }
    fn cancel<'a>(
        &'a self,
        context: &'a ToolContext,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Option<ToolExecution>> + Send + 'a>> {
        Box::pin(async move {
            if self.mutation() {
                {
                    let mut result = self.execution(context, args, true).await;
                    result.result_schema_version = 2;
                    Some(result)
                }
            } else {
                None
            }
        })
    }
}
fn node_execution(name: &str, value: Value) -> ToolExecution {
    let _ = name;
    let failed = value["pending_delivery"] == true
        || matches!(
            value["state"].as_str(),
            Some("failed" | "outcome_unknown" | "not_dispatched")
        );
    let mut result = ToolExecution::success(value);
    if failed {
        result.outcome = ToolOutcome::Failed;
    }
    result
}

impl Named {
    fn mutation(&self) -> bool {
        matches!(
            self.name.as_str(),
            "device.exec" | "mcp.call" | "mcp.install" | "mcp.update" | "mcp.uninstall"
        )
    }
    async fn execution(
        &self,
        context: &ToolContext,
        args: &Value,
        interrupt: bool,
    ) -> ToolExecution {
        let result = async {
            let value = self.request(context, args, interrupt).await?;
            if self.mutation() {
                self.complete(context, value, interrupt).await
            } else {
                Ok(value)
            }
        }
        .await;
        match result {
            Ok(mut value) => {
                present_result(&self.name, &mut value);
                if self.mutation() {
                    if let Some(result) = value["result"].as_object_mut() {
                        result.remove("read_with");
                    }
                    if let Some(object) = value.as_object_mut() {
                        object.remove("operation_id");
                        object.remove("call_id");
                        object.remove("pending_delivery");
                    }
                }
                let mut execution = if self.name.starts_with("mcp.") {
                    mcp::execution_for(self.name.trim_start_matches("mcp."), value)
                } else {
                    node_execution(&self.name, value)
                };
                if self.mutation() && execution.data["state"] == "cancelled" {
                    execution.outcome = ToolOutcome::Cancelled;
                }
                if self.name == "mcp.call"
                    && serde_json::to_vec(&execution.data)
                        .is_ok_and(|bytes| bytes.len() > 64 * 1024)
                {
                    execution.data["result"] =
                        json!({"summary":"Full result is in output_path; use file.read."});
                }
                execution
            }
            Err(error) if error.is::<DeliveryRejected>() => ToolExecution {
                images: vec![],
                outcome: ToolOutcome::Failed,
                data: json!({"state":"not_dispatched","effects_may_have_occurred":false,"error":error.to_string()}),
                result_schema_version: 1,
                knowledge: None,
            },
            Err(error) => ToolExecution {
                images: vec![],
                outcome: ToolOutcome::Failed,
                data: if self.mutation() {
                    json!({"state":"outcome_unknown","error":error.to_string(),"effects_may_have_occurred":true,"instruction":"Do not repeat this operation; its outcome could not be confirmed."})
                } else {
                    json!({"error":error.to_string()})
                },
                result_schema_version: 1,
                knowledge: None,
            },
        }
    }
    async fn request(
        &self,
        context: &ToolContext,
        args: &Value,
        interrupt: bool,
    ) -> anyhow::Result<Value> {
        if !interrupt {
            if let Some(args) = args.as_object() {
                if let Some(field) = args.keys().find(|field| !self.fields.contains(field)) {
                    return Err(DeliveryRejected(format!("Invalid argument `{field}` for {}. Allowed fields: {}. Use tool.help for the parameter definition.", self.name, self.fields.join(", "))).into());
                }
            }
        }
        let (url, body) = if let Some(op) = self.name.strip_prefix("mcp.") {
            let mut request = args.clone();
            request["op"] = json!(op);
            let map = request
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("Invalid MCP arguments"))?;
            let target = map.remove("target");
            if let Some(server) = map.remove("server_id") {
                map.insert("server_ref".into(),json!({"owner_origin":target.clone().unwrap_or(json!("local")),"server_id":server}));
            }
            if let Some(target) = target {
                map.insert("owner".into(), target);
            }
            if let Some(id) = map.remove("operation_id") {
                map.insert("call_id".into(), id);
            }
            (
                "/v1/mcp",
                json!({"session_id":context.session_id,"invocation_id":context.invocation_id,"request":request}),
            )
        } else {
            (
                "/v1/node-tools",
                json!({"session_id":context.session_id,"invocation_id":context.invocation_id,"tool":self.name,"arguments":args}),
            )
        };
        let suffix = if interrupt { "/interrupt" } else { "" };
        let mut attempt = 0;
        let mut value = loop {
            let result: anyhow::Result<Value> = async {
                let response = self
                    .http
                    .post(format!("{}{url}{suffix}", self.base))
                    .json(&body)
                    .send()
                    .await?;
                let status = response.status();
                let text = response.text().await?;
                if self.name.starts_with("mcp.") && status == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
                    return Err(DeliveryRejected(format!("Invalid arguments for {}. Check required fields and value types with tool.help. Allowed fields: {}.", self.name, self.fields.join(", "))).into());
                }
                decode_response(status, &text)
            }
            .await;
            let retry = matches!(&result,Ok(value) if value["pending_delivery"] == true)
                || result
                    .as_ref()
                    .err()
                    .is_some_and(|error| error.downcast_ref::<reqwest::Error>().is_some());
            if !self.mutation() || !retry || attempt == 3 {
                break result?;
            }
            tokio::time::sleep(Duration::from_millis(250 << attempt)).await;
            attempt += 1;
        };
        if self.name.starts_with("mcp.") {
            normalize_mcp(&mut value);
            if let Some(target) = args.get("target") {
                if value.get("target").is_none() {
                    value["target"] = target.clone();
                }
            }
        }
        Ok(value)
    }
    async fn complete(
        &self,
        context: &ToolContext,
        receipt: Value,
        interrupt: bool,
    ) -> anyhow::Result<Value> {
        use base64::Engine;
        use std::io::{Read, Seek, SeekFrom, Write};
        if self.name.starts_with("mcp.") && self.name != "mcp.call" {
            return Ok(receipt);
        }
        let Some(id) = receipt["operation_id"]
            .as_str()
            .or(receipt["call_id"].as_str())
        else {
            anyhow::ensure!(
                receipt["pending_delivery"] != true,
                "Tool delivery could not be confirmed"
            );
            return Ok(receipt);
        };
        let domain = if self.name.starts_with("mcp.") {
            "mcp"
        } else {
            "device"
        };
        anyhow::ensure!(
            !context.invocation_id.contains(['/', '\\']),
            "Invalid invocation ID"
        );
        let relative = format!(".zork/live-{}.log", context.invocation_id);
        let path = std::path::Path::new(&context.workspace).join(&relative);
        std::fs::create_dir_all(
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("Invalid output path"))?,
        )?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(!interrupt)
            .open(&path)?;
        let mut offset = if interrupt { file.metadata()?.len() } else { 0 };
        file.seek(SeekFrom::Start(offset))?;
        for attempt in 0..4 {
            let result: anyhow::Result<Value> = async {
                let mut response = self
            .http
            .post(format!("{}/v1/tools/watch", self.base))
            .json(&json!({"session_id":context.session_id,"domain":domain,"id":id,"offset":offset}))
            .send()
            .await?
            .error_for_status()?;
                let mut buffer = Vec::new();
                while let Some(chunk) = response.chunk().await? {
                    buffer.extend_from_slice(&chunk);
                    anyhow::ensure!(
                        buffer.len() <= 16 * 1024 * 1024,
                        "Tool stream frame too large"
                    );
                    while let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
                        let event: Value = serde_json::from_slice(&buffer[..end])?;
                        buffer.drain(..=end);
                        anyhow::ensure!(
                            event.get("error").is_none(),
                            "Tool stream failed: {}",
                            event["error"]
                        );
                        let mut value = event["value"].clone();
                        if let Some(encoded) = value["chunk"]["base64"].as_str() {
                            anyhow::ensure!(
                                value["chunk"]["offset"].as_u64() == Some(offset),
                                "Tool output offset mismatch"
                            );
                            let bytes =
                                base64::engine::general_purpose::STANDARD.decode(encoded)?;
                            file.write_all(&bytes)?;
                            file.flush()?;
                            offset += bytes.len() as u64;
                        }
                        if event["done"] == true {
                            if domain == "mcp" {
                                file.seek(SeekFrom::Start(0))?;
                                value["result"] = serde_json::from_reader(&mut file)?;
                            }
                            file.seek(SeekFrom::End(
                                -(file.metadata()?.len().min(64 * 1024) as i64),
                            ))?;
                            let mut tail = Vec::new();
                            file.read_to_end(&mut tail)?;
                            value
                                .as_object_mut()
                                .ok_or_else(|| anyhow::anyhow!("Invalid tool completion"))?
                                .remove("chunk");
                            value["output_path"] = json!(relative);
                            if domain == "device" {
                                value["output"] = json!(String::from_utf8_lossy(&tail));
                            }
                            return Ok(value);
                        }
                    }
                }
                anyhow::bail!("Tool stream closed before completion")
            }
            .await;
            match result {
                Ok(value) => return Ok(value),
                Err(error) if attempt == 3 => return Err(error),
                Err(_) => tokio::time::sleep(Duration::from_millis(250 << attempt)).await,
            }
        }
        unreachable!()
    }
}
#[derive(Debug)]
struct DeliveryRejected(String);
impl std::fmt::Display for DeliveryRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for DeliveryRejected {}
pub(super) fn decode_response(status: reqwest::StatusCode, text: &str) -> anyhow::Result<Value> {
    let value = serde_json::from_str::<Value>(text);
    if !status.is_success() {
        let detail = value
            .as_ref()
            .ok()
            .and_then(|v| v["error"].as_str())
            .unwrap_or(text);
        let detail: String = detail.chars().take(4096).collect();
        if value
            .as_ref()
            .is_ok_and(|value| value["delivery_rejected"] == true)
        {
            return Err(DeliveryRejected(detail).into());
        }
        anyhow::bail!("Node tool failed ({status}): {detail}");
    }
    Ok(value?)
}

fn present_result(name: &str, value: &mut Value) {
    if value.get("tool").is_some() {
        value["tool"] = json!(name);
    }
    if name.starts_with("mcp.") {
        normalize_mcp(value);
        fn strip(value: &mut Value) {
            if let Some(object) = value.as_object_mut() {
                object.remove("server_ref");
                object.remove("call_id");
            }
            if let Some(server) = value.get_mut("server") {
                strip(server);
            }
            for field in ["items", "calls"] {
                if let Some(items) = value.get_mut(field).and_then(Value::as_array_mut) {
                    for item in items {
                        strip(item);
                    }
                }
            }
        }
        strip(value);
    }
}

fn normalize_mcp(value: &mut Value) {
    if let Some(reference) = value.get("server_ref").cloned() {
        value["target"] = reference["owner_origin"].clone();
        value["server_id"] = reference["server_id"].clone();
    }
    if let Some(id) = value.get("call_id").cloned() {
        value["operation_id"] = id;
    }
    if let Some(server) = value.get_mut("server") {
        normalize_mcp(server);
    }
    for field in ["items", "calls"] {
        if let Some(items) = value.get_mut(field).and_then(Value::as_array_mut) {
            for item in items {
                normalize_mcp(item);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_results_preserve_call_identity_and_hide_transport_aliases() {
        let mut searched = json!({"items":[{"server_ref":{"owner_origin":"node","server_id":"server"},"name":"echo"}]});
        present_result("mcp.search", &mut searched);
        assert_eq!(
            searched,
            json!({"items":[{"target":"node","server_id":"server","name":"echo"}]})
        );
        let mut installed = json!({"items":[{"server":{"server_ref":{"owner_origin":"node","server_id":"server"},"availability":"disabled"},"enabled":false}]});
        present_result("mcp.list", &mut installed);
        assert_eq!(
            installed,
            json!({"items":[{"server":{"target":"node","server_id":"server","availability":"disabled"},"enabled":false}]})
        );
    }

    #[tokio::test]
    async fn invalid_public_field_is_rejected_before_transport() {
        let tool = Named {
            base: "http://127.0.0.1:1".into(),
            http: reqwest::Client::new(),
            name: "mcp.inspect".into(),
            fields: vec!["target".into(), "server_id".into(), "tool".into()],
        };
        let context = ToolContext {
            control: None,
            session_id: "test".into(),
            invocation_id: "invalid-field".into(),
            workspace: "/unused".into(),
        };
        let result = tool
            .execute(&context, &json!({"server_id":"server","tool_name":"echo"}))
            .await;
        assert_eq!(result.outcome, ToolOutcome::Failed);
        assert_eq!(result.data["state"], "not_dispatched");
        let error = result.data["error"].as_str().unwrap();
        assert!(error.contains("tool_name") && error.contains("target, server_id, tool"));
        assert!(!error.contains("server_ref") && !error.contains("call_id"));
    }

    #[test]
    fn parameter_rejections_preserve_plain_text_and_json_diagnostics() {
        let plain =
            "request.config.grant: invalid type: string, expected internally tagged enum Grant";
        let error = decode_response(reqwest::StatusCode::UNPROCESSABLE_ENTITY, plain)
            .unwrap_err()
            .to_string();
        assert!(error.contains(plain));
        let error = decode_response(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":"mcp_revision_conflict"}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("mcp_revision_conflict"));
        let huge = "字".repeat(10000);
        assert!(
            decode_response(reqwest::StatusCode::BAD_REQUEST, &huge)
                .unwrap_err()
                .to_string()
                .chars()
                .count()
                < 4200
        );
    }
}

fn activity(name: &str, args: &Value) -> ToolActivity {
    if let Some(op) = name.strip_prefix("mcp.") {
        let mut args = args.clone();
        args["op"] = json!(op);
        return mcp::activity(&args);
    }
    let (zh, en) = match name {
        "device.list" => ("查看设备", "Listing devices"),
        "device.inspect" => ("查看设备环境", "Inspecting device"),
        _ => ("执行命令", "Running command"),
    };
    ToolActivity::new(zh, en, "")
}

#[cfg(test)]
mod stream_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn disconnected_output_resumes_at_committed_offset_without_reexecuting() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for index in 0..3 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let header_end = loop {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).await.unwrap();
                    request.push(byte[0]);
                    if request.ends_with(b"\r\n\r\n") {
                        break request.len();
                    }
                };
                let headers = String::from_utf8_lossy(&request).to_string();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|value| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                request.resize(header_end + length, 0);
                socket.read_exact(&mut request[header_end..]).await.unwrap();
                let body: Value = serde_json::from_slice(&request[header_end..]).unwrap();
                let response = if index == 0 {
                    assert!(headers.starts_with("POST /v1/node-tools "));
                    assert_eq!(body["invocation_id"], "resume-test");
                    json!({"operation_id":"receipt","state":"running"}).to_string()
                } else {
                    assert!(headers.starts_with("POST /v1/tools/watch "));
                    assert_eq!(body["id"], "receipt");
                    let (offset, next, text, done) = if index == 1 {
                        (0, 6, "Zmlyc3QK", false)
                    } else {
                        (6, 13, "c2Vjb25kCg==", true)
                    };
                    assert_eq!(body["offset"], offset);
                    format!(
                        "{}\n",
                        json!({"value":{"operation_id":"receipt","state":if done {"succeeded"}else{"running"},"result":{"process_state":if done {"exited"} else {"running"}},"chunk":{"offset":offset,"next_offset":next,"base64":text}},"done":done})
                    )
                };
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            response.len(),
                            response
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        });
        let workspace = std::env::temp_dir().join(format!("zork-stream-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&workspace).unwrap();
        let tool = Named {
            base,
            http: reqwest::Client::new(),
            name: "device.exec".into(),
            fields: vec!["command".into()],
        };
        let context = ToolContext {
            control: None,
            session_id: "session".into(),
            invocation_id: "resume-test".into(),
            workspace: workspace.to_string_lossy().into(),
        };
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            tool.execute(&context, &json!({"command":"original"})),
        )
        .await
        .unwrap();
        server.await.unwrap();
        assert_eq!(result.outcome, ToolOutcome::Succeeded);
        assert_eq!(result.data["output"], "first\nsecond\n");
        assert_eq!(
            std::fs::read(workspace.join(result.data["output_path"].as_str().unwrap())).unwrap(),
            b"first\nsecond\n"
        );
        assert!(result.data.get("operation_id").is_none());
        assert!(history::Results
            .outstanding(history::Results.fold(None, &result.data).unwrap().as_ref())
            .is_empty());
        std::fs::remove_dir_all(workspace).unwrap();
    }
}

pub(super) fn remote_shell(base: &str) -> anyhow::Result<Arc<dyn ToolImplementation>> {
    Ok(Arc::new(Named {
        base: base.into(),
        http: reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
        name: "device.exec".into(),
        fields: ["target", "command", "cwd", "env"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
    }))
}
