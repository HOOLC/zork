//! Shared model editing rules. Platform editors only supply input values.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub fn valid_id(id: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !id.is_empty()
            && id.len() <= 160
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            && !matches!(id, "." | ".."),
        "invalid identifier"
    );
    Ok(())
}
pub fn valid_name(name: &str, max: usize) -> anyhow::Result<String> {
    let name = name.trim();
    anyhow::ensure!(
        !name.is_empty() && name.chars().count() <= max && !name.chars().any(char::is_control),
        "名称需为 1–{max} 个字符"
    );
    Ok(name.into())
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionInput {
    pub id: String,
    pub provider: String,
    pub billing: String,
    pub base_url: String,
    pub key: String,
}

pub fn connection_options(catalog: &[Value], subscription: bool) -> Vec<(usize, usize)> {
    catalog
        .iter()
        .enumerate()
        .filter_map(|(index, provider)| {
            let billing = provider["billing"]
                .as_array()?
                .iter()
                .position(|b| (b["id"] == "subscription") == subscription)?;
            Some((index, billing))
        })
        .collect()
}

pub fn connection_choices(
    catalog: &[Value],
    subscription: bool,
    provider: &str,
    billing: &str,
) -> Value {
    let providers = connection_options(catalog, subscription)
        .into_iter()
        .map(|(p, _)| &catalog[p])
        .collect::<Vec<_>>();
    let provider = providers.iter().find(|p| p["id"] == provider).copied();
    let billings = provider
        .and_then(|p| p["billing"].as_array())
        .into_iter()
        .flatten()
        .filter(|b| (b["id"] == "subscription") == subscription)
        .collect::<Vec<_>>();
    let billing = billings
        .iter()
        .find(|b| b["id"] == billing)
        .copied()
        .or(billings.first().copied());
    json!({"providers":providers,"provider":provider,"billings":billings,"billing":billing})
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInput {
    pub previous: Option<Value>,
    pub copied: Option<Value>,
    pub id: String,
    pub api: String,
    pub context: String,
    pub output: String,
    pub thinking: String,
    pub default_thinking: String,
    pub images: bool,
    /// Structured thinking chosen in the editor. When absent, `thinking` and
    /// `default_thinking` are read as the legacy comma-separated list.
    #[serde(default)]
    pub thinking_scheme: Option<crate::thinking::ThinkingScheme>,
}

pub fn model_form(profile: &Value, providers: &[Value], model: Option<Value>) -> ModelInput {
    let template = providers
        .iter()
        .find(|p| p["id"] == profile["provider"])
        .and_then(|p| p["billing"].as_array())
        .and_then(|items| items.iter().find(|b| b["id"] == profile["billing"]))
        .and_then(|b| b["template"]["models"].as_array())
        .and_then(|models| models.first());
    let defaults = model.as_ref().or(template);
    ModelInput {
        id: model
            .as_ref()
            .and_then(|m| m["id"].as_str())
            .unwrap_or_default()
            .into(),
        api: defaults
            .and_then(|m| m["api"].as_str())
            .filter(|api| MODEL_APIS.iter().any(|a| a.0 == *api))
            .unwrap_or(MODEL_APIS[0].0)
            .into(),
        context: model
            .as_ref()
            .and_then(|m| m["limits"]["context_window_tokens"].as_u64())
            .map(compact_tokens)
            .unwrap_or_default(),
        output: model
            .as_ref()
            .and_then(|m| m["limits"]["max_output_tokens"].as_u64())
            .map(compact_tokens)
            .unwrap_or_default(),
        thinking: defaults
            .and_then(|m| m["thinking"].as_array())
            .map(|v| {
                v.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_else(|| "off".into()),
        default_thinking: defaults
            .and_then(|m| m["default_thinking"].as_str())
            .unwrap_or("off")
            .into(),
        images: defaults.is_some_and(model_accepts_images),
        thinking_scheme: defaults.map(crate::model_catalog::stored_scheme),
        previous: model,
        copied: None,
    }
}
/// The form for a model, with unset fields filled from the built-in catalog
/// while the model is still unconfigured (new, or fetched without limits).
pub fn model_form_with_catalog(
    profile: &Value,
    providers: &[Value],
    model: Option<Value>,
) -> ModelInput {
    let input = model_form(profile, providers, model);
    let unconfigured = input
        .previous
        .as_ref()
        .is_none_or(|m| !m["limits"].is_object());
    if !unconfigured {
        return input;
    }
    crate::model_catalog::fill_input(input, profile["provider"].as_str()).0
}
pub fn copy_form(input: ModelInput, source: Value) -> ModelInput {
    let mut copied = model_form(&Value::Null, &[], Some(source.clone()));
    copied.id = input.id;
    copied.previous = input.previous;
    copied.copied = Some(source);
    copied
}
pub fn copyable(model: &Value) -> bool {
    model["limits"]["context_window_tokens"]
        .as_u64()
        .zip(model["limits"]["max_output_tokens"].as_u64())
        .is_some_and(|(c, o)| o > 0 && c > o)
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

impl ModelInput {
    pub fn errors(&self, models: &[Value]) -> Vec<FieldError> {
        let mut errors = Vec::new();
        let mut fail = |field: &str, message: &str| {
            errors.push(FieldError {
                field: field.into(),
                message: message.into(),
            })
        };
        let previous = self.previous.as_ref().and_then(|v| v["id"].as_str());
        let id = self.id.trim();
        if id.is_empty() {
            fail("profile-model", "填写供应商提供的模型 ID。");
        } else if models
            .iter()
            .any(|m| m["id"] == id && m["id"].as_str() != previous)
        {
            fail("profile-model", "此 ID 已存在，请使用其他模型 ID。");
        }
        if !MODEL_APIS.iter().any(|(api, _)| *api == self.api) {
            fail("profile-api", "请选择支持的模型接口。");
        }
        let context = parse_tokens(&self.context).filter(|n| *n > 0);
        let output = parse_tokens(&self.output)
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0);
        if context.is_none() {
            fail("profile-context-limit", "请输入大于 0 的整数。");
        }
        if output.is_none() {
            fail("profile-output-limit", "请输入大于 0 的整数。");
        } else if context.zip(output).is_some_and(|(c, o)| u64::from(o) >= c) {
            fail(
                "profile-output-limit",
                &format!("需小于上下文上限（{}）。", compact_tokens(context.unwrap())),
            );
        }
        if let Some(message) = self.scheme().error(output) {
            fail("profile-default-thinking", &message);
        }
        errors
    }
    /// The thinking scheme this form describes.
    pub fn scheme(&self) -> crate::thinking::ThinkingScheme {
        self.thinking_scheme.clone().unwrap_or_else(|| {
            crate::thinking::ThinkingScheme::from_legacy_input(
                &self.thinking,
                &self.default_thinking,
            )
        })
    }
    pub fn apply(&self, models: Vec<Value>) -> anyhow::Result<Vec<Value>> {
        if let Some(error) = self.errors(&models).first() {
            anyhow::bail!("{}", error.message);
        }
        let previous = self.previous.as_ref().and_then(|v| v["id"].as_str());
        let existing = previous.and_then(|id| models.iter().find(|m| m["id"] == id));
        if let Some(expected) = &self.previous {
            let current =
                existing.ok_or_else(|| anyhow::anyhow!("模型已被移除，请重新打开编辑器"))?;
            anyhow::ensure!(
                configuration(current) == configuration(expected),
                "模型已在其他位置修改，请重新打开编辑器"
            );
        }
        let mut model = existing
            .cloned()
            .unwrap_or_else(|| json!({"capabilities":{"input":["text"]},"streaming":true}));
        if let Some(source) = &self.copied {
            model = copied_model_configuration(&model, source);
        }
        model["id"] = json!(self.id.trim());
        model["api"] = json!(self.api);
        let (thinking, default_thinking) = self.scheme().encode();
        model["thinking"] = json!(thinking);
        model["default_thinking"] = json!(default_thinking);
        set_model_image_input(&mut model, self.images);
        if !model["limits"].is_object() {
            model["limits"] = json!({});
        }
        model["limits"]["context_window_tokens"] = json!(parse_tokens(&self.context).unwrap());
        model["limits"]["max_output_tokens"] = json!(parse_tokens(&self.output).unwrap());
        model["default"] =
            json!(existing.is_some_and(|m| m["default"] == true) || models.is_empty());
        if let Some(fields) = model.as_object_mut() {
            fields.remove("context_window");
            fields.remove("max_output_tokens");
            if self.api != "openai-responses" {
                fields.remove("parallel_tool_calls");
            }
            if self.api != "openai-codex-responses" {
                fields.remove("service_tier");
            }
        }
        replace_configured_model(models, previous, model).map_err(anyhow::Error::msg)
    }
}

fn configuration(value: &Value) -> Value {
    let mut value = value.clone();
    if let Some(fields) = value.as_object_mut() {
        fields.remove("enabled");
    }
    value
}

pub fn remove_model(mut models: Vec<Value>, expected: &Value) -> anyhow::Result<Vec<Value>> {
    let id = expected["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("缺少模型 ID"))?;
    let current = models
        .iter()
        .find(|m| m["id"] == id)
        .ok_or_else(|| anyhow::anyhow!("模型已被移除"))?;
    anyhow::ensure!(
        configuration(current) == configuration(expected),
        "模型已在其他位置修改，请重新打开编辑器"
    );
    models.retain(|m| m["id"] != id);
    if !models.iter().any(|m| m["default"] == true) {
        if let Some(first) = models.first_mut() {
            first["default"] = json!(true);
        }
    }
    Ok(models)
}

pub fn compact_tokens(value: u64) -> String {
    let (scale, digits, suffix) = if value >= 1_000_000 {
        (1_000_000, 6, "M")
    } else if value >= 1_000 {
        (1_000, 3, "K")
    } else {
        return value.to_string();
    };
    let whole = value / scale;
    let remainder = value % scale;
    if remainder == 0 {
        return format!("{whole}{suffix}");
    }
    let fraction = format!("{remainder:0digits$}");
    format!("{whole}.{}{suffix}", fraction.trim_end_matches('0'))
}
pub fn parse_tokens(value: &str) -> Option<u64> {
    let value = value.trim();
    let (number, scale) = match value.as_bytes().last()? {
        b'k' | b'K' => (&value[..value.len() - 1], 1_000u128),
        b'm' | b'M' => (&value[..value.len() - 1], 1_000_000u128),
        _ => (value, 1u128),
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let denominator = 10u128.checked_pow(fraction.len().try_into().ok()?)?;
    let fractional = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u128>().ok()?.checked_mul(scale)?
    };
    if fractional % denominator != 0 {
        return None;
    }
    u64::try_from(
        whole
            .parse::<u128>()
            .ok()?
            .checked_mul(scale)?
            .checked_add(fractional / denominator)?,
    )
    .ok()
}

pub const MODEL_APIS: [(&str, &str); 4] = [
    ("openai-completions", "OpenAI Chat Completions"),
    ("openai-responses", "OpenAI Responses"),
    ("openai-codex-responses", "Codex Responses"),
    ("anthropic-messages", "Anthropic Messages"),
];
pub fn model_accepts_images(model: &Value) -> bool {
    model["capabilities"]["input"]
        .as_array()
        .is_some_and(|input| input.iter().any(|kind| kind == "image"))
}

pub fn set_model_image_input(model: &mut Value, enabled: bool) {
    if !model["capabilities"].is_object() {
        model["capabilities"] = json!({});
    }
    let mut input = model["capabilities"]["input"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![json!("text")]);
    input.retain(|kind| kind != "image");
    if enabled {
        input.push(json!("image"));
    }
    if input.is_empty() {
        input.push(json!("text"));
    }
    model["capabilities"]["input"] = json!(input);
}

// Model identity and selection belong to the target, not the copied configuration.
pub fn copied_model_configuration(target: &Value, source: &Value) -> Value {
    let mut result = target.clone();
    for key in [
        "api",
        "streaming",
        "parallel_tool_calls",
        "service_tier",
        "thinking",
        "default_thinking",
        "capabilities",
        "limits",
    ] {
        result.as_object_mut().unwrap().remove(key);
        if let Some(value) = source.get(key) {
            result[key] = value.clone();
        }
    }
    result
}

pub fn replace_configured_model(
    mut models: Vec<Value>,
    previous: Option<&str>,
    model: Value,
) -> Result<Vec<Value>, &'static str> {
    if models
        .iter()
        .any(|m| m["id"] == model["id"] && m["id"].as_str() != previous)
    {
        return Err("此模型 ID 已在连接中");
    }
    if let Some(index) = previous.and_then(|id| models.iter().position(|m| m["id"] == id)) {
        models[index] = model;
    } else {
        models.push(model);
    }
    Ok(models)
}
#[cfg(test)]
mod model_tests {
    use super::*;
    #[test]
    fn editing_rejects_changed_or_removed_models_and_preserves_unrelated_changes() {
        let original = json!({"id":"a","api":"openai-responses","default":true,"enabled":true});
        let input = ModelInput {
            previous: Some(original.clone()),
            id: "a".into(),
            api: "openai-responses".into(),
            context: "128K".into(),
            output: "4K".into(),
            thinking: "off, high".into(),
            default_thinking: "high".into(),
            ..Default::default()
        };
        let another = json!({"id":"b","limits":{"context_window_tokens":200000}});
        let mut disabled = original.clone();
        disabled["enabled"] = json!(false);
        let updated = input.apply(vec![disabled, another.clone()]).unwrap();
        assert_eq!(updated[0]["enabled"], false);
        assert_eq!(updated[1], another);
        let mut changed = original.clone();
        changed["api"] = json!("anthropic-messages");
        assert!(input.apply(vec![changed.clone()]).is_err());
        assert!(input.apply(vec![]).is_err());
        assert!(remove_model(vec![changed], &original).is_err());
        assert!(input.errors(&[]).is_empty());
    }
    #[test]
    fn copying_configuration_preserves_target_identity_and_selection() {
        let target =
            json!({"id":"vision","enabled":false,"default":false,"service_tier":"priority"});
        let source = json!({"id":"flash","enabled":true,"default":true,
            "api":"openai-responses","streaming":true,"parallel_tool_calls":true,
            "thinking":["off","high"],"default_thinking":"high",
            "capabilities":{"input":["text","image"]},
            "limits":{"context_window_tokens":1000000,"max_output_tokens":384000,"reserve_percent":15}});
        let copied = copied_model_configuration(&target, &source);
        assert_eq!(copied["id"], "vision");
        assert_eq!(copied["enabled"], false);
        assert_eq!(copied["default"], false);
        assert!(copied.get("service_tier").is_none());
        for key in [
            "api",
            "streaming",
            "parallel_tool_calls",
            "thinking",
            "default_thinking",
            "capabilities",
            "limits",
        ] {
            assert_eq!(copied[key], source[key]);
        }
        assert!(copied_model_configuration(&json!({}), &source)
            .get("enabled")
            .is_none());
        assert_eq!(source["id"], "flash");
    }
    #[test]
    fn token_units_preserve_values_when_editing() {
        for (value, label) in [
            (32000, "32K"),
            (128000, "128K"),
            (1000000, "1M"),
            (1500000, "1.5M"),
            (4096, "4.096K"),
            (1048576, "1.048576M"),
        ] {
            assert_eq!(compact_tokens(value), label);
            assert_eq!(parse_tokens(label), Some(value));
        }
        assert_eq!(parse_tokens("1.5m"), Some(1500000));
        assert_eq!(parse_tokens("1.0011K"), None);
        assert_eq!(parse_tokens("-1K"), None);
    }
    #[test]
    fn edit_preserves_other_models_and_prevents_duplicate_identity() {
        let models = vec![
            json!({"id":"a","default":true}),
            json!({"id":"b","api":"anthropic-messages"}),
        ];
        assert!(replace_configured_model(models.clone(), Some("a"), json!({"id":"b"})).is_err());
        let updated =
            replace_configured_model(models.clone(), Some("a"), json!({"id":"c","default":true}))
                .unwrap();
        assert_eq!(updated[1], models[1]);
        assert_eq!(updated.len(), 2);
        assert_eq!(updated[0]["id"], "c");
    }
}
