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

/// Cloud and router namespaces that precede the model name with a dot, as in
/// Bedrock's `us.anthropic.claude-…` or `meta.llama4-…`.
const DOTTED_NAMESPACES: [&str; 20] = [
    "us",
    "eu",
    "apac",
    "ap",
    "jp",
    "au",
    "ca",
    "global",
    "us-gov",
    "anthropic",
    "meta",
    "mistral",
    "deepseek",
    "qwen",
    "openai",
    "moonshotai",
    "minimax",
    "zai",
    "google",
    "xai",
];
/// `:tag` suffixes that name a routing variant rather than a model size.
const VARIANT_TAGS: [&str; 8] = [
    "free", "beta", "extended", "nitro", "floor", "online", "exacto", "latest",
];

/// Reduces a provider-specific id to the bare model name: router prefixes
/// (`openrouter/deepseek/…`, `models/…`, `us.anthropic.`), variant tags
/// (`:free`, Bedrock `:0`, Vertex `@20251001`) and version suffixes (`-v1`)
/// go away. Ollama-style size tags (`qwen3:32b`) become part of the name, and
/// Claude/Doubao dotted versions use the dashed form (`claude-sonnet-4.5` →
/// `claude-sonnet-4-5`).
pub fn normalize(id: &str) -> String {
    let id = id.trim().to_ascii_lowercase();
    let mut id = id.rsplit('/').next().unwrap_or(&id).to_owned();
    if let Some(at) = id.find('@') {
        id.truncate(at);
    }
    if let Some((name, tag)) = id.split_once(':') {
        id = if tag.is_empty()
            || VARIANT_TAGS.contains(&tag)
            || tag.chars().all(|c| c.is_ascii_digit())
        {
            name.to_owned()
        } else {
            format!("{name}-{}", tag.replace(':', "-"))
        };
    }
    while let Some((namespace, rest)) = id.split_once('.') {
        if !DOTTED_NAMESPACES.contains(&namespace) || rest.is_empty() {
            break;
        }
        id = rest.to_owned();
    }
    for suffix in ["-v1", "-v2"] {
        if let Some(stripped) = id.strip_suffix(suffix) {
            id = stripped.to_owned();
        }
    }
    if id.starts_with("claude") || id.starts_with("doubao") {
        id = id.replace('.', "-");
    }
    id
}

/// The normalized id followed by shorter forms without trailing `-latest`,
/// `-preview`, `-exp` or date/snapshot segments, so `gpt-5.4-2026-03-05`
/// can fall back to `gpt-5.4`.
fn candidates(id: &str) -> Vec<String> {
    let mut out = vec![id.to_owned()];
    let mut parts: Vec<&str> = id.split('-').collect();
    while parts.len() > 1 {
        let last = parts[parts.len() - 1];
        let snapshot = last.len() >= 2 && last.chars().all(|c| c.is_ascii_digit());
        if !(snapshot || matches!(last, "latest" | "preview" | "exp")) {
            break;
        }
        parts.pop();
        out.push(parts.join("-"));
    }
    out
}

/// The catalog entry for a model id, if any. The longest matching id or prefix
/// wins, so `gpt-5.1-codex` is Codex rather than GPT-5; `provider` breaks ties.
/// When nothing matches, the id is retried without snapshot suffixes.
pub fn recognize(id: &str, provider: Option<&str>) -> Option<&'static Entry> {
    let id = normalize(id);
    if id.is_empty() {
        return None;
    }
    candidates(&id)
        .iter()
        .find_map(|candidate| best_match(candidate, provider))
}

fn best_match(id: &str, provider: Option<&str>) -> Option<&'static Entry> {
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
        assert!(entries().len() >= 100);
        let mut keys = std::collections::HashSet::new();
        let mut patterns = std::collections::HashMap::new();
        for entry in entries() {
            assert!(
                keys.insert(entry.key.as_str()),
                "{} is listed twice",
                entry.key
            );
            assert!(
                entry.source.starts_with("https://"),
                "{} needs a source",
                entry.key
            );
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
            let ids = entry.matching.ids.iter().map(|id| ("id", id));
            let prefixes = entry.matching.prefixes.iter().map(|p| ("prefix", p));
            for (kind, pattern) in ids.chain(prefixes) {
                assert_eq!(
                    pattern,
                    &normalize(pattern),
                    "{} {kind} {pattern} is not normalized",
                    entry.key
                );
                if let Some(other) = patterns.insert((kind, pattern.as_str()), entry.key.as_str()) {
                    panic!("{kind} {pattern} is claimed by {other} and {}", entry.key);
                }
            }
        }
    }

    fn key(id: &str) -> Option<&'static str> {
        recognize(id, None).map(|entry| entry.key.as_str())
    }

    #[test]
    fn recognizes_exact_dated_and_routed_ids() {
        for (id, expected) in [
            ("deepseek-chat", "deepseek-chat"),
            ("claude-sonnet-4-5-20250929", "claude-sonnet"),
            ("claude-opus-4-5-20251101", "claude-opus-4-5"),
            ("gpt-5-2025-08-07", "gpt-5"),
            ("gpt-5.1-codex-mini", "gpt-5.1-codex-mini"),
            ("GPT-5-Mini", "gpt-5-mini"),
            (
                "openrouter/deepseek/deepseek-chat-v3.1:free",
                "deepseek-chat",
            ),
            ("models/gemini-2.5-pro", "gemini-2.5-pro"),
            ("kimi-k2-thinking", "kimi-k2-thinking"),
        ] {
            assert_eq!(key(id), Some(expected), "{id}");
        }
        assert_eq!(
            recognize("models/gemini-2.5-pro", Some("google"))
                .unwrap()
                .key,
            "gemini-2.5-pro"
        );
        assert!(recognize("my-private-model", None).is_none());
        assert!(recognize("", None).is_none());
    }

    #[test]
    fn recognizes_popular_ids_across_providers() {
        for (id, expected) in [
            // OpenAI: siblings share prefixes, snapshots and routers vary.
            ("gpt-5", "gpt-5"),
            ("openai/gpt-5", "gpt-5"),
            ("gpt-5-mini-2025-08-07", "gpt-5-mini"),
            ("gpt-5-nano", "gpt-5-nano"),
            ("gpt-5-pro-2025-10-06", "gpt-5-pro"),
            ("gpt-5-codex", "gpt-5-codex"),
            ("gpt-5.1-codex", "gpt-5-codex"),
            ("gpt-5.1-codex-max", "gpt-5.1-codex-max"),
            ("gpt-5-chat-latest", "gpt-5-chat"),
            ("gpt-5.1-2025-11-13", "gpt-5.1"),
            ("gpt-5.2", "gpt-5.2"),
            ("gpt-5.2-pro-2025-12-11", "gpt-5.2-pro"),
            ("gpt-5.3-codex", "gpt-5.3-codex"),
            ("gpt-5.4-2026-03-05", "gpt-5.4"),
            ("gpt-5.4-mini-2026-03-17", "gpt-5.4-mini"),
            ("gpt-5.4-nano", "gpt-5.4-nano"),
            ("openai/gpt-5.5", "gpt-5.5"),
            ("gpt-5.5-pro-2026-04-23", "gpt-5.5-pro"),
            ("gpt-5.6", "gpt-5.6-sol"),
            ("gpt-5.6-terra", "gpt-5.6-terra"),
            ("gpt-6-sol", "gpt-6-sol"),
            ("gpt-6-astra", "gpt-6-astra"),
            ("o3-2025-04-16", "o3"),
            ("o3-pro", "o3-pro"),
            ("o4-mini", "o4-mini"),
            ("gpt-4.1-mini", "gpt-4.1"),
            ("gpt-4o-2024-11-20", "gpt-4o"),
            ("gpt-4o-mini", "gpt-4o-mini"),
            ("gpt-oss:120b", "gpt-oss-120b"),
            ("openai/gpt-oss-20b", "gpt-oss-20b"),
            // Anthropic: dotted router ids, Bedrock and Vertex forms.
            ("anthropic/claude-sonnet-4.5", "claude-sonnet"),
            ("claude-sonnet-4-6", "claude-sonnet-4-6"),
            ("anthropic/claude-sonnet-4.6", "claude-sonnet-4-6"),
            ("claude-sonnet-5", "claude-sonnet-5"),
            ("claude-opus-5-5", "claude-opus-5-5"),
            ("claude-opus-5", "claude-opus-5"),
            ("claude-opus-4-8", "claude-opus-4-8"),
            ("us.anthropic.claude-opus-4-6-v1", "claude-opus-4-6"),
            ("anthropic.claude-sonnet-4-5-20250929-v1:0", "claude-sonnet"),
            ("global.anthropic.claude-opus-4-7", "claude-opus-4-7"),
            ("claude-haiku-4-5@20251001", "claude-haiku"),
            ("claude-opus-4-1-20250805", "claude-opus"),
            ("claude-fable-5-1", "claude-fable-5-1"),
            ("claude-fable-5", "claude-fable-5"),
            ("claude-3-7-sonnet-latest", "claude-sonnet"),
            // Google
            ("google/gemini-3.5-flash", "gemini-3.5-flash"),
            ("models/gemini-3.5-flash-lite", "gemini-3.5-flash-lite"),
            ("gemini-3.8-flash", "gemini-3.8-flash"),
            ("gemini-3.1-pro-preview-customtools", "gemini-3.1-pro"),
            ("gemini-3-flash-preview", "gemini-3-flash"),
            ("gemini-2.5-flash-lite", "gemini-2.5-flash-lite"),
            ("gemini-2.5-flash", "gemini-2.5-flash"),
            // xAI
            ("x-ai/grok-4.7", "grok-4.7"),
            ("grok-4.3-latest", "grok-4.3"),
            ("grok-4.20-0309-reasoning", "grok-4.20"),
            ("grok-4.20-0309-non-reasoning", "grok-4.20-non-reasoning"),
            ("grok-4-0709", "grok-4"),
            ("grok-code-fast-1", "grok-code-fast"),
            // DeepSeek
            ("deepseek-flash", "deepseek-flash"),
            ("deepseek-v4-flash", "deepseek-flash"),
            ("deepseek-ai/DeepSeek-V4-Pro", "deepseek-v4-pro"),
            ("deepseek-ai/DeepSeek-V3.2", "deepseek-chat"),
            ("deepseek-reasoner", "deepseek-reasoner"),
            // Qwen: hosted and open-weight ids stay apart.
            ("qwen3.8-max-0902", "qwen3.8-max"),
            ("qwen3.7-plus-2026-05-26", "qwen3.7-plus"),
            ("qwen/qwen3.6-plus", "qwen3.6-plus"),
            ("qwen-plus-latest", "qwen-plus"),
            ("qwen3-max", "qwen3-max"),
            ("qwen3-coder-plus", "qwen3-coder"),
            ("qwen3-coder-flash", "qwen3-coder-flash"),
            ("Qwen/Qwen3-Coder-480B-A35B-Instruct", "qwen3-coder-open"),
            ("Qwen/Qwen3-235B-A22B-Instruct-2507", "qwen3-instruct"),
            ("qwen3-235b-a22b-thinking-2507", "qwen3-thinking"),
            ("Qwen/Qwen3-32B", "qwen3-open"),
            ("qwen3:32b", "qwen3-open"),
            // Moonshot
            ("kimi-k3", "kimi-k3"),
            ("moonshotai/Kimi-K2.6", "kimi-k2.6"),
            ("kimi-k2.7-code-highspeed", "kimi-k2.7-code"),
            ("kimi-k2-0905-preview", "kimi-k2"),
            ("moonshotai/Kimi-K2-Instruct-0905", "kimi-k2"),
            // Zhipu
            ("zai-org/GLM-4.6", "glm-4.6"),
            ("glm-4.6v", "glm-4.6v"),
            ("z-ai/glm-4.7", "glm-4.7"),
            ("glm-5", "glm-5"),
            ("glm-5-turbo", "glm-5"),
            ("glm-5v-turbo", "glm-5v-turbo"),
            ("glm-5.1", "glm-5.1"),
            ("glm-5.3", "glm-5.3"),
            // MiniMax
            ("MiniMax-M3", "minimax-m3"),
            ("MiniMaxAI/MiniMax-M2.5", "minimax-m2.5"),
            ("MiniMax-M2.1-highspeed", "minimax-m2"),
            // ByteDance
            ("doubao-seed-2-0-pro-260215", "doubao-seed-2-0-pro"),
            ("doubao-seed-2.0-lite", "doubao-seed-2-0-lite"),
            ("doubao-seed-2-1-pro-260915", "doubao-seed-2-1-pro"),
            ("doubao-seed-2-1-pro-260628", "doubao-seed-2-1-pro-260628"),
            // Mistral
            ("mistralai/mistral-large-2512", "mistral-large"),
            ("mistral-medium-latest", "mistral-medium"),
            ("codestral-2508", "codestral"),
            ("devstral-2512", "devstral"),
            // Meta
            ("meta-llama/Llama-4-Scout-17B-16E-Instruct", "llama-4-scout"),
            ("meta.llama4-maverick-17b-instruct-v1:0", "llama-4-maverick"),
        ] {
            assert_eq!(key(id), Some(expected), "{id}");
        }
    }

    #[test]
    fn unrelated_or_unknown_ids_stay_unrecognized() {
        for id in [
            "gpt-6",
            "gpt-5.6-cyber",
            "claude-opus",
            "gemini",
            "mistralai/Mistral-Small-3.2-24B-Instruct-2506",
            "mistralai/Mistral-Large-Instruct-2411",
            "deepseek-r1:70b",
            "qwen-turbo",
            "codex-something",
            "glm-6",
        ] {
            assert_eq!(key(id), None, "{id}");
        }
    }

    #[test]
    fn normalizes_router_and_cloud_forms() {
        assert_eq!(
            normalize(" us.anthropic.claude-sonnet-4-5-20250929-v1:0 "),
            "claude-sonnet-4-5-20250929"
        );
        assert_eq!(normalize("anthropic/claude-opus-4.6"), "claude-opus-4-6");
        assert_eq!(normalize("qwen3:32b"), "qwen3-32b");
        assert_eq!(normalize("deepseek/deepseek-chat:free"), "deepseek-chat");
        assert_eq!(normalize("gpt-5.1"), "gpt-5.1");
        assert_eq!(normalize("qwen3.5-plus"), "qwen3.5-plus");
        assert_eq!(normalize("claude-haiku-4-5@20251001"), "claude-haiku-4-5");
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
