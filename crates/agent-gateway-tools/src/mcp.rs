//! MCP contracts and result presentation. Execution uses the ordinary tool lifecycle.
use serde_json::{json, Value};
pub fn execution_for(op: &str, mut value: Value) -> zork_agent::session::tools::ToolExecution {
    if op == "inspect" {
        if let Some(items) = value["items"].as_array_mut() {
            for item in items {
                if let Some(definition) = item["definition"].as_object_mut() {
                    if let Some(schema) = definition.remove("inputSchema") {
                        definition.insert(
                            "parameters".into(),
                            Value::String(zork_agent::session::tools::parameter_types(&schema)),
                        );
                    }
                }
            }
        }
    }
    execution(value)
}

pub fn execution(mut value: Value) -> zork_agent::session::tools::ToolExecution {
    use base64::Engine;
    use zork_agent::session::{events::ToolOutcome, tools::ToolExecution, wire::ToolImage};
    let mut images = Vec::new();
    if let Some(content) = value
        .pointer_mut("/result/content")
        .and_then(Value::as_array_mut)
    {
        for item in content {
            if item["type"] == "image" {
                if let (Some(mime), Some(data)) = (item["mimeType"].as_str(), item["data"].as_str())
                {
                    if matches!(
                        mime,
                        "image/png" | "image/jpeg" | "image/webp" | "image/gif"
                    ) && images.len() < 4
                        && base64::engine::general_purpose::STANDARD
                            .decode(data)
                            .is_ok()
                    {
                        images.push(ToolImage {
                            media_type: mime.into(),
                            base64: data.into(),
                        });
                        if let Some(object) = item.as_object_mut() {
                            object.remove("data");
                            object.insert("inline".into(), json!(true));
                        }
                    }
                }
            }
        }
    }
    let failed = value["pending_delivery"] == true
        || matches!(
            value["state"].as_str(),
            Some(
                "failed"
                    | "tool_error"
                    | "outcome_unknown"
                    | "not_dispatched"
                    | "expired"
                    | "result_unavailable"
            )
        );
    let mut execution = ToolExecution::success(value);
    execution.images = images;
    if failed {
        execution.outcome = ToolOutcome::Failed;
    }
    execution
}

#[cfg(test)]
mod result_tests {
    use super::*;
    use zork_agent::session::events::ToolOutcome;
    #[test]
    fn tool_errors_and_images_keep_their_native_meaning() {
        let result = execution(
            json!({"state":"succeeded","result":{"content":[{"type":"image","mimeType":"image/png","data":"aGVsbG8="}]}}),
        );
        assert_eq!(result.images.len(), 1);
        assert!(result.data["result"]["content"][0].get("data").is_none());
        assert_eq!(
            execution(json!({"state":"tool_error","result":{"isError":true}})).outcome,
            ToolOutcome::Failed
        );
    }
}

pub fn properties() -> Value {
    let string = || json!({"type":"string","minLength":1});
    let env = json!({"type":"object","additionalProperties":{"type":"string"},"description":"Environment variable map. Secret fields map to Gateway environment variable NAMES, never their secret values."});
    json!({
        "config":{"type":"object","properties":{
            "name":string(),"description":{"type":"string"},"enabled":{"type":"boolean","description":"False disables use for all callers while preserving installation. Disabled services remain in mcp.list but are omitted from mcp.search."},"tool_allowlist":{"type":["array","null"],"items":string(),"description":"Restricts callable tools for local and remote callers. Use null to allow all; an empty array allows none. Omit to preserve the current value when updating."},
            "transport":{"oneOf":[
                {"type":"object","properties":{"kind":{"const":"http"},"url":string(),"secret_headers":env.clone()},"required":["kind","url"],"additionalProperties":false},
                {"type":"object","properties":{"kind":{"const":"stdio"},"command":string(),"args":{"type":"array","items":{"type":"string"}},"cwd":{"type":"string"},"env":env.clone(),"secret_env":env},"required":["kind","command"],"additionalProperties":false}
            ]}
        },"required":["name","transport"],"additionalProperties":false},
        "expected_revision":string(),
        "query":{"type":"string","maxLength":256},"tool":string(),"binding_revision":string(),"arguments":{"type":"object"},"cursor":string()
    })
}
pub fn activity(args: &Value) -> zork_agent::session::tools::ToolActivity {
    use zork_agent::session::tools::ToolActivity;
    let (zh, en) = match args["op"].as_str() {
        Some("install") => ("安装 MCP", "Installing MCP"),
        Some("update") => ("配置 MCP", "Configuring MCP"),
        Some("uninstall") => ("卸载 MCP", "Uninstalling MCP"),
        Some("inspect") => ("查看 MCP", "Inspecting MCP"),
        _ => ("访问 MCP", "Accessing MCP"),
    };
    ToolActivity::field(
        zh,
        en,
        args,
        if args.get("config").is_some() {
            "/config/name"
        } else {
            "/tool"
        },
    )
}

#[cfg(test)]
mod presentation_tests {
    use super::*;
    #[test]
    fn inspect_converts_only_parameter_presentation_and_preserves_binding() {
        let raw = json!({"items":[{"binding_revision":"fixed", "definition":{
            "name":"echo","description":"Echo","inputSchema":{"type":"object","properties":{"text":{"type":"string","description":"Input text."}},"required":["text"],"additionalProperties":false}
        }}]});
        let result = execution_for("inspect", raw.clone());
        let definition = &result.data["items"][0]["definition"];
        assert!(definition.get("inputSchema").is_none());
        assert!(definition["parameters"]
            .as_str()
            .unwrap()
            .contains("// Input text.\n  text: string;"));
        assert_eq!(result.data["items"][0]["binding_revision"], "fixed");
        assert_eq!(execution_for("call", raw.clone()).data, raw);
    }
}
