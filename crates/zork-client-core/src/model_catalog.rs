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

/// A recognized id: the entry, and whether the id names it exactly (as
/// opposed to a dated/suffixed variant such as `gpt-5-2025-08-07`).
#[derive(Clone, Copy, Debug)]
pub struct Recognition {
    pub entry: &'static Entry,
    pub exact: bool,
}

fn dotless(value: &str) -> String {
    value.replace('.', "-")
}

fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.to_ascii_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// Recognizes an id and says how: exactly (the preset's own id, key or name)
/// or as a variant (longest prefix / snapshot suffix).
pub fn recognize_match(id: &str, provider: Option<&str>) -> Option<Recognition> {
    let entry = recognize(id, provider)?;
    let normalized = dotless(&normalize(id));
    let exact = entry
        .matching
        .ids
        .iter()
        .map(String::as_str)
        .chain(
            entry
                .matching
                .prefixes
                .iter()
                .map(|p| p.trim_end_matches('-')),
        )
        .chain([entry.key.as_str()])
        .map(dotless)
        .chain([slug(&entry.name)])
        .any(|candidate| candidate == normalized);
    Some(Recognition { entry, exact })
}

impl Entry {
    /// The id to suggest for this preset: its key when that is one of its
    /// ids, otherwise its first id or prefix — always one that is recognized
    /// back as exactly this preset.
    pub fn canonical_id(&self) -> String {
        static IDS: OnceLock<std::collections::HashMap<String, String>> = OnceLock::new();
        IDS.get_or_init(|| {
            entries()
                .iter()
                .map(|e| (e.key.clone(), e.compute_canonical_id()))
                .collect()
        })
        .get(&self.key)
        .cloned()
        .unwrap_or_else(|| self.compute_canonical_id())
    }
    fn compute_canonical_id(&self) -> String {
        let key = [self.key.as_str()];
        let patterns = self.matching.ids.iter().map(String::as_str).chain(
            self.matching
                .prefixes
                .iter()
                .map(|p| p.trim_end_matches('-')),
        );
        let back = |id: &&str| {
            recognize_match(id, Some(&self.provider))
                .is_some_and(|r| r.exact && r.entry.key == self.key)
        };
        key.into_iter()
            .chain(patterns)
            .find(back)
            .unwrap_or(&self.key)
            .to_owned()
    }
    /// The values this preset fills into an editor.
    pub fn values(&self) -> Values {
        Values {
            scheme: self.thinking.clone(),
            context: self.context,
            output: u64::from(self.output),
            image: self.capabilities.image,
        }
    }
}

/// The three editable sections a preset or another model can fill.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Values {
    pub scheme: ThinkingScheme,
    pub context: u64,
    pub output: u64,
    pub image: bool,
}

impl Values {
    /// Values of a stored model, when it is configured with usable limits.
    pub fn of_model(model: &Value) -> Option<Self> {
        if !crate::model_edit::copyable(model) {
            return None;
        }
        Some(Self {
            scheme: stored_scheme(model),
            context: model["limits"]["context_window_tokens"].as_u64()?,
            output: model["limits"]["max_output_tokens"].as_u64()?,
            image: crate::model_edit::model_accepts_images(model),
        })
    }
    /// `128K · 可开关`
    pub fn meta(&self) -> String {
        format!(
            "{} · {}",
            compact_tokens(self.context),
            self.scheme.kind().short()
        )
    }
    pub fn length_short(&self) -> String {
        format!(
            "上下文 {} · 最长输出 {}",
            compact_tokens(self.context),
            compact_tokens(self.output)
        )
    }
    pub fn image_short(&self) -> &'static str {
        image_label(self.image)
    }
}

pub fn image_label(image: bool) -> &'static str {
    if image {
        "能看图片"
    } else {
        "不能看图片"
    }
}

/// How a connection relates to the preset catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionKind {
    /// A vendor's own API: presets of that catalog provider.
    Direct(&'static str),
    /// A router serving many vendors. `vendor_ids` routers (OpenRouter) name
    /// models `vendor/model`.
    Aggregator { vendor_ids: bool },
    /// A user-supplied base URL: any preset, and the protocol is editable.
    Custom,
}

pub fn connection_kind(provider: &str) -> ConnectionKind {
    match provider {
        "openai" => ConnectionKind::Direct("openai"),
        "anthropic" => ConnectionKind::Direct("anthropic"),
        "deepseek" => ConnectionKind::Direct("deepseek"),
        "xai" => ConnectionKind::Direct("xai"),
        "kimi-coding" | "moonshot" => ConnectionKind::Direct("moonshot"),
        "openrouter" => ConnectionKind::Aggregator { vendor_ids: true },
        "openai-compatible" => ConnectionKind::Custom,
        _ => ConnectionKind::Aggregator { vendor_ids: false },
    }
}

/// The catalog provider a connection provider serves, for recognition ties.
pub fn catalog_provider(provider: &str) -> Option<&'static str> {
    match connection_kind(provider) {
        ConnectionKind::Direct(p) => Some(p),
        _ => None,
    }
}

/// OpenRouter-style vendor namespace of a catalog provider.
fn vendor_namespace(provider: &str) -> &str {
    match provider {
        "moonshot" => "moonshotai",
        "zhipu" => "z-ai",
        "xai" => "x-ai",
        "mistral" => "mistralai",
        "meta" => "meta-llama",
        "doubao" => "bytedance-seed",
        other => other,
    }
}

/// A connection as the editor sees it (a profile detail: `profile_id`,
/// `name`, `provider`, `billing`, `models`). Unknown fields are ignored.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ConnectionInfo {
    #[serde(default)]
    pub profile_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub billing: Option<String>,
    #[serde(default)]
    pub models: Vec<Value>,
}

impl ConnectionInfo {
    pub fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or(&self.profile_id)
    }
    pub fn kind(&self) -> ConnectionKind {
        connection_kind(&self.provider)
    }
    fn has_model(&self, id: &str) -> bool {
        self.models
            .iter()
            .any(|m| m["id"].as_str().is_some_and(|m| m.eq_ignore_ascii_case(id)))
    }
    fn catalog_provider(&self) -> Option<&'static str> {
        catalog_provider(&self.provider)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Suggestion {
    pub id: String,
    /// Preset name, or empty for an unrecognized reported id.
    pub name: String,
    /// `128K · 档位`, or `没有预设`.
    pub meta: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SuggestionGroup {
    /// `reported` (from the provider's model list) or `presets`.
    pub kind: String,
    pub title: String,
    pub items: Vec<Suggestion>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Suggestions {
    pub groups: Vec<SuggestionGroup>,
    /// `把 “q” 当作自定义模型` when the query is not exactly one of the items.
    pub custom: Option<CustomSuggestion>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CustomSuggestion {
    pub id: String,
    pub label: String,
}

impl Suggestions {
    /// Every pickable id in keyboard order, the custom row last.
    pub fn flat(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .groups
            .iter()
            .flat_map(|g| g.items.iter().map(|i| i.id.clone()))
            .collect();
        ids.extend(self.custom.iter().map(|c| c.id.clone()));
        ids
    }
}

pub const SUGGESTION_GROUP_LIMIT: usize = 6;

fn query_rank(query: &str, id: &str, name: &str) -> Option<u8> {
    if query.is_empty() {
        return Some(1);
    }
    let id = id.to_lowercase();
    let name = name.to_lowercase();
    if id.starts_with(query) || name.starts_with(query) {
        Some(0)
    } else if id.contains(query) || name.contains(query) {
        Some(1)
    } else {
        None
    }
}

fn entry_meta(entry: &Entry) -> String {
    entry.values().meta()
}

/// Ids to offer while typing a new model's id: what this connection reported
/// (not yet added), then presets for its provider (every provider for routers
/// and custom base URLs). Case-insensitive substring on id and name; each
/// group is capped; ids already in the connection are left out.
pub fn id_suggestions(
    query: &str,
    connection: &ConnectionInfo,
    reported: &[String],
) -> Suggestions {
    let typed = query.trim();
    let query = typed.to_lowercase();
    let provider = connection.catalog_provider();
    let mut groups = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    let mut reported_items: Vec<(u8, usize, Suggestion)> = reported
        .iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty() && !connection.has_model(id))
        .enumerate()
        .filter_map(|(index, id)| {
            let recognition = recognize(id, provider);
            let name = recognition.map(|e| e.name.clone()).unwrap_or_default();
            let rank = query_rank(&query, id, &name)?;
            Some((
                rank,
                index,
                Suggestion {
                    id: id.to_owned(),
                    meta: recognition
                        .map(entry_meta)
                        .unwrap_or_else(|| "没有预设".into()),
                    name,
                },
            ))
        })
        .collect();
    reported_items.sort_by_key(|(rank, index, _)| (*rank, *index));
    reported_items.dedup_by(|a, b| a.2.id.eq_ignore_ascii_case(&b.2.id));
    let reported_items: Vec<Suggestion> = reported_items
        .into_iter()
        .map(|(_, _, s)| s)
        .take(SUGGESTION_GROUP_LIMIT)
        .collect();
    seen.extend(reported_items.iter().map(|s| s.id.to_lowercase()));
    // Ids the provider reported are offered there even beyond the cap.
    seen.extend(reported.iter().map(|id| id.trim().to_lowercase()));
    if !reported_items.is_empty() {
        groups.push(SuggestionGroup {
            kind: "reported".into(),
            title: format!("{} 上可用", connection.display_name()),
            items: reported_items,
        });
    }

    let kind = connection.kind();
    let mut presets: Vec<(u8, bool, usize, Suggestion)> = entries()
        .iter()
        .enumerate()
        .filter(|(_, e)| match kind {
            ConnectionKind::Direct(p) => e.provider == p,
            _ => true,
        })
        .filter_map(|(index, entry)| {
            let bare = entry.canonical_id();
            let id = match kind {
                ConnectionKind::Aggregator { vendor_ids: true } => {
                    format!("{}/{bare}", vendor_namespace(&entry.provider))
                }
                _ => bare,
            };
            let rank = query_rank(&query, &id, &entry.name)
                .or_else(|| query_rank(&query, &entry.key, "").map(|_| 1))?;
            if connection.has_model(&id) || seen.contains(&id.to_lowercase()) {
                return None;
            }
            Some((
                rank,
                !entry.popular,
                index,
                Suggestion {
                    id,
                    name: entry.name.clone(),
                    meta: entry_meta(entry),
                },
            ))
        })
        .collect();
    presets.sort_by_key(|(rank, unpopular, index, _)| (*rank, *unpopular, *index));
    let presets: Vec<Suggestion> = presets
        .into_iter()
        .map(|(_, _, _, s)| s)
        .take(SUGGESTION_GROUP_LIMIT)
        .collect();
    if !presets.is_empty() {
        groups.push(SuggestionGroup {
            kind: "presets".into(),
            title: "预设".into(),
            items: presets,
        });
    }
    let exact = groups
        .iter()
        .flat_map(|g| &g.items)
        .any(|i| i.id.eq_ignore_ascii_case(typed));
    Suggestions {
        groups,
        custom: (!typed.is_empty() && !exact).then(|| CustomSuggestion {
            id: typed.to_owned(),
            label: format!("把 “{typed}” 当作自定义模型"),
        }),
    }
}

/// Where a fill came from: a preset, or a model in some connection.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum SourceRef {
    Preset { preset: String },
    Model { profile: String, model: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FillSource {
    #[serde(rename = "ref")]
    pub reference: SourceRef,
    /// Model id or preset name.
    pub label: String,
    /// Connection name for models; empty for presets.
    pub sub: String,
    /// `128K · 可开关 · 输出 8K`
    pub meta: String,
    pub values: Values,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FillSourceGroup {
    /// `models` (你的模型) or `presets` (预设).
    pub kind: String,
    pub title: String,
    pub items: Vec<FillSource>,
}

pub const SOURCE_MODEL_LIMIT: usize = 5;
pub const SOURCE_PRESET_LIMIT: usize = 8;

fn source_meta(values: &Values) -> String {
    format!("{} · 输出 {}", values.meta(), compact_tokens(values.output))
}

/// Candidates for “从相似模型填入 / 换一个来源”: configured models in any
/// connection (except the one being edited), then presets.
pub fn fill_sources(
    query: &str,
    connections: &[ConnectionInfo],
    editing: Option<(&str, &str)>,
) -> Vec<FillSourceGroup> {
    let query = query.trim().to_lowercase();
    let mut models = Vec::new();
    for connection in connections {
        for model in &connection.models {
            let Some(id) = model["id"].as_str() else {
                continue;
            };
            if editing == Some((connection.profile_id.as_str(), id)) {
                continue;
            }
            let Some(values) = Values::of_model(model) else {
                continue;
            };
            if query_rank(&query, id, connection.display_name()).is_none() {
                continue;
            }
            models.push(FillSource {
                reference: SourceRef::Model {
                    profile: connection.profile_id.clone(),
                    model: id.to_owned(),
                },
                label: id.to_owned(),
                sub: connection.display_name().to_owned(),
                meta: source_meta(&values),
                values,
            });
            if models.len() == SOURCE_MODEL_LIMIT {
                break;
            }
        }
        if models.len() == SOURCE_MODEL_LIMIT {
            break;
        }
    }
    let mut presets: Vec<(u8, bool, usize, &Entry)> = entries()
        .iter()
        .enumerate()
        .filter_map(|(index, e)| {
            let rank = query_rank(&query, &e.canonical_id(), &e.name)
                .or_else(|| query_rank(&query, &e.key, &e.family))?;
            Some((rank, !e.popular, index, e))
        })
        .collect();
    presets.sort_by_key(|(rank, unpopular, index, _)| (*rank, *unpopular, *index));
    let presets: Vec<FillSource> = presets
        .into_iter()
        .take(SOURCE_PRESET_LIMIT)
        .map(|(_, _, _, e)| {
            let values = e.values();
            FillSource {
                reference: SourceRef::Preset {
                    preset: e.key.clone(),
                },
                label: e.name.clone(),
                sub: String::new(),
                meta: source_meta(&values),
                values,
            }
        })
        .collect();
    let mut groups = Vec::new();
    if !models.is_empty() {
        groups.push(FillSourceGroup {
            kind: "models".into(),
            title: "你的模型".into(),
            items: models,
        });
    }
    if !presets.is_empty() {
        groups.push(FillSourceGroup {
            kind: "presets".into(),
            title: "预设".into(),
            items: presets,
        });
    }
    groups
}

/// Resolves a fill source reference: its display name and values.
pub fn resolve_source(
    reference: &SourceRef,
    connections: &[ConnectionInfo],
) -> Option<(String, Values)> {
    match reference {
        SourceRef::Preset { preset } => entries()
            .iter()
            .find(|e| e.key == *preset)
            .map(|e| (e.name.clone(), e.values())),
        SourceRef::Model { profile, model } => connections
            .iter()
            .find(|c| c.profile_id == *profile)?
            .models
            .iter()
            .find(|m| m["id"] == model.as_str())
            .and_then(Values::of_model)
            .map(|v| (model.clone(), v)),
    }
}

pub fn preset(key: &str) -> Option<&'static Entry> {
    entries().iter().find(|e| e.key == key)
}

/// One row of a connection's model list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModelRow {
    pub id: String,
    /// `128K · 可开关`; empty while unconfigured.
    pub meta: String,
    /// Configured and identical to its recognized preset (`按预设`).
    pub preset: bool,
    /// No limits yet (`待配置`); cannot be enabled.
    pub unconfigured: bool,
    pub enabled: bool,
}

pub fn model_row(model: &Value, provider: &str) -> ModelRow {
    let id = model["id"].as_str().unwrap_or_default().to_owned();
    let values = Values::of_model(model);
    let unconfigured = !model["limits"].is_object();
    let preset = values.as_ref().is_some_and(|values| {
        recognize(&id, catalog_provider(provider)).is_some_and(|e| e.values() == *values)
    });
    let meta = if unconfigured {
        String::new()
    } else {
        match &values {
            Some(values) => values.meta(),
            None => model["limits"]["context_window_tokens"]
                .as_u64()
                .map(|c| {
                    format!(
                        "{} · {}",
                        compact_tokens(c),
                        stored_scheme(model).kind().short()
                    )
                })
                .unwrap_or_else(|| stored_scheme(model).kind().short().into()),
        }
    };
    ModelRow {
        enabled: model["enabled"].as_bool().unwrap_or(true),
        id,
        meta,
        preset,
        unconfigured,
    }
}

/// Result of applying presets to models a provider listing just added.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ImportCounts {
    /// New models in the connection.
    pub added: usize,
    /// New models that got complete preset values.
    pub preset: usize,
    /// New models still without limits (待配置).
    pub pending: usize,
}

impl ImportCounts {
    /// `获取到 N 个新模型：a 个已按预设填好，b 个待配置` / `没有新模型`.
    pub fn message(&self) -> String {
        if self.added == 0 {
            return "没有新模型".into();
        }
        let ready = self.added - self.pending;
        let mut parts = Vec::new();
        if ready > 0 {
            parts.push(if ready == self.preset {
                format!("{ready} 个已按预设填好")
            } else {
                format!("{ready} 个已填好")
            });
        }
        if self.pending > 0 {
            parts.push(format!("{} 个待配置", self.pending));
        }
        format!("获取到 {} 个新模型：{}", self.added, parts.join("，"))
    }
}

/// Fills models that were not in `before` and have no limits from their
/// recognized preset (limits, thinking, image). Filled models stay disabled
/// until the user turns them on. Returns what happened for the toast.
pub fn import_presets(before: &[Value], models: &mut [Value], provider: &str) -> ImportCounts {
    let mut counts = ImportCounts::default();
    let provider = catalog_provider(provider);
    for model in models.iter_mut() {
        if before.iter().any(|b| b["id"] == model["id"]) {
            continue;
        }
        counts.added += 1;
        if !model["limits"].is_object() {
            if let Some(entry) = model["id"].as_str().and_then(|id| recognize(id, provider)) {
                apply_values(model, &entry.values());
                model["enabled"] = json!(false);
                counts.preset += 1;
            }
        }
        if !model["limits"].is_object() {
            counts.pending += 1;
        }
    }
    counts
}

/// Writes editor values into a stored model.
pub fn apply_values(model: &mut Value, values: &Values) {
    if !model["limits"].is_object() {
        model["limits"] = json!({});
    }
    model["limits"]["context_window_tokens"] = json!(values.context);
    model["limits"]["max_output_tokens"] = json!(values.output);
    let (thinking, default) = values.scheme.encode();
    model["thinking"] = json!(thinking);
    model["default_thinking"] = json!(default);
    crate::model_edit::set_model_image_input(model, values.image);
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
/// `exact` is false for variants (`gpt-5-2025-08-07` → GPT-5); `provider` may
/// be a connection provider (`kimi-coding`) or a catalog provider.
pub fn recognition(id: &str, provider: Option<&str>, input: Option<ModelInput>) -> Value {
    let provider = provider.map(|p| catalog_provider(p).unwrap_or(p));
    let found = recognize_match(id, provider);
    let entry = found.map(|r| r.entry);
    let input = input.map(|mut input| {
        input.id = id.to_owned();
        fill_input(input, provider).0
    });
    json!({
        "entry": entry.map(Entry::view),
        "exact": found.map(|r| r.exact),
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
    fn connection(provider: &str, models: Vec<Value>) -> ConnectionInfo {
        ConnectionInfo {
            profile_id: format!("my-{provider}"),
            name: Some(format!("My {provider}")),
            provider: provider.into(),
            billing: Some("usage".into()),
            models,
        }
    }

    #[test]
    fn recognition_says_whether_the_id_is_exact_or_a_variant() {
        let how = |id: &str| recognize_match(id, None).map(|r| (r.entry.key.as_str(), r.exact));
        assert_eq!(how("gpt-5"), Some(("gpt-5", true)));
        assert_eq!(how("GPT-5"), Some(("gpt-5", true)));
        assert_eq!(how("openai/gpt-5"), Some(("gpt-5", true)), "vendor prefix");
        assert_eq!(how("openrouter/openai/gpt-5"), Some(("gpt-5", true)));
        assert_eq!(how("gpt-5-2025-08-07"), Some(("gpt-5", false)));
        assert_eq!(
            how("gpt-5-mini"),
            Some(("gpt-5-mini", true)),
            "no sibling match"
        );
        assert_eq!(how("gpt-5-nano"), Some(("gpt-5-nano", true)));
        assert_eq!(how("gpt-5-mini-2025-08-07"), Some(("gpt-5-mini", false)));
        assert_eq!(how("deepseek/deepseek-chat"), Some(("deepseek-chat", true)));
        assert_eq!(how("deepseek-reasoner"), Some(("deepseek-reasoner", true)));
        // Matching by the preset's display name counts as exact.
        assert_eq!(how("claude-sonnet-4-5"), Some(("claude-sonnet", true)));
        assert_eq!(
            how("anthropic/claude-sonnet-4.5"),
            Some(("claude-sonnet", true))
        );
        assert_eq!(
            how("claude-sonnet-4-5-20250929"),
            Some(("claude-sonnet", false))
        );
        assert!(how("gpt-6").is_none());
        assert!(how("my-model").is_none());
        assert!(how("").is_none());
        assert!(how("   ").is_none());
    }

    #[test]
    fn every_preset_suggests_an_id_that_is_recognized_back() {
        for entry in entries() {
            let id = entry.canonical_id();
            let found = recognize_match(&id, Some(&entry.provider))
                .unwrap_or_else(|| panic!("{} suggests unknown id {id}", entry.key));
            assert_eq!(found.entry.key, entry.key, "{id}");
            assert!(found.exact, "{id} should be exact for {}", entry.key);
            let routed = format!("{}/{id}", vendor_namespace(&entry.provider));
            assert_eq!(
                recognize(&routed, None).map(|e| e.key.as_str()),
                Some(entry.key.as_str()),
                "{routed}"
            );
            assert_eq!(entry.values().scheme, entry.thinking);
        }
    }

    #[test]
    fn connection_kinds_map_providers_to_catalog_scopes() {
        assert_eq!(connection_kind("openai"), ConnectionKind::Direct("openai"));
        assert_eq!(
            connection_kind("kimi-coding"),
            ConnectionKind::Direct("moonshot")
        );
        assert_eq!(
            connection_kind("openrouter"),
            ConnectionKind::Aggregator { vendor_ids: true }
        );
        assert_eq!(
            connection_kind("github-copilot"),
            ConnectionKind::Aggregator { vendor_ids: false }
        );
        assert_eq!(connection_kind("openai-compatible"), ConnectionKind::Custom);
        assert_eq!(catalog_provider("anthropic"), Some("anthropic"));
        assert_eq!(catalog_provider("openrouter"), None);
    }

    #[test]
    fn id_suggestions_group_reported_then_presets() {
        let deepseek = connection("deepseek", vec![json!({"id":"deepseek-chat"})]);
        let reported: Vec<String> = ["deepseek-chat", "deepseek-reasoner", "deepseek-internal-x"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let s = id_suggestions("", &deepseek, &reported);
        assert_eq!(s.groups[0].kind, "reported");
        assert_eq!(s.groups[0].title, "My deepseek 上可用");
        let ids: Vec<&str> = s.groups[0].items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(
            ids,
            ["deepseek-reasoner", "deepseek-internal-x"],
            "added ids left out"
        );
        assert_eq!(s.groups[0].items[0].name, "DeepSeek Reasoner");
        assert!(s.groups[0].items[0].meta.contains("总是思考"));
        assert_eq!(s.groups[0].items[1].meta, "没有预设");
        assert_eq!(s.groups[1].kind, "presets");
        for item in &s.groups[1].items {
            let entry = recognize(&item.id, None).unwrap();
            assert_eq!(entry.provider, "deepseek", "{}", item.id);
            assert_ne!(item.id, "deepseek-chat");
            assert_ne!(item.id, "deepseek-reasoner", "already offered as reported");
        }
        assert!(s.custom.is_none());

        // Substring on id and name, case-insensitive; capped; custom row.
        let s = id_suggestions("DEEPS", &deepseek, &[]);
        assert!(!s.groups.is_empty());
        assert!(s
            .groups
            .iter()
            .all(|g| g.items.len() <= SUGGESTION_GROUP_LIMIT));
        assert_eq!(
            s.custom.as_ref().unwrap().label,
            "把 “DEEPS” 当作自定义模型"
        );
        let s = id_suggestions("reasoner", &deepseek, &[]);
        assert_eq!(s.groups[0].items[0].id, "deepseek-reasoner");
        assert!(s.custom.is_some());
        let s = id_suggestions("DeepSeek-Reasoner", &deepseek, &[]);
        assert!(s.custom.is_none(), "exact match needs no custom row");
        assert_eq!(s.flat(), vec!["deepseek-reasoner".to_owned()]);
        let s = id_suggestions("nothing-like-this", &deepseek, &[]);
        assert!(s.groups.is_empty());
        assert_eq!(s.flat(), vec!["nothing-like-this".to_owned()]);

        // Routers list every provider as vendor/model; custom URLs bare ids.
        let router = connection("openrouter", vec![]);
        let s = id_suggestions("kimi", &router, &[]);
        assert!(s.groups[0]
            .items
            .iter()
            .all(|i| i.id.starts_with("moonshotai/")));
        let s = id_suggestions("claude", &router, &[]);
        assert!(s.groups[0]
            .items
            .iter()
            .all(|i| i.id.starts_with("anthropic/")));
        let custom = connection("openai-compatible", vec![]);
        let s = id_suggestions("qwen", &custom, &[]);
        assert!(s.groups[0].items.iter().all(|i| !i.id.contains('/')));
        assert!(!s.groups[0].items.is_empty());
        let s = id_suggestions("", &custom, &[]);
        assert_eq!(s.groups[0].items.len(), SUGGESTION_GROUP_LIMIT);
        let openai = connection("openai", vec![]);
        let s = id_suggestions("claude", &openai, &[]);
        assert!(
            s.groups.is_empty(),
            "direct connections only offer their own presets"
        );
    }

    #[test]
    fn fill_sources_list_other_configured_models_then_presets() {
        let configured = json!({"id":"qwen3-32b","limits":{"context_window_tokens":128000,"max_output_tokens":16000},
            "thinking":["off","high"],"default_thinking":"high","capabilities":{"input":["text"]}});
        let editing =
            json!({"id":"mine","limits":{"context_window_tokens":64000,"max_output_tokens":8000}});
        let unconfigured = json!({"id":"raw"});
        let a = connection(
            "openai-compatible",
            vec![configured.clone(), editing, unconfigured],
        );
        let b = connection(
            "deepseek",
            vec![json!({"id":"deepseek-chat",
            "limits":{"context_window_tokens":128000,"max_output_tokens":8000}})],
        );
        let groups = fill_sources(
            "",
            &[a.clone(), b.clone()],
            Some(("my-openai-compatible", "mine")),
        );
        assert_eq!(groups[0].title, "你的模型");
        let labels: Vec<&str> = groups[0].items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["qwen3-32b", "deepseek-chat"]);
        assert_eq!(groups[0].items[0].sub, "My openai-compatible");
        assert_eq!(groups[0].items[0].meta, "128K · 可开关 · 输出 16K");
        assert_eq!(groups[1].title, "预设");
        assert!(groups[1].items.len() <= SOURCE_PRESET_LIMIT);
        let groups = fill_sources("sonnet", &[a.clone(), b.clone()], None);
        assert_eq!(groups.len(), 1);
        assert!(groups[0].items.iter().all(|i| i.label.contains("Sonnet")));
        let groups = fill_sources("my deep", &[a.clone(), b.clone()], None);
        assert_eq!(
            groups[0].items[0].label, "deepseek-chat",
            "matches connection name"
        );

        let (name, values) = resolve_source(
            &SourceRef::Model {
                profile: "my-openai-compatible".into(),
                model: "qwen3-32b".into(),
            },
            &[a.clone()],
        )
        .unwrap();
        assert_eq!(name, "qwen3-32b");
        assert_eq!(values.context, 128_000);
        assert!(resolve_source(
            &SourceRef::Model {
                profile: "my-openai-compatible".into(),
                model: "raw".into(),
            },
            &[a],
        )
        .is_none());
        let (name, _) = resolve_source(
            &SourceRef::Preset {
                preset: "gpt-5".into(),
            },
            &[],
        )
        .unwrap();
        assert_eq!(name, "GPT-5");
        assert!(resolve_source(
            &SourceRef::Preset {
                preset: "nope".into()
            },
            &[]
        )
        .is_none());
        // References serialize as simple objects for the clients.
        assert_eq!(
            serde_json::to_value(SourceRef::Preset { preset: "x".into() }).unwrap(),
            json!({"preset":"x"})
        );
        assert_eq!(
            serde_json::from_value::<SourceRef>(json!({"profile":"p","model":"m"})).unwrap(),
            SourceRef::Model {
                profile: "p".into(),
                model: "m".into()
            }
        );
    }

    #[test]
    fn rows_mark_preset_derived_and_unconfigured_models() {
        let mut model = json!({"id":"deepseek-chat","enabled":true});
        let row = model_row(&model, "deepseek");
        assert!(row.unconfigured && !row.preset);
        assert_eq!(row.meta, "");
        apply_values(
            &mut model,
            &recognize("deepseek-chat", None).unwrap().values(),
        );
        let row = model_row(&model, "deepseek");
        assert!(row.preset && !row.unconfigured && row.enabled);
        assert_eq!(row.meta, "128K · 可开关");
        model["limits"]["max_output_tokens"] = json!(4000);
        assert!(!model_row(&model, "deepseek").preset, "modified");
        let custom = json!({"id":"my-model","enabled":false,
            "limits":{"context_window_tokens":32768,"max_output_tokens":4096},"thinking":["off"],"default_thinking":"off"});
        let row = model_row(&custom, "openai-compatible");
        assert_eq!(row.meta, "32,768 · 不思考");
        assert!(!row.preset && !row.enabled);
    }

    #[test]
    fn imports_fill_recognized_models_and_count_the_rest() {
        let before = vec![
            json!({"id":"deepseek-chat","limits":{"context_window_tokens":1,"max_output_tokens":0}}),
        ];
        let mut models = vec![
            before[0].clone(),
            json!({"id":"deepseek-reasoner","enabled":true,"thinking":["off"],"default_thinking":"off"}),
            json!({"id":"deepseek-internal-x","enabled":false}),
            json!({"id":"provider-configured","enabled":true,
                "limits":{"context_window_tokens":64000,"max_output_tokens":8000}}),
        ];
        let counts = import_presets(&before, &mut models, "deepseek");
        assert_eq!(
            counts,
            ImportCounts {
                added: 3,
                preset: 1,
                pending: 1
            }
        );
        assert_eq!(models[0], before[0], "existing models are untouched");
        assert_eq!(models[1]["enabled"], false, "disabled until turned on");
        assert_eq!(models[1]["limits"]["context_window_tokens"], 128000);
        assert_eq!(models[1]["thinking"], json!(["high"]));
        assert!(model_row(&models[1], "deepseek").preset);
        assert!(model_row(&models[2], "deepseek").unconfigured);
        assert_eq!(
            counts.message(),
            "获取到 3 个新模型：2 个已填好，1 个待配置"
        );
        assert_eq!(
            ImportCounts {
                added: 2,
                preset: 2,
                pending: 0
            }
            .message(),
            "获取到 2 个新模型：2 个已按预设填好"
        );
        assert_eq!(
            ImportCounts {
                added: 3,
                preset: 1,
                pending: 2
            }
            .message(),
            "获取到 3 个新模型：1 个已按预设填好，2 个待配置"
        );
        assert_eq!(
            ImportCounts {
                added: 1,
                preset: 0,
                pending: 1
            }
            .message(),
            "获取到 1 个新模型：1 个待配置"
        );
        assert_eq!(ImportCounts::default().message(), "没有新模型");
    }
}
