//! The Agent tools' parameter contract and its user-editable projection.
use serde_json::{json, Value};
use zork_client_types::interaction::{Choice, Field, FieldKind, Request};

pub fn schema(creating: bool, complete: bool) -> Value {
    let string = || json!({"type":"string","maxLength":512});
    let selection = json!({"type":"object","properties":{"profile_id":string(),"model":string(),"thinking":string()},"required":if complete {json!(["profile_id","model","thinking"])}else{json!([])},"additionalProperties":false});
    let mut value = json!({"type":"object","properties":{
        "name":{"type":"string","maxLength":160},"avatar":{"type":["string","null"]},
        "selection":selection,"instructions":{"type":"string","maxLength":32000},
        "allowed_leaders":{"type":"array","maxItems":32,"uniqueItems":true,"items":string()}
    },"required":if complete {json!(["name","selection"])}else{json!([])},"additionalProperties":false});
    if creating {
        value["properties"]["role"] = json!({"enum":["leader","worker"],"default":"worker"});
    }
    for (key, title) in [
        ("name", "Name"),
        ("avatar", "Avatar"),
        ("selection", "Model selection"),
        ("instructions", "Instructions"),
        ("allowed_leaders", "Allowed leaders (JSON list)"),
        ("role", "Role"),
    ] {
        if let Some(property) = value["properties"].get_mut(key) {
            property["title"] = json!(title);
        }
    }
    for (key, title) in [
        ("profile_id", "Profile"),
        ("model", "Model"),
        ("thinking", "Reasoning level"),
    ] {
        value["properties"]["selection"]["properties"][key]["title"] = json!(title);
    }
    value
}
/// Choices are resolved by the Agent business on the owning node. No IDs or
/// JSON arrays need to become editable text in the user-facing card.
#[derive(Default)]
pub struct ReviewChoices {
    pub models: Vec<Choice>,
    pub leaders: Vec<Choice>,
}
pub fn selection_value(value: &Value) -> String {
    serde_json::to_string(&json!({"profile_id":value["profile_id"],"model":value["model"],"thinking":value["thinking"]})).unwrap()
}

pub fn form(creating: bool, values: &Value, changes: &Value, choices: ReviewChoices) -> Request {
    let text = |id: &str, label: &str, kind, required| Field {
        id: id.into(),
        label: label.into(),
        kind,
        required,
        default: values
            .pointer(id)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        options: vec![],
    };
    let multiple = |id: &str, label: &str, options| Field {
        id: id.into(),
        label: label.into(),
        kind: FieldKind::MultiChoice,
        required: false,
        default: serde_json::to_string(&values.pointer(id).cloned().unwrap_or_else(|| json!([])))
            .unwrap(),
        options,
    };
    let selection = selection_value(&values["selection"]);
    let selection = choices
        .models
        .iter()
        .any(|item| item.value == selection)
        .then_some(selection)
        .unwrap_or_default();
    let mut fields = vec![
        text("/name", "interaction_name", FieldKind::Text, true),
        text(
            "/instructions",
            "interaction_instructions",
            FieldKind::Multiline,
            false,
        ),
        Field {
            id: "/selection".into(),
            label: "interaction_model".into(),
            kind: FieldKind::Choice,
            required: true,
            default: selection,
            options: choices.models,
        },
        multiple("/allowed_leaders", "interaction_grants", choices.leaders),
        text("/avatar", "interaction_avatar", FieldKind::Text, false),
    ];
    if creating {
        fields.push(Field {
            id: "/role".into(),
            label: "interaction_role".into(),
            kind: FieldKind::Choice,
            required: true,
            default: values["role"].as_str().unwrap_or("worker").into(),
            options: vec![
                Choice {
                    value: "worker".into(),
                    label: "interaction_role_worker".into(),
                },
                Choice {
                    value: "leader".into(),
                    label: "interaction_role_leader".into(),
                },
            ],
        });
    }
    let mut prominent = fields
        .iter()
        .filter(|field| {
            let key = field.id.trim_start_matches('/');
            if creating {
                matches!(key, "name" | "instructions" | "selection")
                    || changes
                        .get(key)
                        .is_some_and(|v| !v.is_null() && v != &json!([]) && v != &json!(""))
            } else {
                changes.get(key).is_some()
            }
        })
        .map(|field| field.id.clone())
        .collect::<Vec<_>>();
    if prominent.is_empty() {
        prominent.push("/name".into());
    }
    Request::AgentConfiguration {
        creating,
        name: values["name"].as_str().unwrap_or_default().into(),
        fields,
        prominent,
    }
}
fn set(value: &mut Value, path: &str, new: Value) {
    let keys = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
    let mut current = value;
    for key in &keys[..keys.len() - 1] {
        if !current[*key].is_object() {
            current[*key] = json!({});
        }
        current = &mut current[*key];
    }
    current[keys[keys.len() - 1]] = new;
}
pub fn submitted(
    creating: bool,
    original: &Value,
    form: &Request,
    values: &std::collections::BTreeMap<String, String>,
) -> anyhow::Result<Value> {
    let Request::AgentConfiguration { fields, .. } = form else {
        anyhow::bail!("agent_form_required")
    };
    let values = form
        .validate_values(values)
        .map_err(|e| anyhow::anyhow!("{}", serde_json::to_string(&e).unwrap()))?;
    let mut result = original.clone();
    for field in fields {
        let value = &values[&field.id];
        if !creating && value == &field.default {
            continue;
        }
        let value = if field.kind == FieldKind::MultiChoice || field.id == "/selection" {
            serde_json::from_str(value)?
        } else if field.id == "/avatar" && value.is_empty() {
            Value::Null
        } else {
            json!(value)
        };
        set(&mut result, &field.id, value);
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn review_uses_business_parameters_and_keeps_patch_omissions() {
        let initial = json!({"name":"Before","selection":{"profile_id":"p","model":"m","thinking":"high"},"avatar":"avatar","instructions":"Keep","allowed_leaders":["leader"]});
        let form = form(
            false,
            &initial,
            &json!({"name":"Proposed"}),
            ReviewChoices {
                models: vec![Choice {
                    value: selection_value(&initial["selection"]),
                    label: "Model".into(),
                }],
                leaders: vec![Choice {
                    value: "leader".into(),
                    label: "Leader".into(),
                }],
            },
        );
        let patch = submitted(
            false,
            &json!({"name":"Proposed"}),
            &form,
            &[("/name".into(), "User name".into())].into(),
        )
        .unwrap();
        assert_eq!(patch, json!({"name":"User name"}));
        assert!(form.defaults().contains_key("/selection"));
        assert!(
            matches!(&form, Request::AgentConfiguration { prominent, .. } if prominent==&vec!["/name".to_owned()])
        );
    }
    #[test]
    fn reviewed_model_choice_updates_the_whole_selection_and_preserves_hidden_values() {
        let selection = json!({"profile_id":"p","model":"new-model","thinking":"high"});
        let values = json!({"name":"Helper","selection":selection,"role":"worker","allowed_leaders":["caller"]});
        let form = form(
            true,
            &values,
            &json!({"name":"Helper"}),
            ReviewChoices {
                models: vec![Choice {
                    value: selection_value(&selection),
                    label: "Configured model".into(),
                }],
                leaders: vec![Choice {
                    value: "caller".into(),
                    label: "Requesting Agent".into(),
                }],
            },
        );
        form.validate().unwrap();
        let submitted = submitted(
            true,
            &values,
            &form,
            &[("/name".into(), "Chosen name".into())].into(),
        )
        .unwrap();
        assert_eq!(submitted["selection"], selection);
        assert_eq!(submitted["name"], "Chosen name");
        assert_eq!(submitted["allowed_leaders"], values["allowed_leaders"]);
        assert!(!form.defaults().contains_key("/selection/profile_id"));
    }
}
