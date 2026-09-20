//! Native cua IPC to this device's signed desktop host; no MCP server or fallback daemon.
use crate::state::AppState;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt},
    process::Command,
};

const TIMEOUT: Duration = Duration::from_secs(120);
const MAX_OUTPUT: usize = 256 * 1024;
const MAX_WIRE: u64 = 48 * 1024 * 1024;
const DRIVER_VERSION: &str = "0.28.2";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
    tool: String,
    #[serde(default)]
    arguments: Value,
}

#[derive(Deserialize)]
struct Endpoint {
    socket: PathBuf,
    status: PathBuf,
    bundle_id: String,
    driver_version: String,
    #[serde(default)]
    driver_pid: u64,
}

#[cfg(unix)]
async fn frame(
    stream: &mut tokio::io::BufStream<tokio::net::UnixStream>,
    request: Value,
) -> anyhow::Result<Value> {
    stream.write_all(format!("{request}\n").as_bytes()).await?;
    stream.flush().await?;
    let mut bytes = Vec::new();
    (&mut *stream)
        .take(MAX_WIRE + 1)
        .read_until(b'\n', &mut bytes)
        .await?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_WIRE && bytes.last() == Some(&b'\n'),
        "computer_response_invalid_or_too_large"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(unix)]
async fn exchange(socket: &Path, request: Value) -> anyhow::Result<Value> {
    let mut stream = tokio::io::BufStream::new(tokio::net::UnixStream::connect(socket).await?);
    frame(&mut stream, request).await
}

#[cfg(unix)]
async fn invoke(endpoint: &Endpoint, request: Value) -> anyhow::Result<Value> {
    let mut stream =
        tokio::io::BufStream::new(tokio::net::UnixStream::connect(&endpoint.socket).await?);
    #[cfg(target_os = "macos")]
    anyhow::ensure!(
        stream.get_ref().peer_cred()?.pid().map(|pid| pid as u64) == Some(endpoint.driver_pid),
        "computer_host_peer_mismatch"
    );
    // Handshake and action share one accepted connection: replacement cannot race the call.
    verify_identity(
        endpoint,
        &frame(&mut stream, json!({"method":"metadata"})).await?,
    )?;
    frame(&mut stream, request).await
}
#[cfg(not(unix))]
async fn invoke(_: &Endpoint, _: Value) -> anyhow::Result<Value> {
    anyhow::bail!("computer_host_unsupported_platform")
}
#[cfg(not(unix))]
async fn exchange(_: &Path, _: Value) -> anyhow::Result<Value> {
    anyhow::bail!("computer_host_unsupported_platform")
}

async fn endpoint() -> anyhow::Result<&'static Endpoint> {
    static HOST: tokio::sync::OnceCell<Endpoint> = tokio::sync::OnceCell::const_new();
    HOST.get_or_try_init(|| async {
        anyhow::ensure!(cfg!(target_os = "macos"), "computer_host_unsupported_platform");
        let app = if let Some(path) = std::env::var_os("ZORK_CUA_HOST_APP") {
            PathBuf::from(path).canonicalize()?
        } else {
            let executable = std::env::current_exe()?;
            let host = executable.parent().ok_or_else(|| anyhow::anyhow!("computer_host_missing"))?
                .join("zork-cua-host").canonicalize().map_err(|_| anyhow::anyhow!("computer_host_missing: package the Zork Desktop Control helper"))?;
            host.parent().and_then(Path::parent).and_then(Path::parent)
                .ok_or_else(|| anyhow::anyhow!("computer_host_bundle_invalid"))?.to_path_buf()
        };
        let host = app.join("Contents/MacOS/zork-cua-host");
        let output = Command::new(host).arg("--endpoint").stdin(Stdio::null()).kill_on_drop(true).output().await?;
        anyhow::ensure!(output.status.success(), "computer_host_endpoint_failed");
        let mut endpoint: Endpoint = serde_json::from_slice(&output.stdout)?;
        anyhow::ensure!(endpoint.driver_version == DRIVER_VERSION, "computer_driver_version_mismatch");
        // LaunchServices, rather than the headless Station, owns the permission chain.
        let opened = Command::new("/usr/bin/open").args(["-g", "-a"]).arg(app)
            .stdin(Stdio::null()).kill_on_drop(true).status().await?;
        anyhow::ensure!(opened.success(), "computer_host_launch_failed");
        for _ in 0..50 {
            if let Ok(metadata) = exchange(&endpoint.socket, json!({"method":"metadata"})).await {
                let state: Value = serde_json::from_slice(&tokio::fs::read(&endpoint.status).await?)?;
                endpoint.driver_pid = state["driver_pid"].as_u64().unwrap_or(0);
                verify_identity(&endpoint, &metadata)?;
                return Ok(endpoint);
            }
            // Allow LaunchServices to replace a previous failed launch's report.
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let state: Value = serde_json::from_slice(&tokio::fs::read(&endpoint.status).await.unwrap_or_default()).unwrap_or(Value::Null);
        anyhow::bail!("computer_host_unavailable: {state}; open Zork Desktop Control with --request-permissions to authorize the host, then retry")
    }).await
}

fn verify_identity(endpoint: &Endpoint, response: &Value) -> anyhow::Result<()> {
    let metadata = &response["result"];
    anyhow::ensure!(
        response["ok"] == true
            && metadata["embedded"] == true
            && metadata["driver_version"] == endpoint.driver_version
            && metadata["host_bundle_id"] == endpoint.bundle_id
            && endpoint.driver_pid != 0
            && metadata["pid"].as_u64() == Some(endpoint.driver_pid),
        "computer_host_generation_or_version_mismatch: restart Station after replacing the host"
    );
    Ok(())
}

fn request(tool: &str, mut args: Value, session: &str) -> anyhow::Result<Value> {
    if args.is_null() {
        args = json!({});
    }
    let object = args
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("computer_arguments_must_be_object"))?;
    // Public model arguments cannot supply transport authority or select another session.
    object.retain(|key, _| !key.starts_with('_') && key != "session");
    anyhow::ensure!(
        !tool.is_empty() && tool.len() <= 64,
        "computer_tool_invalid"
    );
    if tool == "list_tools" {
        return Ok(json!({"method":"list"}));
    }
    if tool == "describe" {
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("computer_describe_name_required"))?;
        return Ok(json!({"method":"describe", "name":name}));
    }
    if tool == "check_permissions" {
        object.insert("prompt".into(), json!(false));
        object.insert("probe_direct_capture".into(), json!(false));
    }
    Ok(json!({"method":"call", "name":tool, "args":args,
              "session_id":format!("zork-{session}"), "observation_origin":"direct", "client_kind":"unknown"}))
}

fn normalize(tool: &str, mut response: Value) -> anyhow::Result<Value> {
    use base64::Engine as _;
    let failed = response["ok"] != true || response["result"]["isError"] == true;
    let mut captures = Vec::new();
    let directory = tempfile::Builder::new()
        .prefix("zork-computer-")
        .tempdir()?;
    if let Some(content) = response["result"]["content"].as_array_mut() {
        for item in content.iter_mut() {
            if item["type"] != "image" {
                continue;
            }
            let mime = item["mimeType"].as_str().unwrap_or("");
            anyhow::ensure!(
                matches!(mime, "image/png" | "image/jpeg"),
                "computer_capture_type_invalid"
            );
            anyhow::ensure!(captures.len() < 4, "computer_capture_count_exceeded");
            let data = item["data"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("computer_capture_missing"))?;
            anyhow::ensure!(data.len() <= 12 * 1024 * 1024, "computer_capture_too_large");
            let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
            anyhow::ensure!(bytes.len() <= 8 * 1024 * 1024, "computer_capture_too_large");
            let path = directory.path().join(format!("capture-{}", captures.len()));
            std::fs::write(&path, bytes)?;
            captures.push(json!({"path":path, "mime_type":mime}));
            *item = json!({"type":"image", "transferred":true});
        }
    }
    let text = response.to_string();
    let mut end = text.len().min(MAX_OUTPUT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if !captures.is_empty() {
        let _ = directory.keep();
    }
    Ok(
        json!({"state":if failed {"failed"} else {"succeeded"}, "tool":tool,
              "output":&text[..end], "truncated":end < text.len(), "images":captures}),
    )
}

pub async fn call(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(call): Json<Call>,
) -> Response {
    let Some(session) = headers
        .get("x-zork-session-key")
        .and_then(|value| value.to_str().ok())
    else {
        return failure(StatusCode::FORBIDDEN, "computer_session_required");
    };
    if !matches!(state.db.get_binding(session), Ok(Some(_))) {
        return failure(StatusCode::FORBIDDEN, "computer_session_unknown");
    }
    let run = async {
        let request = request(&call.tool, call.arguments, session)?;
        let endpoint = endpoint().await?;
        normalize(&call.tool, invoke(endpoint, request).await?)
    };
    match tokio::time::timeout(TIMEOUT, run).await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(error)) => failure(StatusCode::OK, &error.to_string()),
        Err(_) => failure(
            StatusCode::OK,
            "computer_timeout: action outcome may be unknown; inspect before retrying",
        ),
    }
}
fn failure(status: StatusCode, reason: &str) -> Response {
    (status, Json(json!({"state":"failed", "error":reason}))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strips_authority_and_never_prompts() {
        let value = request(
            "check_permissions",
            json!({"prompt":true,"_approved":true,"session":"other"}),
            "bound",
        )
        .unwrap();
        assert_eq!(
            value["args"],
            json!({"prompt":false,"probe_direct_capture":false})
        );
        assert_eq!(value["session_id"], "zork-bound");
        assert!(request("click", json!([]), "bound").is_err());
    }
    #[test]
    fn tool_error_is_not_transport_success() {
        let result = normalize("click", json!({"ok":true,"result":{"isError":true,"content":[{"type":"text","text":"denied"}]}})).unwrap();
        assert_eq!(result["state"], "failed");
    }
    #[test]
    fn generation_and_version_are_checked() {
        let endpoint = Endpoint {
            socket: PathBuf::new(),
            status: PathBuf::new(),
            bundle_id: "test.host".into(),
            driver_version: DRIVER_VERSION.into(),
            driver_pid: 42,
        };
        let mut value = json!({"ok":true,"result":{"embedded":true,"driver_version":DRIVER_VERSION,"host_bundle_id":"test.host","pid":42}});
        verify_identity(&endpoint, &value).unwrap();
        value["result"]["pid"] = json!(43);
        assert!(verify_identity(&endpoint, &value).is_err());
    }
}
