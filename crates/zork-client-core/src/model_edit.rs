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

/// Validation failures are keyed by field: `profile-model` (id),
/// `profile-api`, `profile-context-limit`, `profile-output-limit` and
/// `profile-default-thinking` (the thinking scheme).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct FieldError {
    pub field: String,
    pub message: String,
    /// For a duplicate id: the existing model to edit instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate: Option<String>,
    /// For thinking budgets not below the output limit: those presets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bad_budgets: Vec<u32>,
}

pub const FIELD_ID: &str = "profile-model";
pub const FIELD_API: &str = "profile-api";
pub const FIELD_CONTEXT: &str = "profile-context-limit";
pub const FIELD_OUTPUT: &str = "profile-output-limit";
pub const FIELD_THINKING: &str = "profile-default-thinking";
const ID_MAX: usize = 200;

/// Why typed context text is unusable, if it is.
pub fn context_error(text: &str) -> Option<String> {
    if text.trim().is_empty() {
        Some("填写上下文长度".into())
    } else if parse_tokens(text).is_none() {
        Some("写成 128K 或 131072 这样的数字".into())
    } else {
        None
    }
}

/// Why typed output text is unusable, if it is; `context` is the parsed context.
pub fn output_error(text: &str, context: Option<u64>) -> Option<String> {
    if text.trim().is_empty() {
        return Some("填写最长输出".into());
    }
    let Some(output) = parse_tokens(text).filter(|o| u32::try_from(*o).is_ok()) else {
        return Some("写成 8K 或 8192 这样的数字".into());
    };
    context
        .filter(|c| output >= *c)
        .map(|c| format!("需小于上下文 {}", compact_tokens(c)))
}

/// Id problems: empty, too long, whitespace, or already in the connection.
pub fn id_error(id: &str, models: &[Value], previous: Option<&str>) -> Option<FieldError> {
    let id = id.trim();
    let message = if id.is_empty() {
        "填写供应商提供的模型 ID".to_owned()
    } else if id.chars().count() > ID_MAX {
        format!("模型 ID 最多 {ID_MAX} 个字符")
    } else if id.chars().any(|c| c.is_whitespace() || c.is_control()) {
        "模型 ID 不能包含空格".to_owned()
    } else if models
        .iter()
        .any(|m| m["id"] == id && m["id"].as_str() != previous)
    {
        return Some(FieldError {
            field: FIELD_ID.into(),
            message: format!("这个连接里已经有 {id}"),
            duplicate: Some(id.into()),
            bad_budgets: vec![],
        });
    } else {
        return None;
    };
    Some(FieldError {
        field: FIELD_ID.into(),
        message,
        duplicate: None,
        bad_budgets: vec![],
    })
}

impl ModelInput {
    pub fn errors(&self, models: &[Value]) -> Vec<FieldError> {
        let mut errors = Vec::new();
        let mut fail = |field: &str, message: String| {
            errors.push(FieldError {
                field: field.into(),
                message,
                duplicate: None,
                bad_budgets: vec![],
            })
        };
        let previous = self.previous.as_ref().and_then(|v| v["id"].as_str());
        if !MODEL_APIS.iter().any(|(api, _)| *api == self.api) {
            fail(FIELD_API, "请选择支持的模型接口".into());
        }
        let context = parse_tokens(&self.context);
        if let Some(message) = context_error(&self.context) {
            fail(FIELD_CONTEXT, message);
        }
        if let Some(message) = output_error(&self.output, context) {
            fail(FIELD_OUTPUT, message);
        }
        let output = parse_tokens(&self.output).and_then(|n| u32::try_from(n).ok());
        errors.extend(id_error(&self.id, models, previous));
        if let Some(problem) = self.scheme().problem(output) {
            errors.push(FieldError {
                field: FIELD_THINKING.into(),
                message: problem.message,
                duplicate: None,
                bad_budgets: problem.bad_budgets,
            });
        }
        // Field order in the form: id, thinking, length, protocol.
        let rank = |field: &str| match field {
            FIELD_ID => 0,
            FIELD_THINKING => 1,
            FIELD_CONTEXT => 2,
            FIELD_OUTPUT => 3,
            _ => 4,
        };
        errors.sort_by_key(|e| rank(&e.field));
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

/// Token counts as people write them: `128K`, `1M` and `1.5M` when that is
/// exact with at most one decimal (K = 1000, M = 1,000,000), otherwise the
/// thousands-separated integer (`32,768`), never odd decimals like `32.768K`.
pub fn compact_tokens(value: u64) -> String {
    for (scale, suffix) in [(1_000_000, "M"), (1_000, "K")] {
        if value >= scale && value % (scale / 10) == 0 {
            let whole = value / scale;
            let tenth = value % scale / (scale / 10);
            return if tenth == 0 {
                format!("{whole}{suffix}")
            } else {
                format!("{whole}.{tenth}{suffix}")
            };
        }
        if value >= scale {
            break;
        }
    }
    exact_tokens(value)
}

/// The exact count with thousands separators, e.g. `128,000`.
pub fn exact_tokens(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Parses `128K`, `128k`, `131072`, `1M`, `1.5M`, `200 000` or `200,000`.
/// The result must be a positive whole number of tokens after scaling, so
/// `1.2345K`, `0`, negatives and other units (`128千`) are rejected.
pub fn parse_tokens(value: &str) -> Option<u64> {
    let value: String = value
        .chars()
        .filter(|c| !(c.is_whitespace() || matches!(c, ',' | '_' | '\u{202f}')))
        .collect();
    let (number, scale) = match value.as_bytes().last()? {
        b'k' | b'K' => (&value[..value.len() - 1], 1_000u128),
        b'm' | b'M' => (&value[..value.len() - 1], 1_000_000u128),
        _ => (value.as_str(), 1u128),
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty()
        || whole.len() > 20
        || fraction.len() > 12
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || (number.contains('.') && fraction.is_empty())
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
    .filter(|n| *n > 0)
}

/// Shows typed token text in its normal form once it parses (on blur/Enter);
/// text that does not parse stays as typed.
pub fn normalize_tokens(text: &str) -> String {
    parse_tokens(text)
        .map(compact_tokens)
        .unwrap_or_else(|| text.trim().to_owned())
}

/// From here on a whole number typed next to a K|M unit is a raw token count
/// (`131072` with K selected means 131,072 tokens, not 131M); below it the
/// number means the selected unit (`128` with K is 128K).
pub const RAW_TOKEN_MIN: u64 = 10_000;

fn without_separators(text: &str) -> String {
    text.chars()
        .filter(|c| !(c.is_whitespace() || matches!(c, ',' | '_' | '\u{202f}')))
        .collect()
}

/// What a number field + K|M unit control show for token text: `128K` →
/// (`128`, `K`), `131,072` → (`131072`, ``) — an empty unit is a raw count —
/// and empty text → (``, `K`).
pub fn token_parts(text: &str) -> (String, &'static str) {
    let text = without_separators(text);
    match text.chars().last() {
        None => (String::new(), "K"),
        Some('k' | 'K') => (text[..text.len() - 1].to_owned(), "K"),
        Some('m' | 'M') => (text[..text.len() - 1].to_owned(), "M"),
        Some(_) => (text, ""),
    }
}

/// Joins a number field and its unit (`K`, `M` or `` for a raw count) into
/// token text. A whole number of at least [`RAW_TOKEN_MIN`] is a raw count
/// whatever the unit, so a pasted `131072` can never become 131,072K.
pub fn join_token_parts(number: &str, unit: &str) -> String {
    let number = without_separators(number);
    let raw = !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit())
        && number.parse::<u64>().map_or(true, |n| n >= RAW_TOKEN_MIN);
    match unit {
        _ if raw || number.is_empty() => number,
        "k" | "K" => number + "K",
        "m" | "M" => number + "M",
        _ => number,
    }
}

/// Picks another unit for token text: a raw count of at least
/// [`RAW_TOKEN_MIN`] keeps its value (`131072` → `131.072K`, `0.131072M`);
/// any smaller number takes the new unit (`128K` → `128M`, `1` → `1M`), which
/// is what choosing a unit after typing means.
pub fn switch_token_unit(text: &str, unit: &str) -> String {
    let (number, current) = token_parts(text);
    let (scale, suffix, digits): (u64, &str, usize) = match unit {
        "k" | "K" => (1_000, "K", 3),
        "m" | "M" => (1_000_000, "M", 6),
        _ => return join_token_parts(&number, ""),
    };
    if current.is_empty() {
        if let Some(count) = parse_tokens(&number).filter(|n| *n >= RAW_TOKEN_MIN) {
            let fraction = format!("{:0digits$}", count % scale);
            let fraction = fraction.trim_end_matches('0');
            let whole = count / scale;
            return if fraction.is_empty() {
                format!("{whole}{suffix}")
            } else {
                format!("{whole}.{fraction}{suffix}")
            };
        }
    }
    if number.is_empty() {
        return number;
    }
    number + suffix
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
            (1200000, "1.2M"),
            (32800, "32.8K"),
            (4096, "4,096"),
            (32768, "32,768"),
            (131072, "131,072"),
            (1048576, "1,048,576"),
            (1250000, "1,250,000"),
            (999, "999"),
            (1000, "1K"),
        ] {
            assert_eq!(compact_tokens(value), label);
            assert_eq!(parse_tokens(label), Some(value), "{label}");
        }
        assert_eq!(exact_tokens(128000), "128,000");
        assert_eq!(exact_tokens(1000000), "1,000,000");
        assert_eq!(exact_tokens(7), "7");
    }
    #[test]
    fn token_parts_join_and_unit_switch() {
        assert_eq!(token_parts("128K"), ("128".into(), "K"));
        assert_eq!(token_parts("1.5m"), ("1.5".into(), "M"));
        assert_eq!(token_parts("131,072"), ("131072".into(), ""));
        assert_eq!(token_parts(" "), (String::new(), "K"));
        // A big bare number is a raw count whatever unit is selected.
        assert_eq!(join_token_parts("131072", "K"), "131072");
        assert_eq!(join_token_parts("10000", "M"), "10000");
        assert_eq!(join_token_parts("9999", "K"), "9999K");
        assert_eq!(join_token_parts("128", "K"), "128K");
        assert_eq!(join_token_parts("1.5", "M"), "1.5M");
        assert_eq!(join_token_parts("10000.5", "K"), "10000.5K");
        assert_eq!(join_token_parts("819", ""), "819");
        assert_eq!(join_token_parts("", "K"), "");
        assert_eq!(
            join_token_parts("99999999999999999999999", "K"),
            "99999999999999999999999"
        );
        for (number, unit, tokens) in [
            ("131072", "K", 131072),
            ("128", "K", 128000),
            ("1.5", "M", 1500000),
        ] {
            assert_eq!(parse_tokens(&join_token_parts(number, unit)), Some(tokens));
        }
        // Switching a raw count keeps its value; a number with a unit takes the new unit.
        assert_eq!(switch_token_unit("131072", "K"), "131.072K");
        assert_eq!(switch_token_unit("131,072", "M"), "0.131072M");
        assert_eq!(switch_token_unit("2000000", "M"), "2M");
        assert_eq!(
            parse_tokens(&switch_token_unit("131072", "M")),
            Some(131072)
        );
        assert_eq!(switch_token_unit("128K", "M"), "128M");
        assert_eq!(switch_token_unit("1.5M", "K"), "1.5K");
        assert_eq!(switch_token_unit("1", "M"), "1M");
        assert_eq!(switch_token_unit("4,096", "K"), "4096K");
        assert_eq!(switch_token_unit("", "M"), "");
        assert_eq!(switch_token_unit("12x", "M"), "12xM");
    }
    #[test]
    fn token_text_accepts_common_spellings_and_rejects_the_rest() {
        for (text, value) in [
            ("128K", 128_000),
            ("128k", 128_000),
            ("131072", 131_072),
            ("1M", 1_000_000),
            ("1m", 1_000_000),
            ("1.5M", 1_500_000),
            ("200 000", 200_000),
            ("200,000", 200_000),
            (" 8K ", 8_000),
            ("1.5000K", 1_500),
            ("4.096K", 4_096),
            ("128 K", 128_000),
        ] {
            assert_eq!(parse_tokens(text), Some(value), "{text}");
        }
        for text in [
            "",
            " ",
            "K",
            "1.2345K",
            "128千",
            "-1K",
            "-5",
            "0",
            "0K",
            "0.0M",
            "1.K",
            ".5K",
            "12x",
            "1e5",
            "1.0011K",
            "128KK",
            "99999999999999999999999",
        ] {
            assert_eq!(parse_tokens(text), None, "{text:?}");
        }
        assert_eq!(normalize_tokens("131072"), "131,072");
        assert_eq!(normalize_tokens("128000"), "128K");
        assert_eq!(normalize_tokens(" 1.50m "), "1.5M");
        assert_eq!(normalize_tokens("12x "), "12x");
    }
    #[test]
    fn validation_messages_follow_the_editor_wording() {
        let models = vec![json!({"id":"deepseek-chat"})];
        let base = ModelInput {
            id: "m".into(),
            api: "openai-completions".into(),
            context: "128K".into(),
            output: "8K".into(),
            thinking_scheme: Some(crate::thinking::ThinkingScheme::Unsupported),
            ..Default::default()
        };
        let message = |input: &ModelInput, field: &str| {
            input
                .errors(&models)
                .into_iter()
                .find(|e| e.field == field)
                .map(|e| e.message)
        };
        assert!(base.errors(&models).is_empty());
        let mut input = base.clone();
        input.id = " ".into();
        assert_eq!(
            message(&input, FIELD_ID).unwrap(),
            "填写供应商提供的模型 ID"
        );
        input.id = "deepseek-chat".into();
        let duplicate = input.errors(&models).remove(0);
        assert_eq!(duplicate.message, "这个连接里已经有 deepseek-chat");
        assert_eq!(duplicate.duplicate.as_deref(), Some("deepseek-chat"));
        input.id = "a b".into();
        assert_eq!(message(&input, FIELD_ID).unwrap(), "模型 ID 不能包含空格");
        input.id = "x".repeat(201);
        assert!(message(&input, FIELD_ID).unwrap().contains("200"));
        // Editing keeps its own id.
        let mut editing = base.clone();
        editing.id = "deepseek-chat".into();
        editing.previous = Some(json!({"id":"deepseek-chat"}));
        assert_eq!(message(&editing, FIELD_ID), None);

        let mut input = base.clone();
        input.context = "".into();
        assert_eq!(message(&input, FIELD_CONTEXT).unwrap(), "填写上下文长度");
        input.context = "128千".into();
        assert_eq!(
            message(&input, FIELD_CONTEXT).unwrap(),
            "写成 128K 或 131072 这样的数字"
        );
        let mut input = base.clone();
        input.output = "".into();
        assert_eq!(message(&input, FIELD_OUTPUT).unwrap(), "填写最长输出");
        input.output = "8千".into();
        assert_eq!(
            message(&input, FIELD_OUTPUT).unwrap(),
            "写成 8K 或 8192 这样的数字"
        );
        input.output = "128K".into();
        assert_eq!(message(&input, FIELD_OUTPUT).unwrap(), "需小于上下文 128K");
        input.context = "131072".into();
        input.output = "200K".into();
        assert_eq!(
            message(&input, FIELD_OUTPUT).unwrap(),
            "需小于上下文 131,072"
        );
        input.output = "5000000000".into();
        assert!(
            message(&input, FIELD_OUTPUT).is_some(),
            "output must fit u32"
        );

        let mut input = base.clone();
        input.output = "16K".into();
        input.thinking_scheme = Some(crate::thinking::ThinkingScheme::Budget {
            presets: vec![4_000, 16_000, 32_000],
            default: crate::thinking::BudgetChoice::Off,
            dynamic: false,
            allow_off: true,
        });
        let error = input
            .errors(&models)
            .into_iter()
            .find(|e| e.field == FIELD_THINKING)
            .unwrap();
        assert_eq!(error.message, "预算需小于最长输出 16K");
        assert_eq!(error.bad_budgets, vec![16_000, 32_000]);

        // Errors come in form order: id, thinking, context, output.
        let mut input = base.clone();
        input.id = String::new();
        input.context = String::new();
        input.output = String::new();
        input.thinking_scheme = Some(crate::thinking::ThinkingScheme::Levels {
            values: vec![],
            default: String::new(),
        });
        let fields: Vec<String> = input.errors(&[]).into_iter().map(|e| e.field).collect();
        assert_eq!(
            fields,
            [FIELD_ID, FIELD_THINKING, FIELD_CONTEXT, FIELD_OUTPUT]
        );
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
