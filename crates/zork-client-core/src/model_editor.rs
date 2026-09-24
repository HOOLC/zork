//! The model editor shared by every client: one state machine for adding and
//! editing a connection's model. Clients keep an [`EditorState`], forward user
//! edits as [`Action`]s and render the returned [`View`]; they never decide
//! what a field means, how presets fill, or which errors show.
//!
//! JSON surface (`op: "model_editor"`, also `NativeBridge.modelEditor`):
//!
//! ```text
//! request  {"context": Context, "state": EditorState | null, "action": Action}
//! response {"state": EditorState, "view": View,
//!           "effect": null | {"save": ModelInput},   // valid: persist it
//!           "focus": null | "id" | "thinking" | "context" | "output" | "api"}
//! Context  {"profile": {profile_id, name, provider, billing, models},
//!           "providers": [provider catalog], "profiles": [every connection],
//!           "reported": ["ids from the last model discovery"]}
//! Action   {"type": "open_add"} | {"type": "open_edit", "model": "id"}
//!          | {"type": "set_id", "id": "…"} | {"type": "recognize"}   // 250 ms after typing
//!          | {"type": "settle", "id": "…" | null}  // pick, Enter or blur
//!          | {"type": "toggle_section", "section": "thinking"|"length"|"image"|"api"}
//!          | {"type": "restore", "section": …} | {"type": "refill"}
//!          | {"type": "use_source", "source": {"preset": key} | {"profile": p, "model": id}}
//!          | {"type": "set_kind", "kind": "unsupported"|"always"|"toggle"|"levels"|"budget"}
//!          | {"type": "toggle_default", "on": bool}
//!          | {"type": "level_default", "name"} | {"type": "level_delete", "index"}
//!          | {"type": "level_move", "from", "to"} | {"type": "level_add", "name"}
//!          | {"type": "budget_default", "choice": {"kind": "off"|"dynamic"} | {"kind": "tokens", "tokens": n}}
//!          | {"type": "budget_delete", "index"} | {"type": "budget_add", "text": "8K"}
//!          | {"type": "budget_allow_off", "on"} | {"type": "budget_dynamic", "on"}
//!          | {"type": "clear_add_errors"}
//!          | {"type": "set_length", "field": "context"|"output", "text"}
//!          | {"type": "blur_length", "field"} | {"type": "step_length", "field", "up": bool}
//!          | {"type": "pick_length", "field", "value": n}
//!          | {"type": "set_image", "on"} | {"type": "set_api", "api"} | {"type": "save"}
//!          | {"type": "view"}   // re-project after the context changed
//! ```
//!
//! `op: "model_editor_sources"` `{context, state, query}` returns the fill
//! source groups (你的模型 / 预设) for the “从相似模型填入” popover.
use crate::model_catalog::{
    self, catalog_provider, ConnectionInfo, ConnectionKind, FillSourceGroup, SourceRef,
    Suggestions, Values,
};
use crate::model_edit::{
    compact_tokens, exact_tokens, id_error, normalize_tokens, parse_tokens, ModelInput, FIELD_API,
    FIELD_CONTEXT, FIELD_ID, FIELD_OUTPUT, FIELD_THINKING, MODEL_APIS,
};
use crate::thinking::{BudgetChoice, Panel, ThinkingKind, ThinkingScheme};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    Thinking,
    Length,
    Image,
    Api,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Add,
    Edit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LengthField {
    Context,
    Output,
}

pub const CONTEXT_PICKS: [u64; 5] = [32_000, 128_000, 200_000, 256_000, 1_000_000];
pub const OUTPUT_PICKS: [u64; 5] = [8_000, 16_000, 32_000, 64_000, 128_000];

impl LengthField {
    pub fn picks(self) -> &'static [u64] {
        match self {
            Self::Context => &CONTEXT_PICKS,
            Self::Output => &OUTPUT_PICKS,
        }
    }
}

/// What the editor needs to know about its surroundings. It is passed with
/// every call because it can change while the editor is open.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Context {
    pub profile: ConnectionInfo,
    #[serde(default)]
    pub providers: Vec<Value>,
    /// Every connection, for “你的模型” fill sources. May be empty.
    #[serde(default)]
    pub profiles: Vec<ConnectionInfo>,
    /// Ids the provider listed in the last discovery, for id suggestions.
    #[serde(default)]
    pub reported: Vec<String>,
}

impl Context {
    /// Every connection, with the edited one in its current form.
    pub fn connections(&self) -> Vec<ConnectionInfo> {
        let mut all: Vec<ConnectionInfo> = self
            .profiles
            .iter()
            .filter(|p| p.profile_id != self.profile.profile_id)
            .cloned()
            .collect();
        all.insert(0, self.profile.clone());
        all
    }
    fn provider(&self) -> Option<&'static str> {
        catalog_provider(&self.profile.provider)
    }
    fn custom(&self) -> bool {
        self.profile.kind() == ConnectionKind::Custom
    }
    /// The protocol models of this connection use unless changed.
    pub fn default_api(&self) -> String {
        self.providers
            .iter()
            .find(|p| p["id"] == self.profile.provider.as_str())
            .and_then(|p| p["billing"].as_array())
            .and_then(|items| {
                items
                    .iter()
                    .find(|b| b["id"].as_str() == self.profile.billing.as_deref())
                    .or(items.first())
            })
            .and_then(|b| b["template"]["models"][0]["api"].as_str())
            .filter(|api| MODEL_APIS.iter().any(|a| a.0 == *api))
            .map(str::to_owned)
            .unwrap_or_else(|| match self.profile.provider.as_str() {
                "anthropic" => "anthropic-messages".into(),
                _ => MODEL_APIS[0].0.into(),
            })
    }
    fn model(&self, id: &str) -> Option<&Value> {
        self.profile.models.iter().find(|m| m["id"] == id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RecognitionState {
    pub key: String,
    pub exact: bool,
}

/// Where the current values came from.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ActiveSource {
    pub reference: SourceRef,
    /// Preset name or model id.
    pub name: String,
    pub values: Values,
    /// Applied because the id was recognized (not picked by the user), so a
    /// different id may replace it while it is unmodified.
    #[serde(default)]
    pub automatic: bool,
}

impl ActiveSource {
    fn preset(entry: &model_catalog::Entry) -> Self {
        Self {
            reference: SourceRef::Preset {
                preset: entry.key.clone(),
            },
            name: entry.name.clone(),
            values: entry.values(),
            automatic: true,
        }
    }
    fn preset_key(&self) -> Option<&str> {
        match &self.reference {
            SourceRef::Preset { preset } => Some(preset),
            SourceRef::Model { .. } => None,
        }
    }
    /// `预设` or the source model's id, as the provenance prefix.
    fn prefix(&self) -> &str {
        match self.reference {
            SourceRef::Preset { .. } => "预设",
            SourceRef::Model { .. } => &self.name,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct EditorState {
    /// The connection (`profile_id`) this editor belongs to.
    pub connection: String,
    pub mode: Mode,
    /// Edit: the stored model as opened.
    pub original: Option<Value>,
    pub id: String,
    /// The id is final (picked, Enter or focus left); only then does an
    /// unrecognized id conclude “no preset”.
    pub settled: bool,
    /// The id text the recognition below was computed for.
    pub recognized_id: Option<String>,
    pub recognition: Option<RecognitionState>,
    pub source: Option<ActiveSource>,
    /// `None` until a kind is chosen (unrecognized new model).
    pub scheme: Option<ThinkingScheme>,
    pub context: String,
    pub output: String,
    pub image: bool,
    pub api: String,
    /// Last configuration of each kind this session, for switching back.
    pub cache: Vec<ThinkingScheme>,
    pub open: Vec<Section>,
    /// Numbered form with every section expanded (unrecognized model).
    pub full: bool,
    /// A preset the id now maps to but that was not applied (user edits).
    pub refill: Option<String>,
    pub attempted: bool,
    pub touched: Vec<LengthField>,
    pub level_error: Option<String>,
    pub budget_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    OpenAdd,
    OpenEdit {
        model: String,
    },
    SetId {
        id: String,
    },
    Recognize,
    Settle {
        #[serde(default)]
        id: Option<String>,
    },
    ToggleSection {
        section: Section,
    },
    Restore {
        section: Section,
    },
    Refill,
    UseSource {
        source: SourceRef,
    },
    SetKind {
        kind: ThinkingKind,
    },
    ToggleDefault {
        on: bool,
    },
    LevelDefault {
        name: String,
    },
    LevelDelete {
        index: usize,
    },
    LevelMove {
        from: usize,
        to: usize,
    },
    LevelAdd {
        name: String,
    },
    BudgetDefault {
        choice: BudgetChoice,
    },
    BudgetDelete {
        index: usize,
    },
    BudgetAdd {
        text: String,
    },
    BudgetAllowOff {
        on: bool,
    },
    BudgetDynamic {
        on: bool,
    },
    ClearAddErrors,
    SetLength {
        field: LengthField,
        text: String,
    },
    BlurLength {
        field: LengthField,
    },
    StepLength {
        field: LengthField,
        up: bool,
    },
    PickLength {
        field: LengthField,
        value: u64,
    },
    SetImage {
        on: bool,
    },
    SetApi {
        api: String,
    },
    Save,
    View,
}

/// The field to focus after a failed save.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Focus {
    Id,
    Thinking,
    Context,
    Output,
    Api,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Outcome {
    /// Set when a save passed validation: persist this input.
    pub save: Option<ModelInput>,
    /// Set when a save failed: the first field with an error.
    pub focus: Option<Focus>,
}

const SECTIONS: [Section; 3] = [Section::Thinking, Section::Length, Section::Image];

impl EditorState {
    pub fn open_add(ctx: &Context) -> Self {
        Self {
            connection: ctx.profile.profile_id.clone(),
            mode: Mode::Add,
            api: ctx.default_api(),
            ..Default::default()
        }
    }

    pub fn open_edit(ctx: &Context, id: &str) -> anyhow::Result<Self> {
        let model = ctx
            .model(id)
            .ok_or_else(|| anyhow::anyhow!("模型已被移除"))?
            .clone();
        let mut state = Self {
            connection: ctx.profile.profile_id.clone(),
            mode: Mode::Edit,
            id: id.to_owned(),
            settled: true,
            recognized_id: Some(id.to_owned()),
            api: model["api"]
                .as_str()
                .filter(|api| MODEL_APIS.iter().any(|a| a.0 == *api))
                .map(str::to_owned)
                .unwrap_or_else(|| ctx.default_api()),
            ..Default::default()
        };
        let recognition = model_catalog::recognize_match(id, ctx.provider());
        state.recognition = recognition.map(|r| RecognitionState {
            key: r.entry.key.clone(),
            exact: r.exact,
        });
        state.source = recognition.map(|r| ActiveSource::preset(r.entry));
        let configured = model["limits"].is_object();
        if configured {
            let tokens = |key: &str| {
                model["limits"][key]
                    .as_u64()
                    .map(compact_tokens)
                    .unwrap_or_default()
            };
            state.context = tokens("context_window_tokens");
            state.output = tokens("max_output_tokens");
            state.scheme = Some(model_catalog::stored_scheme(&model));
            state.image = crate::model_edit::model_accepts_images(&model);
        } else {
            // Not configured yet: a numbered form, starting from the preset
            // when there is one, otherwise from nothing the user must check.
            state.full = true;
            match &state.source {
                Some(source) => {
                    let values = source.values.clone();
                    state.set_values(&values);
                }
                None => {
                    let stored = model_catalog::stored_scheme(&model);
                    state.scheme = (stored != ThinkingScheme::Unsupported).then_some(stored);
                    state.image = crate::model_edit::model_accepts_images(&model);
                }
            }
        }
        state.original = Some(model);
        Ok(state)
    }

    fn set_values(&mut self, values: &Values) {
        self.scheme = Some(values.scheme.clone());
        self.context = compact_tokens(values.context);
        self.output = compact_tokens(values.output);
        self.image = values.image;
    }

    fn blank(&self) -> bool {
        self.scheme.is_none()
            && self.context.trim().is_empty()
            && self.output.trim().is_empty()
            && !self.image
    }

    /// Whether a section differs from the source's values.
    pub fn differs(&self, section: Section) -> bool {
        let Some(source) = &self.source else {
            return false;
        };
        let values = &source.values;
        match section {
            Section::Thinking => self.scheme.as_ref() != Some(&values.scheme),
            Section::Length => {
                parse_tokens(&self.context) != Some(values.context)
                    || parse_tokens(&self.output) != Some(values.output)
            }
            Section::Image => self.image != values.image,
            Section::Api => false,
        }
    }

    pub fn modified(&self) -> Vec<Section> {
        SECTIONS.into_iter().filter(|s| self.differs(*s)).collect()
    }

    fn apply_source(&mut self, source: ActiveSource) {
        let values = source.values.clone();
        self.set_values(&values);
        self.source = Some(source);
        self.refill = None;
        self.level_error = None;
        self.budget_error = None;
    }

    fn clear_values(&mut self) {
        self.source = None;
        self.scheme = None;
        self.context.clear();
        self.output.clear();
        self.image = false;
    }

    fn duplicate(&self, ctx: &Context) -> Option<String> {
        if self.mode != Mode::Add {
            return None;
        }
        id_error(&self.id, &ctx.profile.models, None).and_then(|e| e.duplicate)
    }

    fn recognize_now(&mut self, ctx: &Context) {
        if self.mode != Mode::Add {
            return;
        }
        let id = self.id.trim().to_owned();
        self.recognized_id = Some(id.clone());
        let found = (!id.is_empty())
            .then(|| model_catalog::recognize_match(&id, ctx.provider()))
            .flatten();
        let previous = self.recognition.as_ref().map(|r| r.key.clone());
        self.recognition = found.map(|r| RecognitionState {
            key: r.entry.key.clone(),
            exact: r.exact,
        });
        let untouched_preset = self
            .source
            .as_ref()
            .is_some_and(|s| s.automatic && s.preset_key().is_some())
            && self.modified().is_empty();
        match found {
            Some(found) => {
                let key = found.entry.key.as_str();
                let current = self.source.as_ref().and_then(ActiveSource::preset_key) == Some(key);
                if current {
                    self.refill = None;
                } else if previous.as_deref() != Some(key) {
                    if untouched_preset || (self.source.is_none() && self.blank()) {
                        self.apply_source(ActiveSource::preset(found.entry));
                        self.full = false;
                        self.open.clear();
                    } else {
                        self.refill = Some(key.to_owned());
                    }
                }
            }
            None => {
                self.refill = None;
                if untouched_preset {
                    self.clear_values();
                }
                if self.source.is_none() {
                    self.full = true;
                }
            }
        }
    }

    /// Applies one action. Errors are for requests that cannot apply (a
    /// model that vanished, an unknown source); user mistakes become view
    /// errors instead.
    pub fn apply(&mut self, ctx: &Context, action: Action) -> anyhow::Result<Outcome> {
        if !matches!(action, Action::OpenAdd | Action::OpenEdit { .. }) {
            anyhow::ensure!(
                self.connection == ctx.profile.profile_id,
                "编辑器已过期，请重新打开"
            );
        }
        if !matches!(
            action,
            Action::LevelAdd { .. } | Action::BudgetAdd { .. } | Action::View
        ) {
            self.level_error = None;
            self.budget_error = None;
        }
        let mut outcome = Outcome::default();
        match action {
            Action::OpenAdd => *self = Self::open_add(ctx),
            Action::OpenEdit { model } => *self = Self::open_edit(ctx, &model)?,
            Action::SetId { id } => {
                if self.mode == Mode::Add && id != self.id {
                    self.id = id;
                    self.settled = false;
                }
            }
            Action::Recognize => self.recognize_now(ctx),
            Action::Settle { id } => {
                if self.mode == Mode::Add {
                    if let Some(id) = id {
                        self.id = id;
                    }
                    self.settled = true;
                    self.recognize_now(ctx);
                }
            }
            Action::ToggleSection { section } => {
                if !self.full {
                    if let Some(at) = self.open.iter().position(|s| *s == section) {
                        self.open.remove(at);
                    } else {
                        self.open.push(section);
                    }
                }
            }
            Action::Restore { section } => {
                if let Some(values) = self.source.as_ref().map(|s| s.values.clone()) {
                    match section {
                        Section::Thinking => self.scheme = Some(values.scheme),
                        Section::Length => {
                            self.context = compact_tokens(values.context);
                            self.output = compact_tokens(values.output);
                        }
                        Section::Image => self.image = values.image,
                        Section::Api => {}
                    }
                }
            }
            Action::Refill => {
                if let Some(entry) = self.refill.take().and_then(|k| model_catalog::preset(&k)) {
                    self.apply_source(ActiveSource::preset(entry));
                    self.full = false;
                }
            }
            Action::UseSource { source } => {
                let (name, values) = model_catalog::resolve_source(&source, &ctx.connections())
                    .ok_or_else(|| anyhow::anyhow!("这个来源已不可用"))?;
                self.apply_source(ActiveSource {
                    reference: source,
                    name,
                    values,
                    automatic: false,
                });
                self.full = false;
                self.open = SECTIONS.to_vec();
            }
            Action::SetKind { kind } => self.set_kind(kind),
            Action::ToggleDefault { on } => self.edit_scheme(|s| s.toggle_default_on(on)),
            Action::LevelDefault { name } => self.edit_scheme(|s| {
                s.set_level_default(&name);
            }),
            Action::LevelDelete { index } => self.edit_scheme(|s| {
                s.delete_level(index);
            }),
            Action::LevelMove { from, to } => self.edit_scheme(|s| {
                s.move_level(from, to);
            }),
            Action::LevelAdd { name } => {
                if let Some(scheme) = &mut self.scheme {
                    self.level_error = scheme.add_level(&name).err();
                }
            }
            Action::BudgetDefault { choice } => self.edit_scheme(|s| {
                s.set_budget_default(choice);
            }),
            Action::BudgetDelete { index } => self.edit_scheme(|s| s.delete_budget(index)),
            Action::BudgetAdd { text } => {
                let output = self.output_tokens();
                if let Some(scheme) = &mut self.scheme {
                    self.budget_error = scheme.add_budget(&text, output).err();
                }
            }
            Action::BudgetAllowOff { on } => self.edit_scheme(|s| s.set_budget_allow_off(on)),
            Action::BudgetDynamic { on } => self.edit_scheme(|s| s.set_budget_dynamic(on)),
            Action::ClearAddErrors => {}
            Action::SetLength { field, text } => *self.length_mut(field) = text,
            Action::BlurLength { field } => {
                if !self.touched.contains(&field) {
                    self.touched.push(field);
                }
                let text = normalize_tokens(self.length(field));
                *self.length_mut(field) = text;
            }
            Action::StepLength { field, up } => {
                let picks = self.enabled_picks(field);
                let current = parse_tokens(self.length(field)).unwrap_or(0);
                let next = if up {
                    picks
                        .iter()
                        .copied()
                        .find(|p| *p > current)
                        .or(picks.last().copied())
                } else {
                    picks
                        .iter()
                        .rev()
                        .copied()
                        .find(|p| *p < current)
                        .or(picks.first().copied())
                };
                if let Some(next) = next {
                    *self.length_mut(field) = compact_tokens(next);
                }
            }
            Action::PickLength { field, value } => {
                if self.enabled_picks(field).contains(&value) {
                    *self.length_mut(field) = compact_tokens(value);
                }
            }
            Action::SetImage { on } => self.image = on,
            Action::SetApi { api } => {
                if ctx.custom() && MODEL_APIS.iter().any(|a| a.0 == api) {
                    self.api = api;
                }
            }
            Action::Save => outcome = self.save(ctx),
            Action::View => {}
        }
        Ok(outcome)
    }

    fn edit_scheme(&mut self, change: impl FnOnce(&mut ThinkingScheme)) {
        if let Some(scheme) = &mut self.scheme {
            change(scheme);
        }
    }

    fn set_kind(&mut self, kind: ThinkingKind) {
        if self.scheme.as_ref().map(ThinkingScheme::kind) == Some(kind) {
            return;
        }
        if let Some(current) = self.scheme.take() {
            self.cache.retain(|s| s.kind() != current.kind());
            self.cache.push(current);
        }
        let from_source = self
            .source
            .as_ref()
            .map(|s| &s.values.scheme)
            .filter(|s| s.kind() == kind);
        self.scheme = Some(
            self.cache
                .iter()
                .find(|s| s.kind() == kind)
                .or(from_source)
                .cloned()
                .unwrap_or_else(|| ThinkingScheme::default_for(kind)),
        );
    }

    fn length(&self, field: LengthField) -> &str {
        match field {
            LengthField::Context => &self.context,
            LengthField::Output => &self.output,
        }
    }

    fn length_mut(&mut self, field: LengthField) -> &mut String {
        match field {
            LengthField::Context => &mut self.context,
            LengthField::Output => &mut self.output,
        }
    }

    fn output_tokens(&self) -> Option<u32> {
        parse_tokens(&self.output).and_then(|o| u32::try_from(o).ok())
    }

    /// Quick picks that can be chosen: output picks must stay below context.
    pub fn enabled_picks(&self, field: LengthField) -> Vec<u64> {
        let context = parse_tokens(&self.context);
        field
            .picks()
            .iter()
            .copied()
            .filter(|p| field == LengthField::Context || context.is_none_or(|c| *p < c))
            .collect()
    }

    /// The input this form would save.
    pub fn input(&self) -> ModelInput {
        let scheme = self.scheme.clone().unwrap_or(ThinkingScheme::Unsupported);
        let (thinking, default_thinking) = scheme.encode();
        ModelInput {
            previous: self.original.clone(),
            copied: None,
            id: match (&self.mode, &self.original) {
                (Mode::Edit, Some(original)) => {
                    original["id"].as_str().unwrap_or_default().to_owned()
                }
                _ => self.id.trim().to_owned(),
            },
            api: self.api.clone(),
            context: self.context.clone(),
            output: self.output.clone(),
            thinking: thinking.join(", "),
            default_thinking,
            images: self.image,
            thinking_scheme: Some(scheme),
        }
    }

    /// Every validation error, regardless of when the view shows it.
    pub fn errors(&self, ctx: &Context) -> Vec<crate::model_edit::FieldError> {
        let mut errors = self.input().errors(&ctx.profile.models);
        if self.scheme.is_none() {
            errors.retain(|e| e.field != FIELD_THINKING);
            let at = errors
                .iter()
                .position(|e| e.field != FIELD_ID)
                .unwrap_or(errors.len());
            errors.insert(
                at,
                crate::model_edit::FieldError {
                    field: FIELD_THINKING.into(),
                    message: "选一种思考方式".into(),
                    duplicate: None,
                    bad_budgets: vec![],
                },
            );
        }
        errors
    }

    fn save(&mut self, ctx: &Context) -> Outcome {
        self.attempted = true;
        let errors = self.errors(ctx);
        let Some(first) = errors.first() else {
            return Outcome {
                save: Some(self.input()),
                focus: None,
            };
        };
        for error in &errors {
            if let Some(section) = section_of(&error.field) {
                if !self.open.contains(&section) {
                    self.open.push(section);
                }
            }
        }
        Outcome {
            save: None,
            focus: Some(match first.field.as_str() {
                FIELD_ID => Focus::Id,
                FIELD_THINKING => Focus::Thinking,
                FIELD_CONTEXT => Focus::Context,
                FIELD_OUTPUT => Focus::Output,
                _ => Focus::Api,
            }),
        }
    }
}

fn section_of(field: &str) -> Option<Section> {
    match field {
        FIELD_THINKING => Some(Section::Thinking),
        FIELD_CONTEXT | FIELD_OUTPUT => Some(Section::Length),
        FIELD_API => Some(Section::Api),
        _ => None,
    }
}

// ---------------------------------------------------------------- view

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct View {
    pub mode: Mode,
    /// `添加模型`, or the model id when editing.
    pub title: String,
    pub id: IdView,
    /// Recognition / source line under the id.
    pub status: Option<StatusView>,
    /// Numbered form with every section expanded.
    pub full: bool,
    /// Empty while the id is still being typed or duplicates another model.
    pub sections: Vec<SectionView>,
    pub thinking: ThinkingView,
    pub length: LengthView,
    pub image: ImageView,
    /// Custom-base-URL connections only.
    pub api: Option<ApiView>,
    pub footer: FooterView,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct IdView {
    pub text: String,
    pub editable: bool,
    pub placeholder: &'static str,
    pub error: Option<String>,
    /// For a duplicate id: the model “去编辑它” opens.
    pub duplicate: Option<String>,
    /// Add mode: what the suggestion popover offers for the current text.
    pub suggestions: Option<Suggestions>,
    /// A recognition for newer text is still due (keep showing the old one).
    pub pending: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StatusView {
    /// `recognized`, `source` (filled from something else, id unknown) or
    /// `no_preset` (warning).
    pub kind: &'static str,
    pub title: String,
    pub detail: String,
    /// `换一个来源` / `从相似模型填入`: opens the fill source popover.
    pub action: &'static str,
    pub refill: Option<RefillView>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RefillView {
    pub text: String,
    pub action: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SectionView {
    pub section: Section,
    pub label: &'static str,
    /// Question heading in the numbered form, e.g. `它会思考吗？`.
    pub question: &'static str,
    pub number: usize,
    pub summary: String,
    pub muted: bool,
    pub open: bool,
    pub error: bool,
    pub provenance: Option<ProvenanceView>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProvenanceView {
    /// `预设：可开关 · 默认关` or `{source}：…`; ellipsize, never wrap.
    pub text: String,
    /// Full source value for a tooltip.
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct KindView {
    pub kind: ThinkingKind,
    pub title: &'static str,
    pub example: &'static str,
    pub selected: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ThinkingView {
    pub kinds: Vec<KindView>,
    pub selected: Option<ThinkingKind>,
    pub toggle: Option<ToggleView>,
    pub levels: Option<LevelsView>,
    pub budget: Option<BudgetView>,
    /// Explanation for kinds without a sub-editor.
    pub note: Option<&'static str>,
    /// `模型面板里会显示` preview; absent until a kind is chosen.
    pub panel: Option<PanelView>,
    /// ⓘ text: `请求里写入 reasoning.effort` / `由系统按接口协议写入请求`.
    pub request_field: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToggleView {
    pub default_on: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LevelChip {
    pub name: String,
    pub default: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LevelsView {
    pub values: Vec<LevelChip>,
    pub can_delete: bool,
    /// Common names for the `+ 档位` popover.
    pub addable: Vec<&'static str>,
    pub add_error: Option<String>,
    pub help: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BudgetChip {
    pub tokens: u32,
    pub label: String,
    pub default: bool,
    /// Not below the output limit.
    pub bad: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChoiceView {
    pub choice: BudgetChoice,
    pub label: String,
    pub selected: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BudgetView {
    pub presets: Vec<BudgetChip>,
    pub allow_off: bool,
    pub dynamic: bool,
    pub dynamic_help: &'static str,
    pub defaults: Vec<ChoiceView>,
    pub add_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PanelView {
    pub panel: Panel,
    /// Text for `hidden` / `fixed` panels.
    pub text: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PickView {
    pub value: u64,
    pub label: String,
    pub selected: bool,
    pub disabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TokenFieldView {
    pub label: &'static str,
    pub text: String,
    /// `128,000` once the text parses.
    pub exact: Option<String>,
    pub placeholder: &'static str,
    pub picks: Vec<PickView>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LengthView {
    pub context: TokenFieldView,
    pub output: TokenFieldView,
    pub help: &'static str,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ImageView {
    pub on: bool,
    pub label: &'static str,
    pub help: &'static str,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ApiOption {
    pub api: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// The connection's default protocol (`· 连接默认`).
    pub default: bool,
    pub selected: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ApiView {
    pub options: Vec<ApiOption>,
    pub warning: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FooterView {
    pub remove: bool,
    pub save: &'static str,
}

pub fn api_description(api: &str) -> &'static str {
    match api {
        "openai-completions" => "绝大多数兼容 OpenAI 的服务",
        "openai-responses" => "OpenAI 官方接口",
        "openai-codex-responses" => "ChatGPT 订阅登录的连接",
        "anthropic-messages" => "Claude 及兼容 Anthropic 的服务",
        _ => "",
    }
}

fn api_title(api: &str) -> &'static str {
    MODEL_APIS
        .iter()
        .find(|a| a.0 == api)
        .map(|a| a.1)
        .unwrap_or("")
}

impl EditorState {
    fn thinking_field(&self) -> Option<&'static str> {
        let preset = self
            .source
            .as_ref()
            .and_then(ActiveSource::preset_key)
            .or(self.recognition.as_ref().map(|r| r.key.as_str()))
            .and_then(model_catalog::preset);
        preset.and_then(|e| e.thinking_field.as_deref())
    }

    pub fn view(&self, ctx: &Context) -> View {
        let errors = self.errors(ctx);
        let error = |field: &str| errors.iter().find(|e| e.field == field);
        let add = self.mode == Mode::Add;
        let id_text = self.id.trim();

        // ---- id
        let id_problem = error(FIELD_ID);
        let duplicate = self.duplicate(ctx);
        let id_error = id_problem.and_then(|e| {
            let visible =
                e.duplicate.is_some() || self.attempted || (self.settled && !id_text.is_empty());
            (add && visible).then(|| e.message.clone())
        });
        let id = IdView {
            text: self.id.clone(),
            editable: add,
            placeholder: "模型 ID，例如 deepseek-chat",
            error: id_error,
            duplicate: duplicate.clone(),
            suggestions: add
                .then(|| model_catalog::id_suggestions(&self.id, &ctx.profile, &ctx.reported)),
            pending: add && self.recognized_id.as_deref() != Some(id_text),
        };

        // ---- which parts show
        let form_visible = !add
            || (!id_text.is_empty()
                && duplicate.is_none()
                && (self.recognition.is_some() || self.settled));

        // ---- status line
        let status = form_visible.then(|| self.status()).flatten();

        // ---- thinking
        let bad_budgets = error(FIELD_THINKING)
            .map(|e| e.bad_budgets.clone())
            .unwrap_or_default();
        let thinking_error = error(FIELD_THINKING)
            .filter(|_| self.attempted || self.scheme.is_some())
            .map(|e| e.message.clone());
        let thinking = self.thinking_view(thinking_error.clone(), &bad_budgets);

        // ---- length
        let length_error = |field: LengthField, key: &str| {
            let problem = error(key)?;
            let text = self.length(field);
            let unparsable = !text.trim().is_empty() && parse_tokens(text).is_none();
            let visible = self.attempted
                || (self.touched.contains(&field) && unparsable)
                || (field == LengthField::Output
                    && !unparsable
                    && !text.trim().is_empty()
                    && parse_tokens(&self.context).is_some());
            visible.then(|| problem.message.clone())
        };
        let token_field = |field: LengthField, key: &str| {
            let text = self.length(field);
            let parsed = parse_tokens(text);
            let enabled = self.enabled_picks(field);
            TokenFieldView {
                label: match field {
                    LengthField::Context => "上下文",
                    LengthField::Output => "最长输出",
                },
                text: text.to_owned(),
                exact: parsed.map(exact_tokens),
                placeholder: match field {
                    LengthField::Context => "例如 128K",
                    LengthField::Output => "例如 8K",
                },
                picks: field
                    .picks()
                    .iter()
                    .map(|value| PickView {
                        value: *value,
                        label: compact_tokens(*value),
                        selected: parsed == Some(*value),
                        disabled: !enabled.contains(value),
                    })
                    .collect(),
                error: length_error(field, key),
            }
        };
        let length = LengthView {
            context: token_field(LengthField::Context, FIELD_CONTEXT),
            output: token_field(LengthField::Output, FIELD_OUTPUT),
            help: "可以写 128K、1M、131072；K = 1000。最长输出需小于上下文。",
        };

        // ---- api
        let default_api = ctx.default_api();
        let api = ctx.custom().then(|| ApiView {
            options: MODEL_APIS
                .iter()
                .map(|(api, title)| ApiOption {
                    api,
                    title,
                    description: api_description(api),
                    default: *api == default_api,
                    selected: *api == self.api,
                })
                .collect(),
            warning: (self.api != default_api).then(|| {
                format!(
                    "这个连接默认用 {}，确认供应商支持再改",
                    api_title(&default_api)
                )
            }),
        });

        // ---- sections
        let mut sections = Vec::new();
        if form_visible {
            let parsed_context = parse_tokens(&self.context);
            let parsed_output = parse_tokens(&self.output);
            let mut list = vec![
                (
                    Section::Thinking,
                    "思考",
                    "它会思考吗？",
                    self.scheme
                        .as_ref()
                        .map(ThinkingScheme::describe)
                        .unwrap_or_else(|| "还没选".into()),
                    self.scheme.is_none(),
                    thinking.error.is_some(),
                ),
                (
                    Section::Length,
                    "长度",
                    "上下文和最长输出",
                    if self.context.trim().is_empty() && self.output.trim().is_empty() {
                        "还没填".into()
                    } else {
                        let show = |parsed: Option<u64>, text: &str| match parsed {
                            Some(n) => compact_tokens(n),
                            None if text.trim().is_empty() => "—".into(),
                            None => text.trim().to_owned(),
                        };
                        format!(
                            "上下文 {} · 最长输出 {}",
                            show(parsed_context, &self.context),
                            show(parsed_output, &self.output)
                        )
                    },
                    parsed_context.is_none() || parsed_output.is_none(),
                    length.context.error.is_some() || length.output.error.is_some(),
                ),
                (
                    Section::Image,
                    "图片",
                    "能看图片吗？",
                    model_catalog::image_label(self.image).into(),
                    !self.image,
                    false,
                ),
            ];
            if api.is_some() {
                list.push((
                    Section::Api,
                    "协议",
                    "接口协议",
                    format!(
                        "{}{}",
                        api_title(&self.api),
                        if self.api == default_api {
                            " · 跟随连接"
                        } else {
                            ""
                        }
                    ),
                    false,
                    error(FIELD_API).is_some() && self.attempted,
                ));
            }
            sections = list
                .into_iter()
                .enumerate()
                .map(
                    |(index, (section, label, question, summary, muted, has_error))| SectionView {
                        section,
                        label,
                        question,
                        number: index + 1,
                        summary,
                        muted,
                        open: self.full || self.open.contains(&section),
                        error: has_error,
                        provenance: self.provenance(section),
                    },
                )
                .collect();
        }

        View {
            mode: self.mode,
            title: if add {
                "添加模型".into()
            } else {
                self.id.clone()
            },
            id,
            status,
            full: self.full && form_visible,
            sections,
            thinking,
            length,
            image: ImageView {
                on: self.image,
                label: model_catalog::image_label(self.image),
                help: "打开后，这个模型可以接收对话里的图片；关闭时给它发图会在发送前提醒",
            },
            api,
            footer: FooterView {
                remove: !add,
                save: if add { "保存模型" } else { "保存" },
            },
        }
    }

    /// Per-section provenance: `预设：{short}` when the section differs from
    /// its source.
    pub fn provenance(&self, section: Section) -> Option<ProvenanceView> {
        let source = self.source.as_ref()?;
        if !self.differs(section) {
            return None;
        }
        let values = &source.values;
        let (short, detail) = match section {
            Section::Thinking => (values.scheme.short(), values.scheme.describe()),
            Section::Length => (values.length_short(), values.length_short()),
            Section::Image => (values.image_short().into(), values.image_short().into()),
            Section::Api => return None,
        };
        Some(ProvenanceView {
            text: format!("{}：{short}", source.prefix()),
            detail: format!("{}：{detail}", source.prefix()),
        })
    }

    fn status(&self) -> Option<StatusView> {
        let add = self.mode == Mode::Add;
        let recognized = self
            .recognition
            .as_ref()
            .and_then(|r| model_catalog::preset(&r.key).map(|e| (r, e)));
        let refill = self
            .refill
            .as_deref()
            .and_then(model_catalog::preset)
            .map(|e| RefillView {
                text: format!("ID 对应 {}，你改过参数所以没有覆盖", e.name),
                action: format!("按 {} 预设重新填入", e.name),
            });
        if let Some((recognition, entry)) = recognized {
            let source_key = self.source.as_ref().and_then(ActiveSource::preset_key);
            let detail = match &self.source {
                Some(source) if source_key != Some(entry.key.as_str()) => match source.reference {
                    SourceRef::Preset { .. } => format!("参数来自 {} 预设", source.name),
                    SourceRef::Model { .. } => format!("参数来自 {}", source.name),
                },
                None => String::new(),
                Some(_) if !add => match self.modified().len() {
                    0 => "与预设一致".into(),
                    n => format!("{n} 项与预设不同"),
                },
                Some(_) if !recognition.exact => {
                    format!("按 {} 预设（{} 是它的变体）", entry.name, self.id.trim())
                }
                Some(_) => "参数已按预设填好".into(),
            };
            return Some(StatusView {
                kind: "recognized",
                title: entry.name.clone(),
                detail,
                action: "换一个来源",
                refill,
            });
        }
        if let Some(source) = &self.source {
            return Some(StatusView {
                kind: "source",
                title: format!("参数来自 {}", source.name),
                detail: "ID 没有对应的预设".into(),
                action: "换一个来源",
                refill: None,
            });
        }
        if add && !self.settled {
            return None;
        }
        Some(if self.full {
            StatusView {
                kind: "no_preset",
                title: "没有这个模型的预设".into(),
                detail: "按下面三项填写；如果它是某个常见模型的变体，可以".into(),
                action: "从相似模型填入",
                refill: None,
            }
        } else {
            StatusView {
                kind: "no_preset",
                title: String::new(),
                detail: "这个模型没有预设".into(),
                action: "从相似模型填入",
                refill: None,
            }
        })
    }

    fn thinking_view(&self, error: Option<String>, bad_budgets: &[u32]) -> ThinkingView {
        let selected = self.scheme.as_ref().map(ThinkingScheme::kind);
        let scheme = self.scheme.as_ref();
        ThinkingView {
            kinds: ThinkingKind::ALL
                .into_iter()
                .map(|kind| KindView {
                    kind,
                    title: kind.title(),
                    example: kind.example(),
                    selected: selected == Some(kind),
                })
                .collect(),
            selected,
            toggle: match scheme {
                Some(ThinkingScheme::Toggle { default_on, .. }) => Some(ToggleView {
                    default_on: *default_on,
                }),
                _ => None,
            },
            levels: match scheme {
                Some(s @ ThinkingScheme::Levels { values, default }) => Some(LevelsView {
                    values: values
                        .iter()
                        .map(|v| LevelChip {
                            name: v.clone(),
                            default: v == default,
                        })
                        .collect(),
                    can_delete: values.len() > 1,
                    addable: s.addable_levels(),
                    add_error: self.level_error.clone(),
                    help: format!(
                        "用供应商自己的名字，从弱到强排列（拖动调整）；点一下设为默认，现在是 {default}"
                    ),
                }),
                _ => None,
            },
            budget: match scheme {
                Some(
                    s @ ThinkingScheme::Budget {
                        presets,
                        default,
                        dynamic,
                        allow_off,
                    },
                ) => Some(BudgetView {
                    presets: presets
                        .iter()
                        .map(|t| BudgetChip {
                            tokens: *t,
                            label: compact_tokens(u64::from(*t)),
                            default: *default == BudgetChoice::Tokens(*t),
                            bad: bad_budgets.contains(t),
                        })
                        .collect(),
                    allow_off: *allow_off,
                    dynamic: *dynamic,
                    dynamic_help: "Gemini 的动态预算",
                    defaults: s
                        .budget_options()
                        .into_iter()
                        .map(|choice| ChoiceView {
                            label: choice.label(),
                            selected: choice == *default,
                            choice,
                        })
                        .collect(),
                    add_error: self.budget_error.clone(),
                }),
                _ => None,
            },
            note: match selected {
                Some(ThinkingKind::Always) => Some("每次请求都会思考，模型面板里不提供开关"),
                Some(ThinkingKind::Unsupported) => Some("请求里不带思考参数"),
                _ => None,
            },
            panel: scheme.map(|s| {
                let panel = s.panel();
                PanelView {
                    text: match panel {
                        Panel::Hidden => Some("不显示思考选项"),
                        Panel::Fixed => Some("思考中（不可调）"),
                        Panel::Options { .. } => None,
                    },
                    panel,
                }
            }),
            request_field: match self.thinking_field() {
                Some(field) => format!("请求里写入 {field}"),
                None => "由系统按接口协议写入请求".into(),
            },
            error,
        }
    }

    /// Fill source groups for the “从相似模型填入 / 换一个来源” popover.
    pub fn sources(&self, ctx: &Context, query: &str) -> Vec<FillSourceGroup> {
        let editing = match (&self.mode, &self.original) {
            (Mode::Edit, Some(model)) => model["id"].as_str(),
            _ => None,
        };
        model_catalog::fill_sources(
            query,
            &ctx.connections(),
            editing.map(|id| (self.connection.as_str(), id)),
        )
    }
}

#[derive(Deserialize)]
struct Request {
    context: Context,
    #[serde(default)]
    state: Option<EditorState>,
    action: Action,
}

/// Runs one editor call (see the module docs for the JSON shapes).
pub fn handle(request: Value) -> anyhow::Result<Value> {
    let Request {
        context,
        state,
        action,
    } = serde_json::from_value(request)?;
    let mut state = match (state, &action) {
        (_, Action::OpenAdd) => EditorState::open_add(&context),
        (_, Action::OpenEdit { model }) => EditorState::open_edit(&context, model)?,
        (Some(state), _) => state,
        (None, _) => anyhow::bail!("编辑器还没有打开"),
    };
    let outcome = match action {
        Action::OpenAdd | Action::OpenEdit { .. } => Outcome::default(),
        action => state.apply(&context, action)?,
    };
    let view = state.view(&context);
    Ok(json!({
        "state": state,
        "view": view,
        "effect": outcome.save.map(|input| json!({"save": input})),
        "focus": outcome.focus,
    }))
}

/// `{context, state, query}` → fill source groups.
pub fn handle_sources(request: Value) -> anyhow::Result<Value> {
    #[derive(Deserialize)]
    struct Sources {
        context: Context,
        #[serde(default)]
        state: Option<EditorState>,
        #[serde(default)]
        query: String,
    }
    let Sources {
        context,
        state,
        query,
    } = serde_json::from_value(request)?;
    let state = state.unwrap_or_else(|| EditorState::open_add(&context));
    Ok(serde_json::to_value(state.sources(&context, &query))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_catalog::SourceRef;

    fn providers() -> Vec<Value> {
        let template = |id: &str, api: &str| json!({"id": id, "billing": [{"id": "usage", "template": {"models": [{"api": api}]}}]});
        vec![
            template("deepseek", "openai-completions"),
            template("openai", "openai-responses"),
            template("openai-compatible", "openai-completions"),
            template("openrouter", "openai-completions"),
        ]
    }

    fn configured(id: &str, context: u64, output: u64, thinking: &[&str], default: &str) -> Value {
        json!({"id": id, "api": "openai-completions", "enabled": true,
            "limits": {"context_window_tokens": context, "max_output_tokens": output},
            "thinking": thinking, "default_thinking": default,
            "capabilities": {"input": ["text"]}})
    }

    fn ctx(provider: &str, models: Vec<Value>) -> Context {
        let profile = ConnectionInfo {
            profile_id: format!("my-{provider}"),
            name: Some(format!("My {provider}")),
            provider: provider.into(),
            billing: Some("usage".into()),
            models,
        };
        let other = ConnectionInfo {
            profile_id: "lab".into(),
            name: Some("Lab".into()),
            provider: "openai-compatible".into(),
            billing: Some("usage".into()),
            models: vec![configured(
                "qwen3-32b",
                128_000,
                16_000,
                &["off", "high"],
                "high",
            )],
        };
        Context {
            profile: profile.clone(),
            providers: providers(),
            profiles: vec![profile, other],
            reported: vec![],
        }
    }

    fn deepseek() -> Context {
        ctx(
            "deepseek",
            vec![configured(
                "deepseek-chat",
                128_000,
                8_000,
                &["off", "high"],
                "off",
            )],
        )
    }

    fn run(state: &mut EditorState, ctx: &Context, action: Action) -> Outcome {
        state.apply(ctx, action).unwrap()
    }

    fn type_id(state: &mut EditorState, ctx: &Context, id: &str) {
        run(state, ctx, Action::SetId { id: id.into() });
        run(state, ctx, Action::Recognize);
    }

    fn section(view: &View, section: Section) -> SectionView {
        view.sections
            .iter()
            .find(|s| s.section == section)
            .cloned()
            .unwrap_or_else(|| panic!("{section:?} missing"))
    }

    fn add(ctx: &Context, id: &str) -> EditorState {
        let mut state = EditorState::open_add(ctx);
        run(
            &mut state,
            ctx,
            Action::Settle {
                id: Some(id.into()),
            },
        );
        state
    }

    #[test]
    fn opening_add_uses_the_connection_protocol() {
        let ctx = deepseek();
        let state = EditorState::open_add(&ctx);
        assert_eq!(state.api, "openai-completions");
        let view = state.view(&ctx);
        assert_eq!(view.title, "添加模型");
        assert!(view.id.editable);
        assert!(view.sections.is_empty() && view.status.is_none());
        assert_eq!(view.footer.save, "保存模型");
        assert!(!view.footer.remove);
        assert!(view.api.is_none(), "protocol follows the connection");
        assert_eq!(
            EditorState::open_add(&ctx_with("openai", vec![])).api,
            "openai-responses"
        );
        assert_eq!(
            EditorState::open_add(&ctx_with("anthropic", vec![])).api,
            "anthropic-messages"
        );
    }

    #[test]
    fn a_recognized_id_fills_every_section_from_its_preset() {
        let ctx = deepseek();
        let mut state = EditorState::open_add(&ctx);
        type_id(&mut state, &ctx, "deepseek-reasoner");
        let view = state.view(&ctx);
        let status = view.status.clone().unwrap();
        assert_eq!(status.kind, "recognized");
        assert_eq!(status.title, "DeepSeek Reasoner");
        assert_eq!(status.detail, "参数已按预设填好");
        assert_eq!(status.action, "换一个来源");
        assert!(!view.full);
        let labels: Vec<&str> = view.sections.iter().map(|s| s.label).collect();
        assert_eq!(labels, ["思考", "长度", "图片"]);
        assert!(view
            .sections
            .iter()
            .all(|s| !s.open && s.provenance.is_none()));
        assert_eq!(section(&view, Section::Thinking).summary, "总是思考");
        assert_eq!(
            section(&view, Section::Length).summary,
            "上下文 128K · 最长输出 64K"
        );
        assert_eq!(section(&view, Section::Image).summary, "不能看图片");
        assert_eq!(view.thinking.request_field, "请求里写入 reasoning_content");
        assert_eq!(view.length.context.exact.as_deref(), Some("128,000"));
        // Only the clicked section expands, and each collapses on its own.
        run(
            &mut state,
            &ctx,
            Action::ToggleSection {
                section: Section::Length,
            },
        );
        run(
            &mut state,
            &ctx,
            Action::ToggleSection {
                section: Section::Image,
            },
        );
        run(
            &mut state,
            &ctx,
            Action::ToggleSection {
                section: Section::Length,
            },
        );
        let view = state.view(&ctx);
        assert!(!section(&view, Section::Length).open);
        assert!(section(&view, Section::Image).open);
        assert!(!section(&view, Section::Thinking).open);
        // It saves as is.
        let outcome = run(&mut state, &ctx, Action::Save);
        let input = outcome.save.unwrap();
        assert_eq!(input.id, "deepseek-reasoner");
        assert_eq!(input.api, "openai-completions");
        let models = input.apply(ctx.profile.models.clone()).unwrap();
        let saved = &models[1];
        assert_eq!(saved["limits"]["context_window_tokens"], 128000);
        assert_eq!(saved["limits"]["max_output_tokens"], 64000);
        assert_eq!(saved["thinking"], json!(["high"]));
        assert!(saved.get("enabled").is_none(), "new models are enabled");
        assert!(model_catalog::model_row(saved, "deepseek").preset);
    }

    #[test]
    fn variants_name_the_preset_they_follow() {
        let ctx = ctx("openai", vec![]);
        let mut state = EditorState::open_add(&ctx);
        type_id(&mut state, &ctx, "gpt-5-2026-08-07");
        let status = state.view(&ctx).status.unwrap();
        assert_eq!(status.title, "GPT-5");
        assert_eq!(
            status.detail,
            "按 GPT-5 预设（gpt-5-2026-08-07 是它的变体）"
        );
    }

    #[test]
    fn unknown_ids_wait_until_settled_before_concluding() {
        let ctx = deepseek();
        let mut state = EditorState::open_add(&ctx);
        run(&mut state, &ctx, Action::SetId { id: "my-mo".into() });
        let view = state.view(&ctx);
        assert!(view.id.pending, "recognition still due");
        assert!(view.id.suggestions.is_some());
        run(&mut state, &ctx, Action::Recognize);
        let view = state.view(&ctx);
        assert!(!view.id.pending);
        assert!(view.status.is_none(), "no “no preset” while typing");
        assert!(view.sections.is_empty(), "no form while typing");
        run(&mut state, &ctx, Action::Settle { id: None });
        let view = state.view(&ctx);
        let status = view.status.clone().unwrap();
        assert_eq!(status.kind, "no_preset");
        assert_eq!(status.title, "没有这个模型的预设");
        assert_eq!(status.action, "从相似模型填入");
        assert!(view.full);
        let questions: Vec<(usize, &str)> = view
            .sections
            .iter()
            .map(|s| (s.number, s.question))
            .collect();
        assert_eq!(
            questions,
            [
                (1, "它会思考吗？"),
                (2, "上下文和最长输出"),
                (3, "能看图片吗？")
            ]
        );
        assert!(view.sections.iter().all(|s| s.open));
        assert_eq!(view.thinking.selected, None, "no kind pre-selected");
        assert!(view.thinking.kinds.iter().all(|k| !k.selected));
        assert_eq!(section(&view, Section::Thinking).summary, "还没选");
        assert!(section(&view, Section::Thinking).muted);
        assert_eq!(section(&view, Section::Length).summary, "还没填");
        // Sections cannot collapse in the numbered form.
        run(
            &mut state,
            &ctx,
            Action::ToggleSection {
                section: Section::Image,
            },
        );
        assert!(section(&state.view(&ctx), Section::Image).open);
        // No errors before a save.
        let view = state.view(&ctx);
        assert!(view.thinking.error.is_none());
        assert!(view.length.context.error.is_none() && view.length.output.error.is_none());
        // Typing again hides the conclusion until the id settles again.
        run(
            &mut state,
            &ctx,
            Action::SetId {
                id: "my-mod".into(),
            },
        );
        run(&mut state, &ctx, Action::Recognize);
        assert!(state.view(&ctx).sections.is_empty());
    }

    #[test]
    fn a_pending_recognition_keeps_the_previous_one_visible() {
        let ctx = deepseek();
        let mut state = EditorState::open_add(&ctx);
        type_id(&mut state, &ctx, "deepseek-reasoner");
        run(
            &mut state,
            &ctx,
            Action::SetId {
                id: "deepseek-reasonerx".into(),
            },
        );
        let view = state.view(&ctx);
        assert!(view.id.pending);
        assert_eq!(view.status.clone().unwrap().title, "DeepSeek Reasoner");
        assert_eq!(view.sections.len(), 3);
    }

    #[test]
    fn changing_the_id_refills_unless_the_user_changed_values() {
        let ctx = ctx("openai", vec![]);
        let mut state = EditorState::open_add(&ctx);
        type_id(&mut state, &ctx, "gpt-5");
        type_id(&mut state, &ctx, "gpt-4o");
        assert_eq!(state.view(&ctx).status.unwrap().title, "GPT-4o");
        assert_eq!(
            state.source.as_ref().unwrap().reference,
            SourceRef::Preset {
                preset: "gpt-4o".into()
            }
        );
        assert!(state.refill.is_none());
        // A change the user made is not overwritten.
        run(&mut state, &ctx, Action::SetImage { on: false });
        type_id(&mut state, &ctx, "gpt-5");
        let view = state.view(&ctx);
        let status = view.status.clone().unwrap();
        assert_eq!(status.title, "GPT-5");
        let refill = status.refill.unwrap();
        assert_eq!(refill.text, "ID 对应 GPT-5，你改过参数所以没有覆盖");
        assert_eq!(refill.action, "按 GPT-5 预设重新填入");
        assert_eq!(status.detail, "参数来自 GPT-4o 预设");
        assert!(!state.image, "user value kept");
        run(&mut state, &ctx, Action::Refill);
        assert!(state.refill.is_none());
        assert!(state.image);
        assert_eq!(state.context, "400K");
        assert_eq!(state.view(&ctx).status.unwrap().detail, "参数已按预设填好");
        // Back to an unknown id: untouched preset values go away …
        type_id(&mut state, &ctx, "my-own");
        run(&mut state, &ctx, Action::Settle { id: None });
        assert!(state.scheme.is_none() && state.context.is_empty());
        assert_eq!(state.view(&ctx).status.unwrap().kind, "no_preset");
        assert!(state.full);
        // … and hand-entered values are never overwritten by a later match.
        run(
            &mut state,
            &ctx,
            Action::SetLength {
                field: LengthField::Context,
                text: "64K".into(),
            },
        );
        type_id(&mut state, &ctx, "gpt-5");
        assert_eq!(state.context, "64K");
        assert_eq!(state.refill.as_deref(), Some("gpt-5"));
        let status = state.view(&ctx).status.unwrap();
        assert_eq!(status.detail, "");
        assert!(status.refill.is_some());
    }

    #[test]
    fn modified_values_leave_the_preset_when_the_id_becomes_unknown() {
        let ctx = deepseek();
        let mut state = add(&ctx, "deepseek-reasoner");
        run(&mut state, &ctx, Action::SetImage { on: true });
        type_id(&mut state, &ctx, "my-reasoner");
        run(&mut state, &ctx, Action::Settle { id: None });
        let view = state.view(&ctx);
        let status = view.status.clone().unwrap();
        assert_eq!(status.kind, "source");
        assert_eq!(status.title, "参数来自 DeepSeek Reasoner");
        assert_eq!(status.detail, "ID 没有对应的预设");
        assert_eq!(status.action, "换一个来源");
        assert!(!view.full);
        assert_eq!(
            section(&view, Section::Image).provenance.unwrap().text,
            "预设：不能看图片"
        );
    }

    #[test]
    fn modified_sections_show_their_source_and_restore() {
        let ctx = deepseek();
        let mut state = add(&ctx, "deepseek-reasoner");
        run(
            &mut state,
            &ctx,
            Action::SetKind {
                kind: ThinkingKind::Toggle,
            },
        );
        run(
            &mut state,
            &ctx,
            Action::SetLength {
                field: LengthField::Output,
                text: "32K".into(),
            },
        );
        let view = state.view(&ctx);
        let thinking = section(&view, Section::Thinking);
        assert_eq!(thinking.summary, "可以开关 · 默认关");
        assert_eq!(thinking.provenance.unwrap().text, "预设：总是思考");
        let length = section(&view, Section::Length).provenance.unwrap();
        assert_eq!(length.text, "预设：上下文 128K · 最长输出 64K");
        assert!(section(&view, Section::Image).provenance.is_none());
        run(
            &mut state,
            &ctx,
            Action::Restore {
                section: Section::Length,
            },
        );
        let view = state.view(&ctx);
        assert!(section(&view, Section::Length).provenance.is_none());
        assert_eq!(state.output, "64K");
        run(
            &mut state,
            &ctx,
            Action::Restore {
                section: Section::Thinking,
            },
        );
        assert_eq!(
            state.scheme,
            Some(ThinkingScheme::Always {
                value: "high".into()
            })
        );
        assert!(state.modified().is_empty());
        // Typing the same value back counts as unmodified.
        run(
            &mut state,
            &ctx,
            Action::SetLength {
                field: LengthField::Output,
                text: "64000".into(),
            },
        );
        assert!(state.modified().is_empty());
    }

    #[test]
    fn duplicates_offer_to_edit_the_existing_model_and_hide_the_rest() {
        let ctx = deepseek();
        let mut state = EditorState::open_add(&ctx);
        type_id(&mut state, &ctx, "deepseek-chat");
        let view = state.view(&ctx);
        assert_eq!(
            view.id.error.as_deref(),
            Some("这个连接里已经有 deepseek-chat")
        );
        assert_eq!(view.id.duplicate.as_deref(), Some("deepseek-chat"));
        assert!(view.status.is_none());
        assert!(view.sections.is_empty());
        let outcome = run(&mut state, &ctx, Action::Save);
        assert!(outcome.save.is_none());
        assert_eq!(outcome.focus, Some(Focus::Id));
        // 去编辑它 opens that model.
        let edit = EditorState::open_edit(&ctx, "deepseek-chat").unwrap();
        assert_eq!(edit.view(&ctx).title, "deepseek-chat");
    }

    #[test]
    fn saving_an_incomplete_form_expands_errors_and_focuses_the_first() {
        let ctx = deepseek();
        let mut state = add(&ctx, "my-model");
        let outcome = run(&mut state, &ctx, Action::Save);
        assert!(outcome.save.is_none());
        assert_eq!(outcome.focus, Some(Focus::Thinking));
        let view = state.view(&ctx);
        assert_eq!(view.thinking.error.as_deref(), Some("选一种思考方式"));
        assert_eq!(view.length.context.error.as_deref(), Some("填写上下文长度"));
        assert_eq!(view.length.output.error.as_deref(), Some("填写最长输出"));
        assert!(section(&view, Section::Thinking).error);
        assert!(section(&view, Section::Length).error);
        assert_eq!(state.id, "my-model", "typed values stay");
        // Fix thinking; length is next.
        run(
            &mut state,
            &ctx,
            Action::SetKind {
                kind: ThinkingKind::Unsupported,
            },
        );
        assert_eq!(
            run(&mut state, &ctx, Action::Save).focus,
            Some(Focus::Context)
        );
        run(
            &mut state,
            &ctx,
            Action::SetLength {
                field: LengthField::Context,
                text: "32K".into(),
            },
        );
        assert_eq!(
            run(&mut state, &ctx, Action::Save).focus,
            Some(Focus::Output)
        );
        run(
            &mut state,
            &ctx,
            Action::SetLength {
                field: LengthField::Output,
                text: "4K".into(),
            },
        );
        let input = run(&mut state, &ctx, Action::Save).save.unwrap();
        assert_eq!(input.id, "my-model");
        assert_eq!(input.thinking, "off");
        // In summary mode, a failed save opens the sections with errors.
        let mut state = add(&ctx, "deepseek-reasoner");
        run(
            &mut state,
            &ctx,
            Action::SetLength {
                field: LengthField::Output,
                text: "".into(),
            },
        );
        assert_eq!(
            run(&mut state, &ctx, Action::Save).focus,
            Some(Focus::Output)
        );
        assert_eq!(state.open, vec![Section::Length]);
        // An empty id is reported on save only.
        let mut state = EditorState::open_add(&ctx);
        assert!(state.view(&ctx).id.error.is_none());
        assert_eq!(run(&mut state, &ctx, Action::Save).focus, Some(Focus::Id));
        assert_eq!(
            state.view(&ctx).id.error.as_deref(),
            Some("填写供应商提供的模型 ID")
        );
    }

    #[test]
    fn length_fields_parse_normalize_step_and_conflict() {
        let ctx = deepseek();
        let mut state = add(&ctx, "my-model");
        let set = |state: &mut EditorState, field, text: &str| {
            run(
                state,
                &ctx,
                Action::SetLength {
                    field,
                    text: text.into(),
                },
            );
        };
        set(&mut state, LengthField::Context, "12x");
        assert!(
            state.view(&ctx).length.context.error.is_none(),
            "not while typing"
        );
        run(
            &mut state,
            &ctx,
            Action::BlurLength {
                field: LengthField::Context,
            },
        );
        assert_eq!(
            state.view(&ctx).length.context.error.as_deref(),
            Some("写成 128K 或 131072 这样的数字")
        );
        assert_eq!(state.context, "12x", "unparsable text stays");
        set(&mut state, LengthField::Context, "131072");
        let view = state.view(&ctx);
        assert!(view.length.context.error.is_none());
        assert_eq!(view.length.context.exact.as_deref(), Some("131,072"));
        run(
            &mut state,
            &ctx,
            Action::BlurLength {
                field: LengthField::Context,
            },
        );
        assert_eq!(state.context, "131,072");
        set(&mut state, LengthField::Context, "128000");
        run(
            &mut state,
            &ctx,
            Action::BlurLength {
                field: LengthField::Context,
            },
        );
        assert_eq!(state.context, "128K");
        // Output ≥ context shows at once, on the output field.
        set(&mut state, LengthField::Output, "200K");
        assert_eq!(
            state.view(&ctx).length.output.error.as_deref(),
            Some("需小于上下文 128K")
        );
        // … also when the user just changed the context.
        set(&mut state, LengthField::Output, "64K");
        assert!(state.view(&ctx).length.output.error.is_none());
        set(&mut state, LengthField::Context, "32K");
        let view = state.view(&ctx);
        assert!(view.length.context.error.is_none());
        assert_eq!(
            view.length.output.error.as_deref(),
            Some("需小于上下文 32K")
        );
        // Quick picks ≥ context are disabled and skipped by ↑/↓.
        let disabled: Vec<bool> = view
            .length
            .output
            .picks
            .iter()
            .map(|p| p.disabled)
            .collect();
        assert_eq!(disabled, [false, false, true, true, true]);
        set(&mut state, LengthField::Output, "");
        run(
            &mut state,
            &ctx,
            Action::StepLength {
                field: LengthField::Output,
                up: true,
            },
        );
        assert_eq!(state.output, "8K");
        run(
            &mut state,
            &ctx,
            Action::StepLength {
                field: LengthField::Output,
                up: true,
            },
        );
        assert_eq!(state.output, "16K");
        run(
            &mut state,
            &ctx,
            Action::StepLength {
                field: LengthField::Output,
                up: true,
            },
        );
        assert_eq!(state.output, "16K", "stays at the last enabled pick");
        run(
            &mut state,
            &ctx,
            Action::StepLength {
                field: LengthField::Output,
                up: false,
            },
        );
        assert_eq!(state.output, "8K");
        run(
            &mut state,
            &ctx,
            Action::StepLength {
                field: LengthField::Output,
                up: false,
            },
        );
        assert_eq!(state.output, "8K");
        run(
            &mut state,
            &ctx,
            Action::PickLength {
                field: LengthField::Output,
                value: 64_000,
            },
        );
        assert_eq!(state.output, "8K", "disabled pick ignored");
        set(&mut state, LengthField::Context, "");
        run(
            &mut state,
            &ctx,
            Action::StepLength {
                field: LengthField::Context,
                up: true,
            },
        );
        assert_eq!(state.context, "32K");
        run(
            &mut state,
            &ctx,
            Action::StepLength {
                field: LengthField::Context,
                up: true,
            },
        );
        assert_eq!(state.context, "128K");
        set(&mut state, LengthField::Context, "150K");
        run(
            &mut state,
            &ctx,
            Action::StepLength {
                field: LengthField::Context,
                up: false,
            },
        );
        assert_eq!(state.context, "128K", "steps to the nearest pick below");
        run(
            &mut state,
            &ctx,
            Action::PickLength {
                field: LengthField::Context,
                value: 1_000_000,
            },
        );
        assert_eq!(state.context, "1M");
        let view = state.view(&ctx);
        assert!(view.length.context.picks[4].selected);
        assert_eq!(view.length.context.placeholder, "例如 128K");
        assert_eq!(view.length.output.placeholder, "例如 8K");
        assert_eq!(
            section(&view, Section::Length).summary,
            "上下文 1M · 最长输出 8K"
        );
        // Empty fields only complain after a save attempt.
        set(&mut state, LengthField::Output, "");
        run(
            &mut state,
            &ctx,
            Action::BlurLength {
                field: LengthField::Output,
            },
        );
        assert!(state.view(&ctx).length.output.error.is_none());
    }

    #[test]
    fn switching_kinds_keeps_each_kinds_last_configuration() {
        let ctx = ctx("openai", vec![]);
        let mut state = add(&ctx, "gpt-5");
        run(&mut state, &ctx, Action::LevelDelete { index: 0 });
        let edited = state.scheme.clone();
        run(
            &mut state,
            &ctx,
            Action::SetKind {
                kind: ThinkingKind::Toggle,
            },
        );
        assert_eq!(
            state.scheme,
            Some(ThinkingScheme::default_for(ThinkingKind::Toggle))
        );
        run(&mut state, &ctx, Action::ToggleDefault { on: true });
        run(
            &mut state,
            &ctx,
            Action::SetKind {
                kind: ThinkingKind::Levels,
            },
        );
        assert_eq!(state.scheme, edited, "levels come back as left");
        run(
            &mut state,
            &ctx,
            Action::SetKind {
                kind: ThinkingKind::Toggle,
            },
        );
        assert_eq!(
            state.scheme,
            Some(ThinkingScheme::Toggle {
                on: "high".into(),
                default_on: true
            })
        );
        run(
            &mut state,
            &ctx,
            Action::SetKind {
                kind: ThinkingKind::Toggle,
            },
        );
        assert!(matches!(
            state.scheme,
            Some(ThinkingScheme::Toggle {
                default_on: true,
                ..
            })
        ));
        // A kind the source uses starts from the source's configuration.
        let mut edit = EditorState::open_edit(
            &ctx_with(
                "openai",
                vec![configured(
                    "gpt-5",
                    400_000,
                    128_000,
                    &["off", "high"],
                    "off",
                )],
            ),
            "gpt-5",
        )
        .unwrap();
        let ctx2 = ctx_with(
            "openai",
            vec![configured(
                "gpt-5",
                400_000,
                128_000,
                &["off", "high"],
                "off",
            )],
        );
        run(
            &mut edit,
            &ctx2,
            Action::SetKind {
                kind: ThinkingKind::Levels,
            },
        );
        assert_eq!(
            edit.scheme,
            Some(ThinkingScheme::Levels {
                values: ["minimal", "low", "medium", "high"]
                    .map(String::from)
                    .to_vec(),
                default: "medium".into()
            })
        );
    }

    fn ctx_with(provider: &str, models: Vec<Value>) -> Context {
        ctx(provider, models)
    }

    #[test]
    fn level_editing_through_actions() {
        let ctx = deepseek();
        let mut state = add(&ctx, "my-model");
        run(
            &mut state,
            &ctx,
            Action::SetKind {
                kind: ThinkingKind::Levels,
            },
        );
        let levels = state.view(&ctx).thinking.levels.unwrap();
        assert_eq!(levels.addable, vec!["minimal", "xhigh", "max"]);
        assert!(levels.help.ends_with("现在是 medium"));
        run(
            &mut state,
            &ctx,
            Action::LevelAdd {
                name: "xhigh".into(),
            },
        );
        run(&mut state, &ctx, Action::LevelAdd { name: "low".into() });
        let levels = state.view(&ctx).thinking.levels.unwrap();
        assert_eq!(levels.add_error.as_deref(), Some("已经有 low"));
        run(&mut state, &ctx, Action::ClearAddErrors);
        assert!(state
            .view(&ctx)
            .thinking
            .levels
            .unwrap()
            .add_error
            .is_none());
        run(
            &mut state,
            &ctx,
            Action::LevelDefault {
                name: "high".into(),
            },
        );
        run(&mut state, &ctx, Action::LevelMove { from: 3, to: 0 });
        run(&mut state, &ctx, Action::LevelDelete { index: 3 });
        assert_eq!(
            state.scheme,
            Some(ThinkingScheme::Levels {
                values: ["xhigh", "low", "medium"].map(String::from).to_vec(),
                default: "medium".into()
            }),
            "deleting the default moves to its neighbour"
        );
        let view = state.view(&ctx);
        let chips: Vec<(&str, bool)> = view
            .thinking
            .levels
            .as_ref()
            .unwrap()
            .values
            .iter()
            .map(|c| (c.name.as_str(), c.default))
            .collect();
        assert_eq!(chips, [("xhigh", false), ("low", false), ("medium", true)]);
        assert_eq!(
            view.thinking.panel.unwrap().panel,
            Panel::Options {
                options: ["xhigh", "low", "medium"].map(String::from).to_vec(),
                selected: 2
            }
        );
        run(&mut state, &ctx, Action::LevelDelete { index: 0 });
        run(&mut state, &ctx, Action::LevelDelete { index: 0 });
        assert!(!state.view(&ctx).thinking.levels.unwrap().can_delete);
        run(&mut state, &ctx, Action::LevelDelete { index: 0 });
        assert!(
            matches!(&state.scheme, Some(ThinkingScheme::Levels { values, .. }) if values.len() == 1)
        );
    }

    #[test]
    fn budget_editing_through_actions() {
        let ctx = deepseek();
        let mut state = add(&ctx, "my-model");
        run(
            &mut state,
            &ctx,
            Action::SetLength {
                field: LengthField::Context,
                text: "128K".into(),
            },
        );
        run(
            &mut state,
            &ctx,
            Action::SetLength {
                field: LengthField::Output,
                text: "16K".into(),
            },
        );
        run(
            &mut state,
            &ctx,
            Action::SetKind {
                kind: ThinkingKind::Budget,
            },
        );
        let view = state.view(&ctx);
        let budget = view.thinking.budget.clone().unwrap();
        let bad: Vec<(&str, bool)> = budget
            .presets
            .iter()
            .map(|c| (c.label.as_str(), c.bad))
            .collect();
        assert_eq!(bad, [("4K", false), ("16K", true), ("32K", true)]);
        assert_eq!(
            view.thinking.error.as_deref(),
            Some("预算需小于最长输出 16K")
        );
        assert!(section(&view, Section::Thinking).error);
        let defaults: Vec<&str> = budget.defaults.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(defaults, ["关", "4K", "16K", "32K"]);
        assert!(budget.defaults[0].selected);
        run(&mut state, &ctx, Action::BudgetAdd { text: "16K".into() });
        assert_eq!(
            state
                .view(&ctx)
                .thinking
                .budget
                .unwrap()
                .add_error
                .as_deref(),
            Some("已经有 16K")
        );
        run(&mut state, &ctx, Action::BudgetAdd { text: "20K".into() });
        assert_eq!(
            state
                .view(&ctx)
                .thinking
                .budget
                .unwrap()
                .add_error
                .as_deref(),
            Some("需小于最长输出 16K")
        );
        run(&mut state, &ctx, Action::BudgetAdd { text: "abc".into() });
        assert_eq!(
            state
                .view(&ctx)
                .thinking
                .budget
                .unwrap()
                .add_error
                .as_deref(),
            Some("写成 8K 或 8192 这样的数字")
        );
        run(&mut state, &ctx, Action::BudgetAdd { text: "8K".into() });
        assert!(state
            .view(&ctx)
            .thinking
            .budget
            .unwrap()
            .add_error
            .is_none());
        run(&mut state, &ctx, Action::BudgetDelete { index: 3 });
        run(&mut state, &ctx, Action::BudgetDelete { index: 2 });
        assert!(state.view(&ctx).thinking.error.is_none());
        run(&mut state, &ctx, Action::BudgetDynamic { on: true });
        run(
            &mut state,
            &ctx,
            Action::BudgetDefault {
                choice: BudgetChoice::Dynamic,
            },
        );
        run(&mut state, &ctx, Action::BudgetDynamic { on: false });
        assert!(matches!(
            state.scheme,
            Some(ThinkingScheme::Budget {
                default: BudgetChoice::Off,
                ..
            })
        ));
        run(
            &mut state,
            &ctx,
            Action::BudgetDefault {
                choice: BudgetChoice::Tokens(8_000),
            },
        );
        run(&mut state, &ctx, Action::BudgetDelete { index: 1 });
        run(&mut state, &ctx, Action::BudgetAllowOff { on: false });
        let view = state.view(&ctx);
        assert_eq!(view.thinking.budget.as_ref().unwrap().presets.len(), 1);
        assert!(matches!(
            state.scheme,
            Some(ThinkingScheme::Budget {
                default: BudgetChoice::Tokens(4_000),
                ..
            })
        ));
        run(&mut state, &ctx, Action::BudgetDelete { index: 0 });
        assert_eq!(
            state.view(&ctx).thinking.error.as_deref(),
            Some("至少保留一个预算，或改成“不思考”"),
            "shown right away"
        );
        assert_eq!(
            run(&mut state, &ctx, Action::Save).focus,
            Some(Focus::Thinking)
        );
    }

    #[test]
    fn filling_from_a_similar_model_or_preset() {
        let ctx = deepseek();
        let mut state = add(&ctx, "my-model");
        let groups = state.sources(&ctx, "");
        assert_eq!(groups[0].title, "你的模型");
        let mine: Vec<&str> = groups[0].items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(mine, ["deepseek-chat", "qwen3-32b"]);
        run(
            &mut state,
            &ctx,
            Action::UseSource {
                source: SourceRef::Model {
                    profile: "lab".into(),
                    model: "qwen3-32b".into(),
                },
            },
        );
        let view = state.view(&ctx);
        let status = view.status.clone().unwrap();
        assert_eq!(status.kind, "source");
        assert_eq!(status.title, "参数来自 qwen3-32b");
        assert!(!view.full);
        assert!(
            view.sections.iter().all(|s| s.open),
            "filled sections expand"
        );
        assert_eq!(state.api, "openai-completions", "protocol not copied");
        assert_eq!(state.id, "my-model", "id not copied");
        assert_eq!(
            section(&view, Section::Length).summary,
            "上下文 128K · 最长输出 16K"
        );
        run(&mut state, &ctx, Action::SetImage { on: true });
        assert_eq!(
            section(&state.view(&ctx), Section::Image)
                .provenance
                .unwrap()
                .text,
            "qwen3-32b：不能看图片"
        );
        // Preset source over a recognized id.
        let mut state = add(&ctx, "deepseek-reasoner");
        run(
            &mut state,
            &ctx,
            Action::UseSource {
                source: SourceRef::Preset {
                    preset: "gpt-5".into(),
                },
            },
        );
        let status = state.view(&ctx).status.unwrap();
        assert_eq!(status.title, "DeepSeek Reasoner");
        assert_eq!(status.detail, "参数来自 GPT-5 预设");
        assert_eq!(state.context, "400K");
        assert!(state
            .apply(
                &ctx,
                Action::UseSource {
                    source: SourceRef::Preset {
                        preset: "missing".into()
                    }
                }
            )
            .is_err());
        // A source the user picked is not replaced when the id changes.
        let ctx = ctx_with("openai", vec![]);
        let mut state = add(&ctx, "gpt-5");
        run(
            &mut state,
            &ctx,
            Action::UseSource {
                source: SourceRef::Preset {
                    preset: "o3".into(),
                },
            },
        );
        type_id(&mut state, &ctx, "gpt-5-2026-01-01");
        assert!(state.refill.is_none(), "same preset, nothing to offer");
        type_id(&mut state, &ctx, "gpt-4o");
        assert_eq!(
            state.context,
            compact_tokens(model_catalog::preset("o3").unwrap().context)
        );
        assert_eq!(state.refill.as_deref(), Some("gpt-4o"));
        type_id(&mut state, &ctx, "unknown-thing");
        run(&mut state, &ctx, Action::Settle { id: None });
        assert_eq!(
            state.source.as_ref().unwrap().name,
            "o3",
            "picked source stays"
        );
        assert_eq!(state.view(&ctx).status.unwrap().title, "参数来自 o3");
    }

    #[test]
    fn editing_a_configured_model() {
        let ctx = deepseek();
        let mut state = EditorState::open_edit(&ctx, "deepseek-chat").unwrap();
        let view = state.view(&ctx);
        assert_eq!(view.title, "deepseek-chat");
        assert!(!view.id.editable);
        assert!(view.id.suggestions.is_none());
        assert!(view.footer.remove);
        assert_eq!(view.footer.save, "保存");
        let status = view.status.clone().unwrap();
        assert_eq!(status.title, "DeepSeek V3.2");
        assert_eq!(status.detail, "与预设一致");
        assert!(!view.full);
        run(&mut state, &ctx, Action::SetImage { on: true });
        run(&mut state, &ctx, Action::ToggleDefault { on: true });
        assert_eq!(state.view(&ctx).status.unwrap().detail, "2 项与预设不同");
        run(&mut state, &ctx, Action::SetId { id: "other".into() });
        assert_eq!(state.id, "deepseek-chat", "id is not editable here");
        let input = run(&mut state, &ctx, Action::Save).save.unwrap();
        assert_eq!(input.previous.as_ref().unwrap()["id"], "deepseek-chat");
        let models = input.apply(ctx.profile.models.clone()).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["enabled"], true, "enabled state kept");
        assert_eq!(models[0]["default_thinking"], "high");
        assert!(crate::model_edit::model_accepts_images(&models[0]));
        assert!(EditorState::open_edit(&ctx, "missing").is_err());
    }

    #[test]
    fn editing_an_unconfigured_model_opens_the_numbered_form() {
        let ctx = ctx(
            "deepseek",
            vec![
                json!({"id":"internal-x","enabled":false,"thinking":["off"],"default_thinking":"off"}),
                json!({"id":"internal-y","enabled":false,"thinking":["off","low","high"],"default_thinking":"low"}),
                json!({"id":"deepseek-reasoner","enabled":false}),
            ],
        );
        let state = EditorState::open_edit(&ctx, "internal-x").unwrap();
        let view = state.view(&ctx);
        assert!(view.full);
        assert_eq!(view.status.clone().unwrap().title, "没有这个模型的预设");
        assert_eq!(view.thinking.selected, None);
        let state = EditorState::open_edit(&ctx, "internal-y").unwrap();
        assert_eq!(
            state.view(&ctx).thinking.selected,
            Some(ThinkingKind::Levels)
        );
        let mut state = EditorState::open_edit(&ctx, "deepseek-reasoner").unwrap();
        let view = state.view(&ctx);
        assert!(view.full);
        assert_eq!(state.context, "128K");
        assert_eq!(view.status.clone().unwrap().detail, "与预设一致");
        let input = run(&mut state, &ctx, Action::Save).save.unwrap();
        let models = input.apply(ctx.profile.models.clone()).unwrap();
        assert_eq!(
            models[2]["enabled"], false,
            "turning it on stays with the user"
        );
    }

    #[test]
    fn custom_connections_edit_the_protocol() {
        let ctx = ctx("openai-compatible", vec![]);
        let mut state = add(&ctx, "my-model");
        let view = state.view(&ctx);
        let api = view.api.clone().unwrap();
        assert_eq!(api.options.len(), 4);
        assert!(api
            .options
            .iter()
            .any(|o| o.default && o.api == "openai-completions"));
        assert!(api.warning.is_none());
        let protocol = section(&view, Section::Api);
        assert_eq!(protocol.number, 4);
        assert_eq!(protocol.question, "接口协议");
        assert_eq!(protocol.summary, "OpenAI Chat Completions · 跟随连接");
        run(
            &mut state,
            &ctx,
            Action::SetApi {
                api: "anthropic-messages".into(),
            },
        );
        let view = state.view(&ctx);
        assert_eq!(
            view.api.clone().unwrap().warning.as_deref(),
            Some("这个连接默认用 OpenAI Chat Completions，确认供应商支持再改")
        );
        assert_eq!(section(&view, Section::Api).summary, "Anthropic Messages");
        run(
            &mut state,
            &ctx,
            Action::SetApi {
                api: "bogus".into(),
            },
        );
        assert_eq!(state.api, "anthropic-messages");
        // Fill sources never change the protocol.
        run(
            &mut state,
            &ctx,
            Action::UseSource {
                source: SourceRef::Preset {
                    preset: "gpt-5".into(),
                },
            },
        );
        assert_eq!(state.api, "anthropic-messages");
        // Other connections ignore protocol edits.
        let ctx = deepseek();
        let mut state = add(&ctx, "my-model");
        run(
            &mut state,
            &ctx,
            Action::SetApi {
                api: "anthropic-messages".into(),
            },
        );
        assert_eq!(state.api, "openai-completions");
    }

    #[test]
    fn thinking_preview_and_request_field() {
        let ctx = deepseek();
        let mut state = add(&ctx, "my-model");
        assert!(state.view(&ctx).thinking.panel.is_none());
        let expect = [
            (
                ThinkingKind::Unsupported,
                Some("不显示思考选项"),
                Some("请求里不带思考参数"),
            ),
            (
                ThinkingKind::Always,
                Some("思考中（不可调）"),
                Some("每次请求都会思考，模型面板里不提供开关"),
            ),
            (ThinkingKind::Toggle, None, None),
        ];
        for (kind, text, note) in expect {
            run(&mut state, &ctx, Action::SetKind { kind });
            let view = state.view(&ctx);
            assert_eq!(view.thinking.panel.unwrap().text, text);
            assert_eq!(view.thinking.note, note);
            assert_eq!(view.thinking.request_field, "由系统按接口协议写入请求");
        }
        assert_eq!(state.view(&ctx).thinking.toggle.unwrap().default_on, false);
        let state = add(&ctx_with("openai", vec![]), "gpt-5");
        assert_eq!(
            state
                .view(&ctx_with("openai", vec![]))
                .thinking
                .request_field,
            "请求里写入 reasoning.effort"
        );
        let kinds: Vec<&str> = state
            .view(&ctx_with("openai", vec![]))
            .thinking
            .kinds
            .iter()
            .map(|k| k.title)
            .collect();
        assert_eq!(
            kinds,
            [
                "不思考",
                "总是思考",
                "可以开关",
                "按档位调节",
                "按 token 预算"
            ]
        );
    }

    #[test]
    fn suggestions_come_with_the_add_view() {
        let mut ctx = deepseek();
        ctx.reported = vec!["deepseek-reasoner".into(), "deepseek-internal-x".into()];
        let mut state = EditorState::open_add(&ctx);
        run(&mut state, &ctx, Action::SetId { id: "deeps".into() });
        let view = state.view(&ctx);
        let suggestions = view.id.suggestions.unwrap();
        assert_eq!(suggestions.groups[0].title, "My deepseek 上可用");
        assert_eq!(suggestions.custom.unwrap().id, "deeps");
        // Picking one settles the id and recognizes it at once.
        run(
            &mut state,
            &ctx,
            Action::Settle {
                id: Some("deepseek-reasoner".into()),
            },
        );
        let view = state.view(&ctx);
        assert!(!view.id.pending);
        assert_eq!(view.status.clone().unwrap().title, "DeepSeek Reasoner");
    }

    #[test]
    fn id_edge_cases() {
        let ctx = deepseek();
        let long = "x".repeat(300);
        let mut state = add(&ctx, &long);
        assert!(state.view(&ctx).id.error.unwrap().contains("200"));
        assert_eq!(run(&mut state, &ctx, Action::Save).focus, Some(Focus::Id));
        let state = add(&ctx, "  deepseek-reasoner  ");
        assert_eq!(state.view(&ctx).status.unwrap().title, "DeepSeek Reasoner");
        assert_eq!(state.input().id, "deepseek-reasoner", "trimmed");
        let mut state = add(&ctx, "");
        assert!(state.view(&ctx).sections.is_empty());
        assert_eq!(run(&mut state, &ctx, Action::Save).focus, Some(Focus::Id));
    }

    #[test]
    fn stale_editors_are_rejected() {
        let ctx = deepseek();
        let mut state = EditorState::open_add(&ctx);
        let other = ctx_with("openai", vec![]);
        assert!(state.apply(&other, Action::SetImage { on: true }).is_err());
        assert!(state.apply(&other, Action::OpenAdd).is_ok());
        assert_eq!(state.connection, "my-openai");
    }

    #[test]
    fn json_surface_round_trips_state() {
        let ctx = serde_json::to_value(deepseek()).unwrap();
        let opened = handle(json!({"context": ctx, "action": {"type": "open_add"}})).unwrap();
        assert_eq!(opened["view"]["title"], "添加模型");
        let typed = handle(json!({"context": ctx, "state": opened["state"],
            "action": {"type": "settle", "id": "deepseek-reasoner"}}))
        .unwrap();
        assert_eq!(typed["view"]["status"]["title"], "DeepSeek Reasoner");
        assert_eq!(typed["view"]["sections"][0]["section"], "thinking");
        let changed = handle(json!({"context": ctx, "state": typed["state"],
            "action": {"type": "budget_default", "choice": {"kind": "tokens", "tokens": 4000}}}))
        .unwrap();
        assert!(changed["effect"].is_null());
        let saved =
            handle(json!({"context": ctx, "state": changed["state"], "action": {"type": "save"}}))
                .unwrap();
        assert_eq!(saved["effect"]["save"]["id"], "deepseek-reasoner");
        assert!(saved["focus"].is_null());
        let failed =
            handle(json!({"context": ctx, "state": opened["state"], "action": {"type": "save"}}))
                .unwrap();
        assert_eq!(failed["focus"], "id");
        assert!(handle(json!({"context": ctx, "action": {"type": "save"}})).is_err());
        let edit = handle(
            json!({"context": ctx, "action": {"type": "open_edit", "model": "deepseek-chat"}}),
        )
        .unwrap();
        assert_eq!(edit["view"]["title"], "deepseek-chat");
        let sources =
            handle_sources(json!({"context": ctx, "state": edit["state"], "query": "qwen"}))
                .unwrap();
        assert_eq!(
            sources[0]["items"][0]["ref"],
            json!({"profile":"lab","model":"qwen3-32b"})
        );
        let sources =
            handle_sources(json!({"context": ctx, "state": edit["state"], "query": ""})).unwrap();
        assert!(
            sources[0]["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|i| i["label"] != "deepseek-chat"),
            "the edited model is not its own source"
        );
        // The state survives a JSON round trip unchanged.
        let state: EditorState = serde_json::from_value(changed["state"].clone()).unwrap();
        assert_eq!(serde_json::to_value(&state).unwrap(), changed["state"]);
    }
}
