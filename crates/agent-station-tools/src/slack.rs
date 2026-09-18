//! A single Slack namespace adapter; method-specific API tables are not needed.
use serde_json::{json, Value};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use zork_agent::session::{
    events::ToolOutcome,
    tools::{
        NoToolState, ToolActivity, ToolContext, ToolContract, ToolExecution, ToolImplementation,
        ToolInstance, ToolIntroduction, ToolNamespace, ToolRegistry, ToolVersion,
    },
};

const VERSION: &str = "slack-forward-1";
const MANUAL: &str = include_str!("slack_help.md");
struct SlackNamespace {
    base: String,
    http: reqwest::Client,
}
struct SlackTool {
    method: Option<String>,
    base: String,
    http: reqwest::Client,
}

pub fn register(registry: &Arc<ToolRegistry>, base: &str) -> anyhow::Result<()> {
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    registry.register(Arc::new(ToolInstance::new(ToolContract {
        name: "slack.connections".into(), version: ToolVersion::new(VERSION)?,
        initial_description: "List Slack connection IDs to use as connect_id. Read tool.help for the Slack forwarding manual.".into(),
        detailed_description: format!("List configured Slack connections without credentials. Returns connect_id, name, enabled and configured.\n\n{MANUAL}"),
        input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
    }, Arc::new(SlackTool { method: None, base: base.into(), http: http.clone() }), Arc::new(NoToolState))?));
    registry.register_namespace(
        "slack.",
        Arc::new(SlackNamespace {
            base: base.into(),
            http,
        }),
    )?;
    Ok(())
}

impl ToolNamespace for SlackNamespace {
    fn introduction(&self) -> ToolIntroduction {
        ToolIntroduction { name: "slack.*".into(), version: ToolVersion::new(VERSION).unwrap(),
            description: "Call slack.<official method name> with explicit connect_id and native Slack arguments. Use tool.help on the exact name for the forwarding manual; slack.connections lists connection IDs.".into() }
    }
    fn resolve(&self, name: &str) -> Option<Arc<ToolInstance>> {
        let method = name.strip_prefix("slack.")?;
        if !zork_slack::forward::valid_method(method) {
            return None;
        }
        Some(Arc::new(ToolInstance::new(ToolContract {
            name: name.into(), version: ToolVersion::new(VERSION).unwrap(),
            initial_description: format!("Forward Slack {method}"),
            detailed_description: format!("{MANUAL}\nOfficial method reference: https://docs.slack.dev/reference/methods/{method}/"),
            input_schema: json!({"type":"object","properties":{"connect_id":{"type":"string","minLength":1,"description":"ID from slack.connections or the observed message origin.connect_id. Removed before forwarding."}},"required":["connect_id"],"additionalProperties":true}),
        }, Arc::new(SlackTool { method: Some(method.into()), base: self.base.clone(), http: self.http.clone() }), Arc::new(NoToolState)).expect("fixed Slack contract")
            .with_activity(|args| ToolActivity::field("调用 Slack", "Calling Slack", args, "/connect_id")).advertise(false)))
    }
}

impl ToolImplementation for SlackTool {
    fn execute<'a>(
        &'a self,
        context: &'a ToolContext,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = ToolExecution> + Send + 'a>> {
        Box::pin(async move {
            let Some(mut arguments) = args.as_object().cloned() else {
                return failed("arguments_must_be_object");
            };
            let request = if let Some(method) = &self.method {
                let Some(Value::String(connect_id)) = arguments.remove("connect_id") else {
                    return failed("connect_id_required");
                };
                if connect_id.trim().is_empty() {
                    return failed("connect_id_required");
                }
                self.http.post(format!("{}/v1/slack/forward", self.base))
                    .json(&json!({"session_id":context.session_id,"connect_id":connect_id,"method":method,"arguments":arguments}))
            } else {
                if !arguments.is_empty() {
                    return failed("slack.connections_takes_no_arguments");
                }
                self.http
                    .get(format!("{}/v1/slack/connections", self.base))
                    .query(&[("session_id", &context.session_id)])
            };
            let mut response = match request.send().await {
                Ok(r) => r,
                Err(_) => return failed("slack_delivery_unknown: station transport failed"),
            };
            let status = response.status();
            let mut bytes = Vec::new();
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) if bytes.len() + chunk.len() <= 1024 * 1024 => {
                        bytes.extend_from_slice(&chunk)
                    }
                    Ok(None) => break,
                    _ => {
                        return failed(
                            "slack_delivery_unknown: station response unavailable or too large",
                        )
                    }
                }
            }
            let body: Value = match serde_json::from_slice(&bytes) {
                Ok(v) => v,
                Err(_) => return failed("slack_delivery_unknown: invalid station response"),
            };
            let failure = !status.is_success() || body["ok"] == false;
            let mut result = ToolExecution::success(body);
            if failure {
                result.outcome = ToolOutcome::Failed;
            }
            result
        })
    }
}
fn failed(message: &str) -> ToolExecution {
    let mut result = ToolExecution::success(json!({"error":message}));
    result.outcome = ToolOutcome::Failed;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use zork_agent::session::tools::ToolResolution;
    #[test]
    fn exact_slack_names_are_lazy_and_legacy_names_are_absent() {
        let registry = Arc::new(ToolRegistry::default());
        register(&registry, "http://127.0.0.1:9").unwrap();
        let known = registry
            .initial_catalog()
            .into_iter()
            .map(|i| (i.name, i.version))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(known.len(), 2);
        for method in [
            "slack.chat.postMessage",
            "slack.conversations.replies",
            "slack.future.newMethod",
        ] {
            let contract = registry.current_contract(method).unwrap();
            assert!(contract.detailed_description.contains("connect_id"));
            assert!(matches!(
                registry.resolve(method, Some(&contract.version)),
                ToolResolution::Ready(_)
            ));
        }
        assert!(registry.changes(&known).is_empty());
        for name in [
            "slack.history",
            "slack.post_message",
            "slack.post_file",
            "slack.chat.postMessage/evil",
        ] {
            assert!(registry.current_contract(name).is_none(), "{name}");
        }
    }
    #[tokio::test]
    async fn missing_connection_fails_before_io() {
        let tool = SlackTool {
            method: Some("chat.postMessage".into()),
            base: "http://127.0.0.1:9".into(),
            http: reqwest::Client::new(),
        };
        let context = ToolContext {
            session_id: "s".into(),
            invocation_id: "i".into(),
            workspace: "/unused".into(),
            control: None,
        };
        for args in [
            json!({}),
            json!({"connect_id":null}),
            json!({"connect_id":" "}),
        ] {
            assert_eq!(
                tool.execute(&context, &args).await.data["error"],
                "connect_id_required"
            );
        }
    }
}
