//! Built-in defaults for popular models (`data/model_catalog.json`).
//!
//! The catalog fills what a user would otherwise look up: context and output
//! limits, how the model thinks and what it accepts. Values are starting points
//! to be checked against vendor documentation; they only fill unset fields and
//! never overwrite what a user entered.
use crate::model_edit::{compact_tokens, ModelInput};
use crate::thinking::ThinkingScheme;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::OnceLock;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Entry {
    pub key: String,
    pub name: String,
    pub family: String,
    pub provider: String,
    #[serde(rename = "match")]
    pub matching: Matching,
    pub api: String,
    pub context: u64,
    pub output: u32,
    pub thinking: ThinkingScheme,
    pub thinking_field: Option<String>,
    pub capabilities: Capabilities,
    pub popular: bool,
    pub verified: bool,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Matching {
    #[serde(default)]
    pub ids: Vec<String>,
    #[serde(default)]
    pub prefixes: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Capabilities {
    pub image: bool,
    pub pdf: bool,
    pub tools: bool,
}

#[derive(Deserialize)]
struct File {
    models: Vec<Entry>,
}

pub fn entries() -> &'static [Entry] {
    static CATALOG: OnceLock<Vec<Entry>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str::<File>(include_str!("../data/model_catalog.json"))
            .expect("built-in model catalog is valid")
            .models
    })
}

/// Reduces a provider-specific id to the bare model name: router prefixes
/// (`openrouter/deepseek/…`, `models/…`) and variant tags (`:free`) go away.
pub fn normalize(id: &str) -> String {
    let id = id.trim().to_ascii_lowercase();
    let id = id.rsplit('/').next().unwrap_or(&id);
    let id = id.split(':').next().unwrap_or(id);
    id.split('@').next().unwrap_or(id).to_owned()
}

/// The catalog entry for a model id, if any. The longest matching id or prefix
/// wins, so `gpt-5.1-codex` is Codex rather than GPT-5; `provider` breaks ties.
pub fn recognize(id: &str, provider: Option<&str>) -> Option<&'static Entry> {
    let id = normalize(id);
    if id.is_empty() {
        return None;
    }
    entries()
        .iter()
        .filter_map(|entry| {
            let exact = entry
                .matching
                .ids
                .iter()
                .filter(|candidate| **candidate == id)
                .map(|c| c.len() + 1000);
            let prefix = entry
                .matching
                .prefixes
                .iter()
                .filter(|candidate| id.starts_with(candidate.as_str()))
                .map(String::len);
            let score = exact.chain(prefix).max()?;
            let same_provider = provider.is_some_and(|p| p.eq_ignore_ascii_case(&entry.provider));
            Some((score, same_provider, entry))
        })
        .max_by_key(|(score, same_provider, _)| (*score, *same_provider))
        .map(|(_, _, entry)| entry)
}

/// "128K · 输出 8K · 思考开关"
pub fn summary(context: Option<u64>, output: Option<u32>, thinking: &ThinkingScheme) -> String {
    let mut parts = Vec::new();
    if let Some(context) = context {
        parts.push(compact_tokens(context));
    }
    if let Some(output) = output {
        parts.push(format!("输出 {}", compact_tokens(u64::from(output))));
    }
    parts.push(thinking.summary());
    parts.join(" · ")
}

impl Entry {
    pub fn summary(&self) -> String {
        summary(Some(self.context), Some(self.output), &self.thinking)
    }
    fn view(&self) -> Value {
        json!({
            "key": self.key, "name": self.name, "family": self.family,
            "provider": self.provider, "api": self.api,
            "context": self.context, "output": self.output,
            "thinking": self.thinking, "thinking_field": self.thinking_field,
            "capabilities": self.capabilities, "verified": self.verified,
            "source": self.source, "summary": self.summary(),
        })
    }
}

/// Row summary for a stored model (`limits`, `thinking`, `default_thinking`).
pub fn model_summary(model: &Value) -> String {
    let thinking = stored_scheme(model);
    summary(
        model["limits"]["context_window_tokens"].as_u64(),
        model["limits"]["max_output_tokens"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok()),
        &thinking,
    )
}

pub fn stored_scheme(model: &Value) -> ThinkingScheme {
    let values: Vec<String> = model["thinking"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    ThinkingScheme::decode(
        &values,
        model["default_thinking"].as_str().unwrap_or_default(),
    )
}

/// A field that the editor offers reference values for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Context,
    Output,
    Thinking,
    Capabilities,
}

#[derive(Clone, Debug, Serialize)]
pub struct Reference {
    pub key: String,
    pub name: String,
    pub family: String,
    pub provider: String,
    /// The value to put into the field.
    pub value: Value,
    /// How the value reads in the menu, e.g. "128K" or "思考 minimal–high".
    pub label: String,
    /// True for the model the current id was recognized as.
    pub recognized: bool,
}

/// Values of one field across popular models, for the per-field reference menu.
/// The recognized model comes first; the rest follow popularity order.
pub fn references(field: Field, recognized: Option<&str>) -> Vec<Reference> {
    let recognized = recognized.and_then(|id| recognize(id, None));
    let mut list: Vec<&Entry> = recognized.into_iter().collect();
    list.extend(
        entries()
            .iter()
            .filter(|e| e.popular && Some(e.key.as_str()) != recognized.map(|r| r.key.as_str())),
    );
    list.into_iter()
        .map(|entry| {
            let (value, label) = match field {
                Field::Context => (json!(entry.context), compact_tokens(entry.context)),
                Field::Output => (json!(entry.output), compact_tokens(u64::from(entry.output))),
                Field::Thinking => (
                    json!({"scheme": entry.thinking, "field": entry.thinking_field}),
                    entry.thinking.summary(),
                ),
                Field::Capabilities => (
                    serde_json::to_value(entry.capabilities).unwrap_or_default(),
                    capability_label(entry.capabilities),
                ),
            };
            Reference {
                key: entry.key.clone(),
                name: entry.name.clone(),
                family: entry.family.clone(),
                provider: entry.provider.clone(),
                value,
                label,
                recognized: recognized.is_some_and(|r| r.key == entry.key),
            }
        })
        .collect()
}

fn capability_label(c: Capabilities) -> String {
    let mut parts = Vec::new();
    if c.image {
        parts.push("图片");
    }
    if c.pdf {
        parts.push("PDF");
    }
    if c.tools {
        parts.push("工具");
    }
    if parts.is_empty() {
        "仅文本".into()
    } else {
        parts.join(" · ")
    }
}

/// Fills a stored model's unset fields from the catalog: missing limits, a
/// placeholder `["off"]` thinking list, and image input. Returns whether it
/// changed anything. User-set values stay.
pub fn fill_model(model: &mut Value, provider: Option<&str>) -> bool {
    let Some(entry) = model["id"].as_str().and_then(|id| recognize(id, provider)) else {
        return false;
    };
    let mut changed = false;
    if !model["limits"].is_object() {
        model["limits"] = json!({
            "context_window_tokens": entry.context,
            "max_output_tokens": entry.output,
        });
        changed = true;
    }
    if placeholder_thinking(model) && entry.thinking != ThinkingScheme::Unsupported {
        let (values, default) = entry.thinking.encode();
        model["thinking"] = json!(values);
        model["default_thinking"] = json!(default);
        changed = true;
    }
    if entry.capabilities.image && !crate::model_edit::model_accepts_images(model) {
        if model["capabilities"]["input"]
            .as_array()
            .is_none_or(|input| input.iter().all(|kind| kind == "text"))
        {
            crate::model_edit::set_model_image_input(model, true);
            changed = true;
        }
    }
    changed
}

fn placeholder_thinking(model: &Value) -> bool {
    match model["thinking"].as_array() {
        None => true,
        Some(values) => values.is_empty() || (values.len() == 1 && values[0] == "off"),
    }
}

/// Fills a form's empty fields from the recognized model. Fields the user
/// already filled (or an existing model's values) stay as they are.
pub fn fill_input(mut input: ModelInput, provider: Option<&str>) -> (ModelInput, Option<Value>) {
    let Some(entry) = recognize(&input.id, provider) else {
        return (input, None);
    };
    if input.context.trim().is_empty() {
        input.context = compact_tokens(entry.context);
    }
    if input.output.trim().is_empty() {
        input.output = compact_tokens(u64::from(entry.output));
    }
    if input.scheme() == ThinkingScheme::Unsupported {
        input.thinking_scheme = Some(entry.thinking.clone());
        let (values, default) = entry.thinking.encode();
        input.thinking = values.join(", ");
        input.default_thinking = default;
    }
    if !input.images {
        input.images = entry.capabilities.image;
    }
    (input, Some(entry.view()))
}

/// Result of recognizing an id: the entry (if any) plus a filled form.
pub fn recognition(id: &str, provider: Option<&str>, input: Option<ModelInput>) -> Value {
    let entry = recognize(id, provider);
    let input = input.map(|mut input| {
        input.id = id.to_owned();
        fill_input(input, provider).0
    });
    json!({
        "entry": entry.map(Entry::view),
        "summary": entry.map(Entry::summary),
        "input": input,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_loads_and_every_entry_is_valid() {
        assert!(entries().len() >= 20);
        for entry in entries() {
            assert!(!entry.verified, "{} has not been checked yet", entry.key);
            assert!(!entry.source.is_empty(), "{} needs a source", entry.key);
            assert!(
                u64::from(entry.output) < entry.context,
                "{} output must be below context",
                entry.key
            );
            assert_eq!(
                entry.thinking.error(Some(entry.output)),
                None,
                "{} thinking",
                entry.key
            );
            assert!(crate::model_edit::MODEL_APIS
                .iter()
                .any(|(api, _)| *api == entry.api));
        }
    }

    #[test]
    fn recognizes_exact_dated_and_routed_ids() {
        assert_eq!(
            recognize("deepseek-chat", None).unwrap().key,
            "deepseek-chat"
        );
        assert_eq!(
            recognize("claude-sonnet-4-5-20250929", None).unwrap().key,
            "claude-sonnet"
        );
        assert_eq!(
            recognize("claude-opus-4-5-20251101", None).unwrap().key,
            "claude-opus-4-5"
        );
        assert_eq!(recognize("gpt-5-2025-08-07", None).unwrap().key, "gpt-5");
        assert_eq!(
            recognize("gpt-5.1-codex-mini", None).unwrap().key,
            "gpt-5-codex"
        );
        assert_eq!(recognize("GPT-5-Mini", None).unwrap().key, "gpt-5-mini");
        assert_eq!(
            recognize("openrouter/deepseek/deepseek-chat-v3.1:free", None)
                .unwrap()
                .key,
            "deepseek-chat"
        );
        assert_eq!(
            recognize("models/gemini-2.5-pro", Some("google"))
                .unwrap()
                .key,
            "gemini-2.5-pro"
        );
        assert_eq!(
            recognize("kimi-k2-thinking", None).unwrap().key,
            "kimi-k2-thinking"
        );
        assert!(recognize("my-private-model", None).is_none());
        assert!(recognize("", None).is_none());
    }

    #[test]
    fn references_put_the_recognized_model_first_then_popular_ones() {
        let list = references(Field::Context, Some("deepseek-reasoner"));
        assert_eq!(list[0].key, "deepseek-reasoner");
        assert!(list[0].recognized);
        assert_eq!(list[0].label, "128K");
        let popular: Vec<&str> = entries()
            .iter()
            .filter(|e| e.popular)
            .map(|e| e.key.as_str())
            .collect();
        let rest: Vec<&str> = list[1..].iter().map(|r| r.key.as_str()).collect();
        assert_eq!(rest, popular);
        let unknown = references(Field::Thinking, Some("nothing-known"));
        assert!(unknown.iter().all(|r| !r.recognized));
        assert_eq!(unknown.len(), popular.len());
    }

    #[test]
    fn filling_only_touches_unset_fields() {
        let mut fetched = json!({"id":"deepseek-chat","thinking":["off"],"default_thinking":"off",
            "capabilities":{"input":["text"]}});
        assert!(fill_model(&mut fetched, None));
        assert_eq!(fetched["limits"]["context_window_tokens"], 128000);
        assert_eq!(fetched["thinking"], json!(["off", "high"]));
        let mut configured = json!({"id":"deepseek-chat","thinking":["off","low"],
            "default_thinking":"low","limits":{"context_window_tokens":64000,"max_output_tokens":4000}});
        let before = configured.clone();
        assert!(!fill_model(&mut configured, None));
        assert_eq!(configured, before);

        let (input, entry) = fill_input(
            ModelInput {
                id: "glm-4.6".into(),
                output: "16K".into(),
                ..Default::default()
            },
            None,
        );
        assert!(entry.is_some());
        assert_eq!(input.context, "200K");
        assert_eq!(input.output, "16K", "user value stays");
        assert_eq!(
            input.thinking_scheme,
            Some(ThinkingScheme::Toggle {
                on: "high".into(),
                default_on: true
            })
        );
        assert_eq!(model_summary(&fetched), "128K · 输出 8K · 思考开关");
    }
}
