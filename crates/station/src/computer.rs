//! Desktop control through an external cua-driver daemon.
//!
//! The daemon is owned by whichever signed host holds the macOS desktop
//! permissions. This Station only speaks to the driver's local socket, so no
//! Accessibility or Screen Recording grant is attributed to the Station
//! process itself.
use crate::state::AppState;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::process::Command;

const TIMEOUT: Duration = Duration::from_secs(120);
const MAX_OUTPUT: usize = 256 * 1024;
/// Tools whose result is a capture. The driver writes the PNG to a file so it
/// never travels through the Station HTTP body.
const CAPTURE: [&str; 4] = ["get_desktop_state", "get_window_state", "zoom", "page"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
    tool: String,
    #[serde(default)]
    arguments: Value,
}

fn environment(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn capture_path() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("zork-computer-{}-{stamp}.png", std::process::id()))
}

fn truncated(text: String) -> (String, bool) {
    if text.len() <= MAX_OUTPUT {
        return (text, false);
    }
    let mut end = MAX_OUTPUT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

pub async fn call(State(_state): State<AppState>, Json(request): Json<Call>) -> Response {
    if request.tool.trim().is_empty() || request.tool.len() > 64 {
        return failure(StatusCode::BAD_REQUEST, "computer_tool_invalid");
    }
    let capture = CAPTURE.contains(&request.tool.as_str()).then(capture_path);
    let binary = match environment("ZORK_CUA_DRIVER_BIN") {
        Some(binary) => PathBuf::from(binary),
        // A packaged host ships the driver next to the Station; a terminal
        // install resolves it from PATH.
        None => match crate::binshim::resolve_named_bin("cua-driver", None) {
            Ok(binary) => binary,
            Err(_) => return failure(StatusCode::BAD_GATEWAY, "computer_driver_missing"),
        },
    };
    let mut command = Command::new(binary);
    command
        .arg("call")
        .arg(&request.tool)
        .arg(request.arguments.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(socket) = environment("ZORK_CUA_DRIVER_SOCKET") {
        command.arg("--socket").arg(socket);
    }
    if let Some(path) = &capture {
        command.arg("--screenshot-out-file").arg(path);
    }
    let output = match tokio::time::timeout(TIMEOUT, command.output()).await {
        Err(_) => return failure(StatusCode::GATEWAY_TIMEOUT, "computer_driver_timeout"),
        Ok(Err(error)) => {
            return failure(
                StatusCode::BAD_GATEWAY,
                &format!("computer_driver_unavailable: {error}"),
            )
        }
        Ok(Ok(output)) => output,
    };
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let captures = capture
        .filter(|path| path.is_file())
        .map(|path| vec![json!({"path": path.display().to_string(), "mime_type": "image/png"})])
        .unwrap_or_default();
    if !output.status.success() {
        let detail = if stderr.is_empty() { stdout } else { stderr };
        let (detail, _) = truncated(detail);
        return (
            StatusCode::OK,
            Json(json!({"state": "failed", "tool": request.tool, "error": detail, "images": captures})),
        )
            .into_response();
    }
    let parsed: Option<Value> = serde_json::from_str(&stdout).ok();
    let (output, truncated_output) = truncated(match &parsed {
        Some(value) => value.to_string(),
        None => stdout,
    });
    (
        StatusCode::OK,
        Json(json!({
            "state": "succeeded",
            "tool": request.tool,
            "output": output,
            "truncated": truncated_output,
            "images": captures,
        })),
    )
        .into_response()
}

fn failure(status: StatusCode, reason: &str) -> Response {
    (status, Json(json!({"state": "failed", "error": reason}))).into_response()
}
