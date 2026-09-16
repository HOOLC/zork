use serde_json::json;
use zork_agent::session::tools::{provider_call_definition, DynamicCall};

#[test]
// Contract: docs/design/agent-runtime.md [TOOL-01]
fn provider_exposes_one_fixed_call_shape_for_all_logical_tools() {
    let definition = provider_call_definition();
    assert_eq!(definition.name, "call");
    assert_eq!(definition.input_schema["type"], "object");
    assert_eq!(
        definition.input_schema["required"],
        json!(["tool", "action", "arguments"])
    );
    assert_eq!(definition.input_schema["additionalProperties"], false);
    assert_eq!(
        definition.input_schema["properties"]["tool"]["type"],
        "string"
    );
    assert_eq!(
        definition.input_schema["properties"]["arguments"]["type"],
        "object"
    );

    let call = DynamicCall::from_value(json!({
        "tool": "file.read",
        "goal": "检查项目说明",
        "action": "读取 README",
        "arguments": {"path": "README.md"},
        "comment": "ignored"
    }))
    .unwrap();
    assert_eq!(call.tool, "file.read");
    assert_eq!(call.arguments, json!({"path": "README.md"}));
}
