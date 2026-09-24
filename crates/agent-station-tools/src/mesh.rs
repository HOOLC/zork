//! Mesh invitations for onboarding a new computer. The Station decides which
//! Session may mint a join command; the tool only forwards runtime identity.
use super::*;
use zork_agent::session::tools::NoToolState;

const VERSION: &str = "mesh-invite-1";
const MANUAL: &str = include_str!("mesh_help.md");

struct MeshTool {
    name: &'static str,
    base: String,
    http: reqwest::Client,
}

fn definitions() -> [(&'static str, &'static str, Value); 3] {
    [
        (
            "mesh.invite",
            "Create a one-time command that joins a new computer to the user's Mesh. Only in a Zork client Chat. The command is a secret: run it on the target or give it only to the user here. Read tool.help first.",
            json!({"type":"object","properties":{"label":{"type":"string","maxLength":80,"description":"Short note such as the new computer's name."}},"additionalProperties":false}),
        ),
        (
            "mesh.invites",
            "List Mesh invitations and their state (created, used with the joined device, expired, revoked). Never includes commands.",
            json!({"type":"object","properties":{"include_inactive":{"type":"boolean","description":"Also list expired, used and revoked invitations not requested in this Chat."}},"additionalProperties":false}),
        ),
        (
            "mesh.revoke",
            "Revoke an unused Mesh invitation by id so its command no longer works.",
            json!({"type":"object","properties":{"id":{"type":"string","minLength":32,"maxLength":32,"description":"Invitation id from mesh.invite or mesh.invites."}},"required":["id"],"additionalProperties":false}),
        ),
    ]
}

pub(super) fn register(
    registry: &Arc<ToolRegistry>,
    base: &str,
    http: &reqwest::Client,
) -> anyhow::Result<()> {
    for (name, description, schema) in definitions() {
        registry.register(Arc::new(
            ToolInstance::new(
                ToolContract {
                    name: name.into(),
                    version: ToolVersion::new(VERSION)?,
                    initial_description: description.into(),
                    detailed_description: format!("{description}\n\n{MANUAL}"),
                    input_schema: schema,
                },
                Arc::new(MeshTool {
                    name,
                    base: base.into(),
                    http: http.clone(),
                }),
                Arc::new(NoToolState),
            )?
            .with_activity(move |args| activity(name, args)),
        ));
    }
    Ok(())
}

fn activity(name: &str, args: &Value) -> ToolActivity {
    match name {
        "mesh.invite" => {
            ToolActivity::field("生成连接命令", "Creating join command", args, "/label")
        }
        "mesh.revoke" => ToolActivity::new("撤销连接命令", "Revoking join command", ""),
        _ => ToolActivity::new("查看连接邀请", "Checking invitations", ""),
    }
}

impl ToolImplementation for MeshTool {
    fn execute<'a>(
        &'a self,
        context: &'a ToolContext,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = ToolExecution> + Send + 'a>> {
        Box::pin(async move {
            let failed = |data: Value| ToolExecution {
                images: Vec::new(),
                outcome: ToolOutcome::Failed,
                data,
                result_schema_version: 1,
                knowledge: None,
            };
            let response = self
                .http
                .post(format!("{}/v1/mesh/invite-tools", self.base))
                .json(&json!({"session_id":context.session_id,"invocation_id":context.invocation_id,"tool":self.name,"arguments":args}))
                .send()
                .await;
            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    return failed(
                        json!({"error":"station_unreachable","message":format!("The Station did not answer: {error}. Check mesh.invites before creating another invitation.")}),
                    )
                }
            };
            let status = response.status();
            let body = match response.bytes().await {
                Ok(bytes) if bytes.len() <= 256 * 1024 => bytes,
                _ => return failed(json!({"error":"station_response_unavailable"})),
            };
            let value: Value = serde_json::from_slice(&body)
                .unwrap_or_else(|_| json!({"error":"invalid_station_response"}));
            if status.is_success() {
                ToolExecution::success(value)
            } else {
                failed(value)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn mesh_tools_are_advertised_with_secret_handling_manual() {
        let registry = Arc::new(ToolRegistry::default());
        crate::register(&registry, "http://127.0.0.1:9".into()).unwrap();
        for name in ["mesh.invite", "mesh.invites", "mesh.revoke"] {
            let contract = registry.current_contract(name).expect(name);
            assert!(contract.initial_description.len() < 260, "{name}");
            for needle in [
                "secret",
                "mesh.revoke",
                "Zork Chat",
                "never post",
                "device.list",
                "zork mesh status",
            ] {
                assert!(
                    contract
                        .detailed_description
                        .to_lowercase()
                        .contains(&needle.to_lowercase()),
                    "{name} manual lacks {needle}"
                );
            }
        }
        let invite = registry.activity("mesh.invite", &json!({"label":"书房 iMac"}));
        assert_eq!(invite.labels["zh-CN"], "生成连接命令");
        assert_eq!(invite.detail, "书房 iMac");
        assert_eq!(
            registry.activity("mesh.invites", &json!({})).labels["zh-CN"],
            "查看连接邀请"
        );
        assert_eq!(
            registry.activity("mesh.revoke", &json!({"id":"x"})).labels["zh-CN"],
            "撤销连接命令"
        );
    }

    async fn serve_once(
        status: &'static str,
        body: Value,
    ) -> (String, tokio::task::JoinHandle<Value>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            let request_body = loop {
                let read = socket.read(&mut buffer).await.unwrap();
                request.extend_from_slice(&buffer[..read]);
                let text = String::from_utf8_lossy(&request).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let length: usize = text
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap())
                        })
                        .unwrap();
                    if request.len() >= end + 4 + length {
                        assert!(text.starts_with("POST /v1/mesh/invite-tools "));
                        break serde_json::from_slice::<Value>(&request[end + 4..end + 4 + length])
                            .unwrap();
                    }
                }
            };
            let body = body.to_string();
            socket
                .write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes())
                .await
                .unwrap();
            request_body
        });
        (base, handle)
    }

    fn context() -> ToolContext {
        ToolContext {
            control: None,
            session_id: "session".into(),
            invocation_id: "call-1".into(),
            workspace: std::env::temp_dir().to_string_lossy().into(),
        }
    }

    #[tokio::test]
    async fn invite_forwards_runtime_identity_and_returns_station_result() {
        let (base, server) = serve_once("200 OK", json!({"id":"a".repeat(32),"state":"created","command":"zork mesh join 'T' --channel stable","single_use":true})).await;
        let tool = MeshTool {
            name: "mesh.invite",
            base,
            http: reqwest::Client::new(),
        };
        let result = tool.execute(&context(), &json!({"label":"Laptop"})).await;
        let request = server.await.unwrap();
        assert_eq!(
            request,
            json!({"session_id":"session","invocation_id":"call-1","tool":"mesh.invite","arguments":{"label":"Laptop"}})
        );
        assert_eq!(result.outcome, ToolOutcome::Succeeded);
        assert_eq!(result.data["single_use"], true);
    }

    #[tokio::test]
    async fn refusal_is_a_failed_result_with_the_station_message() {
        let (base, server) = serve_once("400 Bad Request", json!({"ok":false,"error":"mesh_invite_requires_zork_chat","message":"mesh.invite is not available in slack conversations. Ask the user to request the new computer from a Chat in the Zork app."})).await;
        let tool = MeshTool {
            name: "mesh.invite",
            base,
            http: reqwest::Client::new(),
        };
        let result = tool.execute(&context(), &json!({})).await;
        server.await.unwrap();
        assert_eq!(result.outcome, ToolOutcome::Failed);
        assert_eq!(result.data["error"], "mesh_invite_requires_zork_chat");
        assert!(result.data.get("command").is_none());
    }
}
