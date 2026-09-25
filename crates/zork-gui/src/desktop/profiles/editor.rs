//! Model editor: the add dialog and the inline editor under a model row.
//!
//! Every rule (recognition, preset fill, provenance, errors, thinking editors)
//! lives in `zork_client_core::model_editor`. This file forwards edits as core
//! actions and renders the returned view. The only local state is
//! presentation: which popover is open, the highlighted row, the focused field
//! and a drag in progress.
use super::*;
use crate::api::model_catalog::{FillSourceGroup, SourceRef};
use crate::api::model_editor::{
    Action as Edit, Context as EditorContext, EditorState, Focus, LengthField, Mode, Section,
    SectionView, TokenFieldView, View,
};
use crate::api::thinking::{BudgetChoice, Panel};
use crate::components::text_input::{ComposerEdited, ComposerSubmit, FieldDown, FieldUp};
use std::time::Duration;
use zork_ui::motion::MotionExt as _;

/// Recognition waits this long after the last keystroke in the id field.
const RECOGNIZE_DELAY: Duration = Duration::from_millis(250);
/// Label column of summary rows; bodies align with the value column.
const LABEL_WIDTH: f32 = 56.;
const SUMMARY_INDENT: f32 = 12. + LABEL_WIDTH + 10.;

/// The editor's text fields. They live as long as the view so focus, caret
/// and IME state are never rebuilt while typing.
pub(super) struct Inputs {
    pub id: Entity<ComposerInput>,
    pub context: Entity<ComposerInput>,
    pub output: Entity<ComposerInput>,
    pub level: Entity<ComposerInput>,
    pub budget: Entity<ComposerInput>,
    pub query: Entity<ComposerInput>,
}

impl Inputs {
    pub(super) fn new(cx: &mut Context<ProfilesView>) -> Self {
        let mut field = |placeholder: &'static str| {
            cx.new(|cx| ComposerInput::new(placeholder, cx).single_line())
        };
        let inputs = Self {
            id: field("模型 ID，例如 deepseek-chat"),
            context: field("例如 128K"),
            output: field("例如 8K"),
            level: field("其他名称，回车添加"),
            budget: field("例如 8K"),
            query: field("搜索模型或预设"),
        };
        cx.subscribe(&inputs.id, |v, _, _: &ComposerEdited, cx| v.id_edited(cx))
            .detach();
        for (input, field) in [
            (&inputs.context, LengthField::Context),
            (&inputs.output, LengthField::Output),
        ] {
            cx.subscribe(input, move |v, input, _: &ComposerEdited, cx| {
                let text = input.read(cx).value().to_owned();
                if v.editor.as_ref().is_some_and(|e| {
                    let current = match field {
                        LengthField::Context => &e.state.context,
                        LengthField::Output => &e.state.output,
                    };
                    *current != text
                }) {
                    v.act(Edit::SetLength { field, text }, cx);
                }
            })
            .detach();
        }
        for input in [&inputs.level, &inputs.budget] {
            cx.subscribe(input, |v, _, _: &ComposerEdited, cx| {
                if v.editor.as_ref().is_some_and(|e| {
                    e.view
                        .thinking
                        .levels
                        .as_ref()
                        .is_some_and(|l| l.add_error.is_some())
                        || e.view
                            .thinking
                            .budget
                            .as_ref()
                            .is_some_and(|b| b.add_error.is_some())
                }) {
                    v.act(Edit::ClearAddErrors, cx);
                }
            })
            .detach();
        }
        cx.subscribe(&inputs.query, |v, input, _: &ComposerEdited, cx| {
            let query = input.read(cx).value().to_owned();
            v.search_sources(&query, cx);
        })
        .detach();
        // Enter arrives as a submit from the field itself.
        cx.subscribe(&inputs.id, |v, _, _: &ComposerSubmit, cx| v.id_enter(cx))
            .detach();
        for (input, field) in [
            (&inputs.context, LengthField::Context),
            (&inputs.output, LengthField::Output),
        ] {
            cx.subscribe(input, move |v, _, _: &ComposerSubmit, cx| {
                v.act(Edit::BlurLength { field }, cx)
            })
            .detach();
        }
        cx.subscribe(&inputs.level, |v, _, _: &ComposerSubmit, cx| {
            v.level_enter(cx)
        })
        .detach();
        cx.subscribe(&inputs.budget, |v, _, _: &ComposerSubmit, cx| {
            v.add_budget(cx)
        })
        .detach();
        cx.subscribe(&inputs.query, |v, _, _: &ComposerSubmit, cx| {
            v.query_enter(cx)
        })
        .detach();
        inputs
    }
}

pub(super) struct Sources {
    pub groups: Vec<FillSourceGroup>,
    pub highlight: usize,
}

impl Sources {
    fn items(&self) -> Vec<(SourceRef, String)> {
        self.groups
            .iter()
            .flat_map(|g| {
                g.items
                    .iter()
                    .map(|i| (i.reference.clone(), i.label.clone()))
            })
            .collect()
    }
}

/// Which element takes focus on the next frame.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    Id,
    Thinking,
    Context,
    Output,
    Api,
    Query,
    Level,
    Budget,
    /// Triggers that get focus back when their popover closes.
    SourceOpen,
    LevelAdd,
    BudgetAdd,
}

pub(super) struct Editor {
    pub state: EditorState,
    pub view: View,
    pub suggest_open: bool,
    pub suggest_hl: Option<usize>,
    pub sources: Option<Sources>,
    pub level_open: bool,
    pub level_hl: Option<usize>,
    pub budget_open: bool,
    pub busy: bool,
    pub error: Option<String>,
    focus: Option<Target>,
    recognize: Option<Task<()>>,
    pub scroll: gpui::ScrollHandle,
    /// Focus of the first thinking / protocol option, known once rendered.
    kind_focus: Option<gpui::FocusHandle>,
    api_focus: Option<gpui::FocusHandle>,
    anchors: HashMap<Section, gpui::ScrollAnchor>,
    /// Which of id, context, output, budget and level had focus last frame.
    focused: [bool; 5],
    prepared: bool,
}

impl Editor {
    fn new(state: EditorState, view: View) -> Self {
        let scroll = gpui::ScrollHandle::new();
        let anchors = [
            Section::Thinking,
            Section::Length,
            Section::Image,
            Section::Api,
        ]
        .into_iter()
        .map(|s| (s, gpui::ScrollAnchor::for_handle(scroll.clone())))
        .collect();
        Self {
            state,
            view,
            suggest_open: false,
            suggest_hl: None,
            sources: None,
            level_open: false,
            level_hl: None,
            budget_open: false,
            busy: false,
            error: None,
            focus: None,
            recognize: None,
            scroll,
            kind_focus: None,
            api_focus: None,
            anchors,
            focused: [false; 5],
            prepared: false,
        }
    }
    fn suggestion_ids(&self) -> Vec<String> {
        self.view
            .id
            .suggestions
            .as_ref()
            .map(|s| s.flat())
            .unwrap_or_default()
    }
    pub fn editing(&self) -> Option<&str> {
        match self.state.mode {
            Mode::Edit => self.state.original.as_ref()?["id"].as_str(),
            Mode::Add => None,
        }
    }
}

impl ProfilesView {
    // ------------------------------------------------------------ state

    /// Everything the core editor needs about this connection.
    pub(super) fn editor_context(&self) -> Option<EditorContext> {
        let detail = self.detail.as_ref()?;
        let profile = serde_json::from_value(detail.clone()).ok()?;
        let profiles = self
            .profiles
            .iter()
            .filter_map(|p| serde_json::to_value(p).ok())
            .filter_map(|p| serde_json::from_value(p).ok())
            .collect();
        let reported = detail["profile_id"]
            .as_str()
            .and_then(|id| self.reported.get(id))
            .cloned()
            .unwrap_or_default();
        Some(EditorContext {
            profile,
            providers: self.catalog.clone(),
            profiles,
            reported,
        })
    }

    pub(super) fn open_add_model(&mut self, cx: &mut Context<Self>) {
        let Some(ctx) = self.editor_context() else {
            return;
        };
        let state = EditorState::open_add(&ctx);
        let view = state.view(&ctx);
        let mut editor = Editor::new(state, view);
        editor.focus = Some(Target::Id);
        self.editor = Some(editor);
        self.message = None;
        self.sync_inputs(true, cx);
        self.fetch_reported(cx);
        self.editor_changed(cx);
    }

    pub(super) fn open_edit_model(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(ctx) = self.editor_context() else {
            return;
        };
        let Ok(state) = EditorState::open_edit(&ctx, id) else {
            return;
        };
        let view = state.view(&ctx);
        self.editor = Some(Editor::new(state, view));
        self.message = None;
        self.sync_inputs(true, cx);
        self.editor_changed(cx);
    }

    /// A row click toggles its inline editor.
    pub(super) fn toggle_edit_model(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.editor.as_ref().and_then(Editor::editing) == Some(id) {
            self.close_editor(cx);
        } else {
            self.open_edit_model(id, cx);
        }
    }

    pub(super) fn close_editor(&mut self, cx: &mut Context<Self>) {
        if self.editor.as_ref().is_some_and(|e| e.busy) {
            return;
        }
        self.editor = None;
        self.editor_changed(cx);
    }

    fn editor_changed(&mut self, cx: &mut Context<Self>) {
        zork_ui::components::region::invalidate_all(cx);
        cx.notify();
    }

    /// Re-projects the editor after the connection changed elsewhere; an
    /// editor whose model disappeared closes.
    pub(super) fn refresh_editor(&mut self, cx: &mut Context<Self>) {
        if self.editor.is_none() {
            return;
        }
        if self.editor.as_ref().is_some_and(|e| {
            self.detail.as_ref().and_then(|d| d["profile_id"].as_str())
                != Some(e.state.connection.as_str())
        }) {
            self.editor = None;
            self.editor_changed(cx);
            return;
        }
        self.act(Edit::View, cx);
    }

    /// Applies one core action and keeps the fields in step with the result.
    fn act(&mut self, action: Edit, cx: &mut Context<Self>) {
        let Some(ctx) = self.editor_context() else {
            return;
        };
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let save = matches!(action, Edit::Save);
        // Only these actions rewrite field text; everything else leaves the
        // fields alone so typing (and IME composition) is never disturbed.
        let rewrites = matches!(
            action,
            Edit::Settle { .. }
                | Edit::Recognize
                | Edit::Refill
                | Edit::UseSource { .. }
                | Edit::Restore { .. }
                | Edit::BlurLength { .. }
                | Edit::StepLength { .. }
                | Edit::PickLength { .. }
        );
        match editor.state.apply(&ctx, action) {
            Ok(outcome) => {
                editor.view = editor.state.view(&ctx);
                if let Some(focus) = outcome.focus {
                    editor.focus = Some(match focus {
                        Focus::Id => Target::Id,
                        Focus::Thinking => Target::Thinking,
                        Focus::Context => Target::Context,
                        Focus::Output => Target::Output,
                        Focus::Api => Target::Api,
                    });
                }
                if save {
                    editor.error = None;
                }
                let flat = editor.suggestion_ids().len();
                if editor.suggest_hl.is_some_and(|i| i >= flat) {
                    editor.suggest_hl = None;
                }
                if let Some(input) = outcome.save {
                    self.persist(input, cx);
                }
            }
            Err(error) => {
                // The model vanished or the connection changed underneath.
                self.editor = None;
                self.message = Some(error.to_string());
            }
        }
        if rewrites {
            self.sync_inputs(false, cx);
        }
        self.editor_changed(cx);
    }

    /// Writes core-owned text into the fields. Unchanged text is left alone so
    /// the caret and selection survive every keystroke.
    fn sync_inputs(&mut self, reset: bool, cx: &mut Context<Self>) {
        let Some(editor) = &self.editor else {
            return;
        };
        let values = [
            (self.editor_inputs.id.clone(), editor.state.id.clone()),
            (
                self.editor_inputs.context.clone(),
                editor.state.context.clone(),
            ),
            (
                self.editor_inputs.output.clone(),
                editor.state.output.clone(),
            ),
        ];
        for (input, value) in values {
            input.update(cx, |input, cx| {
                if reset {
                    input.reset_value(value, cx);
                } else {
                    input.set_value(value, cx);
                }
            });
        }
        if reset {
            for input in [
                &self.editor_inputs.level,
                &self.editor_inputs.budget,
                &self.editor_inputs.query,
            ] {
                input.update(cx, |input, cx| input.reset_value(String::new(), cx));
            }
        }
    }

    fn id_edited(&mut self, cx: &mut Context<Self>) {
        let text = self.editor_inputs.id.read(cx).value().to_owned();
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        if editor.state.mode != Mode::Add || editor.state.id == text {
            return;
        }
        editor.suggest_open = true;
        editor.suggest_hl = None;
        self.act(Edit::SetId { id: text }, cx);
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RECOGNIZE_DELAY).await;
            let _ = this.update(cx, |v, cx| v.act(Edit::Recognize, cx));
        });
        if let Some(editor) = self.editor.as_mut() {
            editor.recognize = Some(task);
        }
    }

    /// The id is final: a suggestion was picked, Enter pressed or focus left.
    fn settle_id(&mut self, pick: Option<String>, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        editor.recognize = None;
        editor.suggest_open = false;
        editor.suggest_hl = None;
        if editor.state.mode != Mode::Add || (pick.is_none() && editor.state.settled) {
            self.editor_changed(cx);
            return;
        }
        self.act(Edit::Settle { id: pick }, cx);
    }

    /// Enter in the id field picks the highlighted suggestion, or settles the
    /// typed id when nothing is highlighted.
    fn id_enter(&mut self, cx: &mut Context<Self>) {
        let pick = self.editor.as_ref().and_then(|e| {
            e.suggest_hl
                .filter(|_| e.suggest_open)
                .and_then(|i| e.suggestion_ids().get(i).cloned())
        });
        self.settle_id(pick, cx);
    }

    fn level_enter(&mut self, cx: &mut Context<Self>) {
        let typed = self.editor_inputs.level.read(cx).value().trim().to_owned();
        let pick = self.editor.as_ref().and_then(|e| {
            let highlight = e.level_hl?;
            e.view
                .thinking
                .levels
                .as_ref()?
                .addable
                .get(highlight)
                .map(|s| (*s).to_owned())
        });
        self.add_level(pick.unwrap_or(typed), cx);
    }

    fn query_enter(&mut self, cx: &mut Context<Self>) {
        let pick = self.editor.as_ref().and_then(|e| {
            let sources = e.sources.as_ref()?;
            sources.items().get(sources.highlight).map(|i| i.0.clone())
        });
        if let Some(source) = pick {
            self.use_source(source, cx);
        }
    }

    fn move_suggestion(&mut self, down: bool, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let count = editor.suggestion_ids().len();
        if count == 0 {
            return;
        }
        editor.suggest_open = true;
        editor.suggest_hl = Some(match (editor.suggest_hl, down) {
            (None, true) => 0,
            (None, false) => count - 1,
            (Some(i), true) => (i + 1) % count,
            (Some(i), false) => (i + count - 1) % count,
        });
        self.editor_changed(cx);
    }

    fn open_sources(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        if editor.sources.is_some() {
            editor.sources = None;
            self.editor_changed(cx);
            return;
        }
        editor.sources = Some(Sources {
            groups: vec![],
            highlight: 0,
        });
        editor.focus = Some(Target::Query);
        self.editor_inputs
            .query
            .update(cx, |input, cx| input.reset_value(String::new(), cx));
        self.search_sources("", cx);
    }

    fn search_sources(&mut self, query: &str, cx: &mut Context<Self>) {
        let Some(ctx) = self.editor_context() else {
            return;
        };
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        if let Some(sources) = editor.sources.as_mut() {
            sources.groups = editor.state.sources(&ctx, query);
            sources.highlight = 0;
        }
        self.editor_changed(cx);
    }

    fn use_source(&mut self, source: SourceRef, cx: &mut Context<Self>) {
        if let Some(editor) = self.editor.as_mut() {
            editor.sources = None;
            editor.focus = Some(Target::SourceOpen);
        }
        self.act(Edit::UseSource { source }, cx);
    }

    fn add_level(&mut self, name: String, cx: &mut Context<Self>) {
        self.act(Edit::LevelAdd { name }, cx);
        let added = self.editor.as_ref().is_some_and(|e| {
            e.view
                .thinking
                .levels
                .as_ref()
                .is_some_and(|l| l.add_error.is_none())
        });
        if added {
            if let Some(editor) = self.editor.as_mut() {
                editor.level_open = false;
                editor.level_hl = None;
                editor.focus = Some(Target::LevelAdd);
            }
            self.editor_inputs
                .level
                .update(cx, |input, cx| input.reset_value(String::new(), cx));
            self.editor_changed(cx);
        }
    }

    fn add_budget(&mut self, cx: &mut Context<Self>) {
        let text = self.editor_inputs.budget.read(cx).value().to_owned();
        self.act(Edit::BudgetAdd { text }, cx);
        let added = self.editor.as_ref().is_some_and(|e| {
            e.view
                .thinking
                .budget
                .as_ref()
                .is_some_and(|b| b.add_error.is_none())
        });
        if added {
            // Stay in the field for the next value.
            self.editor_inputs
                .budget
                .update(cx, |input, cx| input.reset_value(String::new(), cx));
        }
    }

    fn close_popovers(&mut self) -> bool {
        let Some(editor) = self.editor.as_mut() else {
            return false;
        };
        let open = editor.suggest_open
            || editor.sources.is_some()
            || editor.level_open
            || editor.budget_open;
        // Focus returns to whatever opened the layer.
        if editor.sources.is_some() {
            editor.focus = Some(Target::SourceOpen);
        } else if editor.level_open {
            editor.focus = Some(Target::LevelAdd);
        } else if editor.budget_open {
            editor.focus = Some(Target::BudgetAdd);
        }
        editor.suggest_open = false;
        editor.sources = None;
        editor.level_open = false;
        editor.budget_open = false;
        open
    }

    fn save_editor(&mut self, cx: &mut Context<Self>) {
        if self.editor.as_ref().is_none_or(|e| e.busy) {
            return;
        }
        self.close_popovers();
        if let Some(editor) = self.editor.as_mut() {
            editor.focus = None;
        }
        // A pending recognition must not arrive after the user saved.
        if let Some(editor) = self.editor.as_mut() {
            editor.recognize = None;
            if editor.state.mode == Mode::Add && !editor.state.settled {
                self.act(Edit::Settle { id: None }, cx);
            }
        }
        self.act(Edit::Save, cx);
    }

    fn persist(&mut self, input: crate::api::ModelInput, cx: &mut Context<Self>) {
        let Some(profile) = self
            .detail
            .as_ref()
            .and_then(|d| d["profile_id"].as_str())
            .map(str::to_owned)
        else {
            return;
        };
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        editor.busy = true;
        let adding = editor.state.mode == Mode::Add;
        self.watch_source(cx);
        let id = input.id.clone();
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let result = source.save_model(&profile, input).await;
            let _ = this.update(cx, |v, cx| {
                match result {
                    Ok(()) => {
                        v.editor = None;
                        v.show_notice(
                            format!("{} {id}", if adding { "已添加" } else { "已保存" }),
                            cx,
                        );
                    }
                    Err(error) => {
                        if let Some(editor) = v.editor.as_mut() {
                            editor.busy = false;
                            editor.error = Some(error.to_string());
                        }
                    }
                }
                v.editor_changed(cx);
            });
        })
        .detach();
    }

    fn remove_edited_model(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_mut().filter(|e| !e.busy) else {
            return;
        };
        let Some(model) = editor.state.original.clone() else {
            return;
        };
        let Some(profile) = self
            .detail
            .as_ref()
            .and_then(|d| d["profile_id"].as_str())
            .map(str::to_owned)
        else {
            return;
        };
        editor.busy = true;
        let id = model["id"].as_str().unwrap_or_default().to_owned();
        self.watch_source(cx);
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let result = source.remove_model(&profile, model).await;
            let _ = this.update(cx, |v, cx| {
                match result {
                    Ok(()) => {
                        v.editor = None;
                        v.show_notice(format!("已移除 {id}"), cx);
                    }
                    Err(error) => {
                        if let Some(editor) = v.editor.as_mut() {
                            editor.busy = false;
                            editor.error = Some(error.to_string());
                        }
                    }
                }
                v.editor_changed(cx);
            });
        })
        .detach();
        self.editor_changed(cx);
    }

    /// A short confirmation next to the 模型 heading, cleared after a while.
    pub(super) fn show_notice(&mut self, text: String, cx: &mut Context<Self>) {
        self.discovered = Some(json!({ "message": text }));
        self.notice_clear = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(5)).await;
            let _ = this.update(cx, |v, cx| {
                v.discovered = None;
                zork_ui::components::region::invalidate_all(cx);
                cx.notify();
            });
        }));
    }

    /// The provider's current list feeds the id suggestions; it is fetched
    /// once per page visit and never changes the connection.
    fn fetch_reported(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self
            .detail
            .as_ref()
            .and_then(|d| d["profile_id"].as_str())
            .map(str::to_owned)
        else {
            return;
        };
        if self.reported.contains_key(&id) {
            return;
        }
        self.reported.insert(id.clone(), vec![]);
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let Ok(ids) = source.available_models(&id).await else {
                return;
            };
            let _ = this.update(cx, |v, cx| {
                v.reported.insert(id, ids);
                v.refresh_editor(cx);
            });
        })
        .detach();
    }

    /// Focus and blur of the editor's fields drive recognition, quick picks
    /// and normalization. A field's focus handle changes once its editor is
    /// prepared, so the watch is renewed whenever a handle differs.
    /// Call at the start of every render while an editor is open, before any
    /// focus handle is read.
    pub(super) fn prepare_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.as_ref().is_some_and(|e| !e.prepared) {
            if let Some(editor) = self.editor.as_mut() {
                editor.prepared = true;
            }
            // Prepare every field's editor now, so focus handles are final
            // before anything focuses or watches them.
            for input in [
                &self.editor_inputs.id,
                &self.editor_inputs.context,
                &self.editor_inputs.output,
                &self.editor_inputs.level,
                &self.editor_inputs.budget,
                &self.editor_inputs.query,
            ] {
                input.update(cx, |input, cx| {
                    let selection = input.selection(cx);
                    input.select(selection, window, cx);
                });
            }
        }
    }

    /// Focus moves drive recognition (id blur), normalization (token field
    /// blur) and closing the inline add inputs. Focus is read during render,
    /// which also re-renders this view whenever it changes; transitions are
    /// applied right after the frame.
    pub(super) fn watch_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = [
            &self.editor_inputs.id,
            &self.editor_inputs.context,
            &self.editor_inputs.output,
            &self.editor_inputs.budget,
            &self.editor_inputs.level,
        ]
        .map(|input| input.read(cx).focus_handle().is_focused(window));
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let before = std::mem::replace(&mut editor.focused, now);
        let left = |i: usize| before[i] && !now[i];
        if !(0..now.len()).any(left) {
            return;
        }
        let (id, context, output, budget, level) = (left(0), left(1), left(2), left(3), left(4));
        cx.defer_in(window, move |v, _, cx| {
            if id && v.editor.as_ref().is_some_and(|e| e.state.mode == Mode::Add) {
                v.settle_id(None, cx);
            }
            if context {
                v.act(
                    Edit::BlurLength {
                        field: LengthField::Context,
                    },
                    cx,
                );
            }
            if output {
                v.act(
                    Edit::BlurLength {
                        field: LengthField::Output,
                    },
                    cx,
                );
            }
            if budget || level {
                if let Some(editor) = v.editor.as_mut() {
                    if budget {
                        editor.budget_open = false;
                    }
                    // Names are picked with the pointer without moving focus,
                    // so leaving the field means the user went elsewhere.
                    if level {
                        editor.level_open = false;
                    }
                }
                v.act(Edit::ClearAddErrors, cx);
            }
        });
    }
}

// ---------------------------------------------------------------- render

/// Level chips dragged to a new place.
#[derive(Clone)]
struct DraggedLevel {
    index: usize,
    name: String,
}
struct LevelGhost(String);
impl Render for LevelGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let p = ZORK_UI.palette;
        div()
            .h(px(28.))
            .px(px(12.))
            .flex()
            .items_center()
            .rounded_full()
            .bg(rgb(p.canvas))
            .border(px(zork_ui::design::BORDER_WIDTH))
            .border_color(rgb(ui::FIELD_FOCUS_BORDER()))
            .text_size(px(12.5))
            .font_family(zork_ui::assets::CODE_FONT_FAMILY)
            .child(self.0.clone())
    }
}

fn focus_of(id: &str, window: &mut Window, cx: &mut gpui::App) -> gpui::FocusHandle {
    zork_ui::components::widgets::controls::action_focus(
        gpui::SharedString::from(id.to_owned()),
        window,
        cx,
    )
}

/// A popover under its trigger that never takes focus from the field that
/// drives it. It stays inside the window and enters with a short rise.
fn popover(
    id: impl Into<gpui::SharedString>,
    width: f32,
    content: gpui::AnyElement,
) -> gpui::AnyElement {
    let id: gpui::SharedString = id.into();
    let p = ZORK_UI.palette;
    div()
        .w_full()
        .h(px(0.))
        .child(
            gpui::deferred(
                gpui::anchored().snap_to_window_with_margin(px(8.)).child(
                    div()
                        .id(id.clone())
                        .occlude()
                        .mt(px(6.))
                        .w(px(width))
                        .max_h(px(340.))
                        .overflow_y_scroll()
                        .p(px(6.))
                        .flex()
                        .flex_col()
                        .rounded(px(zork_ui::design::RADIUS.container))
                        .bg(rgb(p.canvas))
                        .border(px(zork_ui::design::BORDER_WIDTH))
                        .border_color(rgb(zork_ui::design::FORM.outline))
                        .shadow(vec![gpui::BoxShadow {
                            color: gpui::hsla(0., 0., 0., 0.14),
                            offset: gpui::point(px(0.), px(12.)),
                            blur_radius: px(32.),
                            spread_radius: px(0.),
                            inset: false,
                        }])
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(content)
                        .appear(
                            gpui::SharedString::from(format!("{id}-enter")),
                            zork_ui::motion::POPOVER,
                            zork_ui::motion::POPOVER_OFFSET,
                        ),
                ),
            )
            .with_priority(400),
        )
        .into_any_element()
}

fn group_title(text: String) -> gpui::Div {
    div()
        .h(px(28.))
        .px(px(12.))
        .flex()
        .items_center()
        .flex_shrink_0()
        .text_size(px(12.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(rgb(ZORK_UI.palette.muted))
        .child(text)
}

/// A small capsule action: ink-filled when `on` (with the primary hover), quiet
/// otherwise, so a selected pill never shows a light hover or focus fill.
fn pill(id: impl Into<gpui::ElementId>, on: bool, enabled: bool) -> ui::Action {
    zork_ui::components::widgets::controls::adaptive_action(
        id,
        "",
        ui::ActionStyle {
            primary: on,
            quiet: !on,
            icon_only: Some(false),
            disabled: !enabled,
            ..Default::default()
        },
        ZORK_UI.palette.canvas,
    )
    .min_h_0()
    .py_0()
    .gap(px(4.))
}

impl ProfilesView {
    fn apply_focus_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.editor.as_mut().and_then(|e| e.focus.take()) else {
            return;
        };
        let handle = match target {
            Target::Id => self.editor_inputs.id.read(cx).focus_handle(),
            Target::Context => self.editor_inputs.context.read(cx).focus_handle(),
            Target::Output => self.editor_inputs.output.read(cx).focus_handle(),
            Target::Query => self.editor_inputs.query.read(cx).focus_handle(),
            Target::Level => self.editor_inputs.level.read(cx).focus_handle(),
            Target::Budget => self.editor_inputs.budget.read(cx).focus_handle(),
            Target::Thinking => focus_of("model-kind-0", window, cx),
            Target::Api => focus_of("model-api-0", window, cx),
            Target::SourceOpen => focus_of("model-source-open", window, cx),
            Target::LevelAdd => focus_of("model-level-add", window, cx),
            Target::BudgetAdd => focus_of("model-budget-add", window, cx),
        };
        window.focus(&handle, cx);
        let section = match target {
            Target::Thinking => Some(Section::Thinking),
            Target::Context | Target::Output => Some(Section::Length),
            Target::Api => Some(Section::Api),
            _ => None,
        };
        if let (Some(section), Some(editor)) = (section, self.editor.as_ref()) {
            if let Some(anchor) = editor.anchors.get(&section) {
                anchor.scroll_to(window, cx);
            }
        }
    }

    /// The editor's content. `in_dialog` selects the surface it sits on: the
    /// dialog (open sections take the sunken fill) or the sunken inline panel
    /// (open sections take the surface fill).
    pub(super) fn editor_body(
        &mut self,
        in_dialog: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.watch_fields(window, cx);
        self.apply_focus_request(window, cx);
        let Some(editor) = self.editor.as_ref() else {
            return div().into_any_element();
        };
        let view = editor.view.clone();
        let add = view.mode == Mode::Add;
        let id_part = add.then(|| self.id_field(&view, window, cx));
        let status = self.status_line(&view, window, cx);
        let sections = self.sections(&view, in_dialog, window, cx);
        let content = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(16.))
            .children(id_part)
            .children(status)
            .children(sections);
        let body = div().id("model-editor").w_full().on_key_down(cx.listener(
            |v, event: &gpui::KeyDownEvent, _, cx| {
                // Escape closes the innermost open layer first.
                if event.keystroke.key == "escape" && v.close_popovers() {
                    v.act(Edit::ClearAddErrors, cx);
                    cx.stop_propagation();
                }
            },
        ));
        if !in_dialog {
            return body
                .child(content)
                .automation(AutomationRole::Status, "模型编辑器")
                .into_any_element();
        }
        // The dialog body scrolls here so a failed save can bring its first
        // error into view.
        let viewport = window.viewport_size().height.as_f32();
        let max = (viewport - 64.).clamp(2., 660.) - 132.;
        let scroll = self
            .editor
            .as_ref()
            .map(|e| e.scroll.clone())
            .unwrap_or_default();
        body.child(
            div()
                .id("model-editor-scroll")
                .w_full()
                .max_h(px(max.max(80.)))
                .overflow_y_scroll()
                .track_scroll(&scroll)
                .pb(px(4.))
                .child(content)
                .automation(AutomationRole::ScrollArea, "模型编辑器内容"),
        )
        .automation(AutomationRole::Status, "模型编辑器")
        .into_any_element()
    }

    fn id_field(
        &mut self,
        view: &View,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        let editor = self.editor.as_ref().unwrap();
        let invalid = view.id.error.is_some();
        let field = ui::input_control("profile-model", &self.editor_inputs.id, invalid, cx)
            .h(px(40.))
            .font_family(zork_ui::assets::CODE_FONT_FAMILY)
            .text_size(px(13.5))
            .automation_enabled(!editor.busy, AutomationRole::TextInput, "模型 ID");
        let suggestions = view
            .id
            .suggestions
            .clone()
            .filter(|s| editor.suggest_open && !s.flat().is_empty());
        let highlight = editor.suggest_hl;
        let popup = suggestions.map(|s| {
            let mut index = 0;
            let mut rows: Vec<gpui::AnyElement> = Vec::new();
            for group in &s.groups {
                rows.push(group_title(group.title.clone()).into_any_element());
                for item in &group.items {
                    let i = index;
                    index += 1;
                    let id = item.id.clone();
                    rows.push(
                        div()
                            .id(gpui::SharedString::from(format!("model-suggestion-{i}")))
                            .min_h(px(38.))
                            .px(px(12.))
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap(px(10.))
                            .rounded(px(18.))
                            .cursor_pointer()
                            .when(highlight == Some(i), |v| v.bg(rgb(p.selected)))
                            .when(highlight != Some(i), |v| {
                                v.hover(|s| s.bg(rgb(zork_ui::design::INTERACTION.neutral_hover)))
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(zork_ui::assets::CODE_FONT_FAMILY)
                                    .text_size(px(12.5))
                                    .child(item.id.clone()),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(12.))
                                    .text_color(rgb(p.muted))
                                    .child(item.name.clone()),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(12.))
                                    .text_color(rgb(p.muted))
                                    .child(item.meta.clone()),
                            )
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |v, _, _, cx| {
                                    cx.stop_propagation();
                                    v.settle_id(Some(id.clone()), cx);
                                }),
                            )
                            .automation(
                                AutomationRole::Option,
                                format!("{} {}", item.id, item.name),
                            )
                            .into_any_element(),
                    );
                }
            }
            if let Some(custom) = &s.custom {
                let i = index;
                let id = custom.id.clone();
                rows.push(
                    div()
                        .id("model-suggestion-custom")
                        .min_h(px(38.))
                        .px(px(12.))
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(8.))
                        .rounded(px(18.))
                        .cursor_pointer()
                        .text_size(px(13.))
                        .text_color(rgb(p.muted))
                        .when(highlight == Some(i), |v| v.bg(rgb(p.selected)))
                        .when(highlight != Some(i), |v| {
                            v.hover(|s| s.bg(rgb(zork_ui::design::INTERACTION.neutral_hover)))
                        })
                        .child(ui::icon("interface/plus.svg", 14.).text_color(rgb(p.muted)))
                        .child(div().min_w_0().truncate().child(custom.label.clone()))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |v, _, _, cx| {
                                cx.stop_propagation();
                                v.settle_id(Some(id.clone()), cx);
                            }),
                        )
                        .automation(AutomationRole::Option, custom.label.clone())
                        .into_any_element(),
                );
            }
            popover(
                "model-suggestions",
                ui::DIALOG_RICH_WIDTH - 48.,
                div().flex().flex_col().children(rows).into_any_element(),
            )
        });
        let error = view.id.error.clone().map(|message| {
            let duplicate = view.id.duplicate.clone();
            div()
                .id("profile-model-error")
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(12.))
                .text_color(rgb(p.danger))
                .child(message.clone())
                .when_some(duplicate, |v, duplicate| {
                    v.child(self.link(
                        "model-duplicate-edit",
                        "去编辑它",
                        p.danger,
                        window,
                        cx,
                        move |v, cx| {
                            v.editor = None;
                            v.open_edit_model(&duplicate, cx);
                        },
                    ))
                })
                .automation(AutomationRole::Status, message)
        });
        div()
            .id("model-id-box")
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.))
            .capture_action(cx.listener(|v, _: &FieldUp, _, cx| v.move_suggestion(false, cx)))
            .capture_action(cx.listener(|v, _: &FieldDown, _, cx| v.move_suggestion(true, cx)))
            .on_key_down(cx.listener(|v, event: &gpui::KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "escape" if v.editor.as_ref().is_some_and(|e| e.suggest_open) => {
                        if let Some(editor) = v.editor.as_mut() {
                            editor.suggest_open = false;
                            editor.suggest_hl = None;
                        }
                        v.editor_changed(cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .child(field)
            .children(popup)
            .children(error)
            .into_any_element()
    }

    /// An underlined inline text action.
    fn link(
        &self,
        id: &'static str,
        label: impl Into<gpui::SharedString>,
        color: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
        run: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> gpui::AnyElement {
        let label: gpui::SharedString = label.into();
        let enabled = self.editor.as_ref().is_none_or(|e| !e.busy);
        let focus = focus_of(id, window, cx);
        ui::quiet_button(id, "", enabled, ui::IconButtonSize::Small)
            .track_focus(&focus)
            .h(px(20.))
            .px(px(2.))
            .rounded(px(6.))
            .text_size(px(12.))
            .text_color(rgb(color))
            .child(div().underline().child(label.clone()))
            .on_click(cx.listener(move |v, _, _, cx| {
                cx.stop_propagation();
                run(v, cx)
            }))
            .automation_enabled(enabled, AutomationRole::Link, label.to_string())
            .into_any_element()
    }

    fn status_line(
        &mut self,
        view: &View,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let status = view.status.clone()?;
        let p = ZORK_UI.palette;
        let (icon, color) = match status.kind {
            "recognized" => ("interface/check.svg", p.success),
            "source" => ("interface/check.svg", p.success),
            _ if view.full => ("icons/attention.svg", p.warning),
            _ => ("icons/info.svg", p.subtle),
        };
        let sources = self.sources_popup(cx);
        let refill = status.refill.clone().map(|refill| {
            div()
                .id("model-refill")
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(4.))
                .text_size(px(12.))
                .text_color(rgb(p.muted))
                .child(refill.text.clone())
                .child("·")
                .child(self.link(
                    "model-refill-action",
                    refill.action.clone(),
                    p.text,
                    window,
                    cx,
                    |v, cx| v.act(Edit::Refill, cx),
                ))
                .automation(AutomationRole::Status, refill.text)
        });
        let action = self.link(
            "model-source-open",
            status.action,
            p.muted,
            window,
            cx,
            |v, cx| v.open_sources(cx),
        );
        let detail = div()
            .id("model-status-detail")
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(4.))
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(rgb(p.muted))
            .when(!status.detail.is_empty(), |v| {
                // “…可以 从相似模型填入” reads as one sentence; others separate.
                let joined = status.detail.ends_with("可以");
                v.child(status.detail.clone())
                    .when(!joined, |v| v.child("·"))
            })
            .child(action);
        Some(
            div()
                .id("model-status")
                .flex()
                .items_start()
                .gap(px(10.))
                .when(view.id.pending, |v| v.opacity(0.55))
                .child(
                    div()
                        .pt(px(2.))
                        .child(ui::icon(icon, 16.).text_color(rgb(color))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .when(!status.title.is_empty(), |v| {
                            v.child(
                                div()
                                    .text_size(px(13.))
                                    .line_height(px(20.))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child(status.title.clone()),
                            )
                        })
                        .child(detail)
                        .children(refill)
                        .children(sources),
                )
                .automation(
                    AutomationRole::Status,
                    format!("{} {}", status.title, status.detail),
                )
                .into_any_element(),
        )
    }

    fn sources_popup(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let p = ZORK_UI.palette;
        let editor = self.editor.as_ref()?;
        let sources = editor.sources.as_ref()?;
        let highlight = sources.highlight;
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        let search = ui::input_control("model-source-query", &self.editor_inputs.query, false, cx)
            .h(px(34.))
            .child(ui::icon("interface/search.svg", 14.).text_color(rgb(p.subtle)))
            .automation(AutomationRole::TextInput, "搜索模型或预设");
        let mut index = 0;
        for group in &sources.groups {
            rows.push(group_title(group.title.clone()).into_any_element());
            for item in &group.items {
                let i = index;
                index += 1;
                let reference = item.reference.clone();
                rows.push(
                    div()
                        .id(gpui::SharedString::from(format!("model-source-{i}")))
                        .min_h(px(38.))
                        .px(px(12.))
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(10.))
                        .rounded(px(18.))
                        .cursor_pointer()
                        .text_size(px(13.))
                        .when(highlight == i, |v| v.bg(rgb(p.selected)))
                        .when(highlight != i, |v| {
                            v.hover(|s| s.bg(rgb(zork_ui::design::INTERACTION.neutral_hover)))
                        })
                        .child(div().min_w_0().truncate().child(item.label.clone()))
                        .when(!item.sub.is_empty(), |v| {
                            v.child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(12.))
                                    .text_color(rgb(p.muted))
                                    .child(item.sub.clone()),
                            )
                        })
                        .child(div().flex_1())
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(12.))
                                .text_color(rgb(p.muted))
                                .child(item.meta.clone()),
                        )
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |v, _, _, cx| {
                                cx.stop_propagation();
                                v.use_source(reference.clone(), cx);
                            }),
                        )
                        .automation(
                            AutomationRole::Option,
                            format!("{} {}", item.label, item.meta),
                        )
                        .into_any_element(),
                );
            }
        }
        if index == 0 {
            rows.push(group_title("没有匹配的模型".into()).into_any_element());
        }
        let content = div()
            .id("model-source-box")
            .flex()
            .flex_col()
            .capture_action(cx.listener(|v, _: &FieldUp, _, cx| v.move_source(false, cx)))
            .capture_action(cx.listener(|v, _: &FieldDown, _, cx| v.move_source(true, cx)))
            .on_key_down(cx.listener(|v, event: &gpui::KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => {
                        if let Some(editor) = v.editor.as_mut() {
                            editor.sources = None;
                            editor.focus = Some(Target::SourceOpen);
                        }
                        v.editor_changed(cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .child(div().pb(px(4.)).child(search))
            .children(rows)
            .into_any_element();
        Some(popover("model-sources", 380., content))
    }

    fn move_source(&mut self, down: bool, cx: &mut Context<Self>) {
        let Some(sources) = self.editor.as_mut().and_then(|e| e.sources.as_mut()) else {
            return;
        };
        let count = sources.items().len();
        if count == 0 {
            return;
        }
        sources.highlight = if down {
            (sources.highlight + 1) % count
        } else {
            (sources.highlight + count - 1) % count
        };
        self.editor_changed(cx);
    }
}

impl ProfilesView {
    fn sections(
        &mut self,
        view: &View,
        in_dialog: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let sections = view.sections.clone();
        sections
            .into_iter()
            .map(|section| self.section(view, section, in_dialog, window, cx))
            .collect()
    }

    fn section(
        &mut self,
        view: &View,
        section: SectionView,
        in_dialog: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        let key = match section.section {
            Section::Thinking => "thinking",
            Section::Length => "length",
            Section::Image => "image",
            Section::Api => "api",
        };
        let full = view.full;
        let open = section.open;
        let fill = if in_dialog { p.prompt } else { p.canvas };
        let busy = self.editor.as_ref().is_some_and(|e| e.busy);
        let provenance = section.provenance.clone().map(|prov| {
            let target = section.section;
            div()
                .id(gpui::SharedString::from(format!("model-provenance-{key}")))
                .flex_shrink(1.)
                .min_w_0()
                .max_w(px(200.))
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .id(gpui::SharedString::from(format!(
                            "model-provenance-text-{key}"
                        )))
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(rgb(p.muted))
                        .child(prov.text.clone())
                        .automation(AutomationRole::Status, prov.detail.clone()),
                )
                .child(
                    ui::quiet_button(
                        gpui::SharedString::from(format!("model-restore-{key}")),
                        "",
                        !busy,
                        ui::IconButtonSize::Small,
                    )
                    .flex_shrink_0()
                    .px(px(8.))
                    .gap(px(4.))
                    .child(ui::icon("interface/reload.svg", 12.).text_color(rgb(p.muted)))
                    .child(div().text_size(px(12.)).child("恢复"))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        // Restoring never toggles the row underneath.
                        cx.stop_propagation();
                        v.act(Edit::Restore { section: target }, cx);
                    }))
                    .automation_enabled(!busy, AutomationRole::Button, "恢复"),
                )
        });
        let header = if full {
            div()
                .id(gpui::SharedString::from(format!("model-question-{key}")))
                .min_h(px(28.))
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .size(px(18.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(rgb(p.text))
                        .text_color(rgb(p.canvas))
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(section.number.to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(13.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(section.question),
                )
                .children(provenance)
                .automation(AutomationRole::Status, section.question)
                .into_any_element()
        } else {
            let target = section.section;
            let chevron_id = format!("model-section-{key}");
            // The whole row toggles; its visible parts sit above the hit
            // target, and 恢复 is a sibling button, not nested inside it.
            div()
                .relative()
                .min_h(px(40.))
                .w_full()
                .child(
                    ui::quiet_button(
                        gpui::SharedString::from(format!("model-section-{key}")),
                        "",
                        !busy,
                        ui::IconButtonSize::Standard,
                    )
                    .absolute()
                    .inset_0()
                    .h_full()
                    .w_full()
                    .rounded(px(zork_ui::design::RADIUS.block))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.act(Edit::ToggleSection { section: target }, cx)
                    }))
                    .automation_enabled(
                        !busy,
                        AutomationRole::Button,
                        format!("{} {}", section.label, section.summary),
                    ),
                )
                .child(
                    div()
                        .relative()
                        .min_h(px(40.))
                        .pl(px(12.))
                        .pr(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            div()
                                .w(px(LABEL_WIDTH))
                                .flex_shrink_0()
                                .text_size(px(12.5))
                                .text_color(rgb(p.muted))
                                .child(section.label),
                        )
                        .child(
                            div()
                                .id(gpui::SharedString::from(format!("model-summary-{key}")))
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(13.))
                                .text_color(rgb(if section.muted { p.muted } else { p.text }))
                                .child(section.summary.clone())
                                .automation(AutomationRole::Status, section.summary.clone()),
                        )
                        .when(section.error && !open, |v| {
                            v.child(
                                div()
                                    .size(px(6.))
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .bg(rgb(p.danger)),
                            )
                        })
                        .children(provenance)
                        .child(zork_ui::components::disclosure::chevron(
                            chevron_id, open, p.muted,
                        )),
                )
                .into_any_element()
        };
        let body_content = match section.section {
            Section::Thinking => self.thinking_body(view, window, cx),
            Section::Length => self.length_body(view, window, cx),
            Section::Image => self.image_body(view, window, cx),
            Section::Api => self.api_body(view, window, cx),
        };
        let indent = if full { 26. } else { SUMMARY_INDENT };
        let fold =
            zork_ui::motion::fold(format!("model-section-body-{key}"), open, open, window, cx);
        let body = fold.map(|frame| {
            frame.wrap(
                div()
                    .w_full()
                    .pl(px(indent))
                    .pr(px(if full { 0. } else { 12. }))
                    .pt(px(if full { 8. } else { 2. }))
                    .pb(px(if full { 4. } else { 14. }))
                    .child(body_content),
            )
        });
        let anchor = self
            .editor
            .as_ref()
            .and_then(|e| e.anchors.get(&section.section).cloned());
        div()
            .id(gpui::SharedString::from(format!("model-section-box-{key}")))
            .w_full()
            .flex()
            .flex_col()
            .rounded(px(zork_ui::design::RADIUS.block))
            .when(open && !full, |v| v.bg(rgb(fill)))
            .anchor_scroll(anchor)
            .child(header)
            .children(body)
            .into_any_element()
    }

    /// A compact segmented control for short option sets.
    fn segmented(
        &self,
        id: &'static str,
        options: Vec<String>,
        selected: Option<usize>,
        cx: &mut Context<Self>,
        pick: impl Fn(&mut Self, usize, &mut Context<Self>) + 'static,
    ) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        let enabled = self.editor.as_ref().is_none_or(|e| !e.busy);
        let pick = std::rc::Rc::new(pick);
        let items: Vec<_> = options
            .into_iter()
            .enumerate()
            .map(|(i, label)| {
                let on = selected == Some(i);
                let pick = pick.clone();
                pill(gpui::SharedString::from(format!("{id}-{i}")), on, enabled)
                    .px(px(10.))
                    .h(px(24.))
                    .text_size(px(12.5))
                    .child(
                        div()
                            .text_color(rgb(if on { p.canvas } else { p.muted }))
                            .child(label.clone()),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| pick(view, i, cx)))
                    .automation_enabled(enabled, AutomationRole::Option, label)
            })
            .collect();
        div()
            .id(id)
            .h(px(30.))
            .p(px(3.))
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(2.))
            .rounded_full()
            .border(px(zork_ui::design::BORDER_WIDTH))
            .border_color(rgb(zork_ui::design::FORM.outline))
            .children(items)
            .into_any_element()
    }

    /// A radio row: the mark, a title and an example line.
    fn radio_row(
        &self,
        id: String,
        title: String,
        detail: String,
        selected: bool,
        cx: &mut Context<Self>,
        window: &mut Window,
        run: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        let enabled = self.editor.as_ref().is_none_or(|e| !e.busy);
        let focus = focus_of(&id, window, cx);
        let mark = div()
            .mt(px(2.))
            .size(px(16.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .border(px(if selected { 5. } else { 1.5 }))
            .border_color(rgb(if selected { p.text } else { p.subtle }))
            .bg(rgb(p.canvas));
        ui::quiet_button(
            gpui::SharedString::from(id.clone()),
            "",
            enabled,
            ui::IconButtonSize::Standard,
        )
        .track_focus(&focus)
        .w_full()
        .h_auto()
        .min_h(px(44.))
        .py(px(6.))
        .px(px(10.))
        .justify_start()
        .items_start()
        .whitespace_normal()
        .gap(px(10.))
        .rounded(px(zork_ui::design::RADIUS.block))
        .child(mark)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(rgb(p.text))
                        .child(title.clone()),
                )
                .when(!detail.is_empty(), |v| {
                    v.child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .font_weight(gpui::FontWeight::NORMAL)
                            .text_color(rgb(p.muted))
                            .child(detail.clone()),
                    )
                }),
        )
        .on_click(cx.listener(move |v, _, _, cx| run(v, cx)))
        .automation_enabled(
            enabled,
            AutomationRole::Option,
            format!("{title}{}", if selected { "（已选）" } else { "" }),
        )
        .into_any_element()
    }

    fn thinking_body(
        &mut self,
        view: &View,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        let thinking = view.thinking.clone();
        let first = focus_of("model-kind-0", window, cx);
        if let Some(editor) = self.editor.as_mut() {
            editor.kind_focus = Some(first);
        }
        let mut list: Vec<gpui::AnyElement> = Vec::new();
        for (index, kind) in thinking.kinds.iter().enumerate() {
            let value = kind.kind;
            list.push(self.radio_row(
                format!("model-kind-{index}"),
                kind.title.into(),
                kind.example.into(),
                kind.selected,
                cx,
                window,
                move |v, cx| v.act(Edit::SetKind { kind: value }, cx),
            ));
            if kind.selected {
                list.push(
                    div()
                        .pl(px(36.))
                        .pr(px(4.))
                        .pb(px(8.))
                        .child(self.kind_editor(view, window, cx))
                        .into_any_element(),
                );
            }
        }
        let panel = thinking.panel.clone().map(|panel| {
            let preview: gpui::AnyElement = match &panel.panel {
                Panel::Options { options, selected } => div()
                    .h(px(26.))
                    .p(px(2.))
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .rounded_full()
                    .bg(rgb(p.prompt))
                    .children(options.iter().enumerate().map(|(i, o)| {
                        div()
                            .h(px(22.))
                            .px(px(9.))
                            .flex()
                            .items_center()
                            .rounded_full()
                            .text_size(px(12.))
                            .text_color(rgb(if i == *selected { p.canvas } else { p.muted }))
                            .when(i == *selected, |v| v.bg(rgb(p.text)))
                            .child(o.clone())
                    }))
                    .into_any_element(),
                _ => div()
                    .text_size(px(12.5))
                    .text_color(rgb(p.text))
                    .child(panel.text.unwrap_or_default())
                    .into_any_element(),
            };
            let tip = thinking.request_field.clone();
            let mark = div()
                .id("model-request-field")
                .size(px(16.))
                .flex_shrink_0()
                .child(ui::icon("icons/info.svg", 14.).text_color(rgb(p.subtle)))
                .automation(AutomationRole::Status, tip.clone());
            div()
                .id("model-panel-preview")
                .min_h(px(32.))
                .pl(px(10.))
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(12.))
                        .text_color(rgb(p.muted))
                        .child("模型面板里会显示"),
                )
                .child(preview)
                .child(div().flex_1())
                .child(zork_ui::components::tooltip::hint(
                    mark,
                    "model-request-field",
                    tip,
                ))
                .automation(AutomationRole::Status, "模型面板预览")
        });
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(list)
            .when_some(thinking.error.clone(), |v, error| {
                v.child(
                    div()
                        .id("model-thinking-error")
                        .pl(px(10.))
                        .pt(px(4.))
                        .text_size(px(12.))
                        .text_color(rgb(p.danger))
                        .child(error.clone())
                        .automation(AutomationRole::Status, error),
                )
            })
            .children(panel.map(|v| div().pt(px(6.)).child(v)))
            .into_any_element()
    }

    /// The editor for the chosen kind, under its radio row.
    fn kind_editor(
        &mut self,
        view: &View,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        let thinking = &view.thinking;
        let help = |text: String| {
            div()
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(rgb(p.muted))
                .child(text)
        };
        if let Some(toggle) = &thinking.toggle {
            return div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(rgb(p.muted))
                        .child("默认"),
                )
                .child(self.segmented(
                    "model-toggle-default",
                    vec!["关".into(), "开".into()],
                    Some(usize::from(toggle.default_on)),
                    cx,
                    |v, i, cx| v.act(Edit::ToggleDefault { on: i == 1 }, cx),
                ))
                .into_any_element();
        }
        if let Some(levels) = thinking.levels.clone() {
            return div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(self.level_chips(&levels, window, cx))
                .child(help(levels.help.clone()))
                .into_any_element();
        }
        if let Some(budget) = thinking.budget.clone() {
            return self.budget_editor(&budget, window, cx);
        }
        thinking
            .note
            .map(|note| help(note.into()).into_any_element())
            .unwrap_or_else(|| div().into_any_element())
    }

    /// A capsule chip with its own select and remove buttons (siblings, so
    /// nothing interactive nests).
    fn chip(
        &self,
        key: String,
        label: String,
        default: bool,
        bad: bool,
        removable: bool,
        cx: &mut Context<Self>,
        select: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        remove: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let p = ZORK_UI.palette;
        let enabled = self.editor.as_ref().is_none_or(|e| !e.busy);
        let ink = if default {
            p.canvas
        } else if bad {
            p.danger
        } else {
            p.text
        };
        div()
            .id(gpui::SharedString::from(format!("{key}-chip")))
            .h(px(28.))
            .flex()
            .flex_shrink_0()
            .items_center()
            .rounded_full()
            .border(px(zork_ui::design::BORDER_WIDTH))
            .border_color(rgb(if bad {
                p.danger
            } else if default {
                p.text
            } else {
                zork_ui::design::FORM.outline
            }))
            .when(default, |v| v.bg(rgb(p.text)))
            .child(
                pill(gpui::SharedString::from(key.clone()), default, enabled)
                    .h(px(26.))
                    .pl(px(11.))
                    .pr(px(if removable { 4. } else { 11. }))
                    .text_size(px(12.5))
                    .font_family(zork_ui::assets::CODE_FONT_FAMILY)
                    .child(div().text_color(rgb(ink)).child(label.clone()))
                    .on_click(cx.listener(move |v, _, _, cx| select(v, cx)))
                    .automation_enabled(
                        enabled,
                        AutomationRole::Option,
                        format!("{label}{}", if default { "（默认）" } else { "" }),
                    ),
            )
            .when(removable, |v| {
                v.child(
                    pill(
                        gpui::SharedString::from(format!("{key}-remove")),
                        default,
                        enabled,
                    )
                    .size(px(22.))
                    .mr(px(3.))
                    .px_0()
                    .child(ui::icon("interface/x.svg", 12.).text_color(rgb(if default {
                        p.canvas
                    } else {
                        p.muted
                    })))
                    .on_click(cx.listener(move |v, _, _, cx| remove(v, cx)))
                    .automation_enabled(
                        enabled,
                        AutomationRole::Button,
                        format!("删除 {label}"),
                    ),
                )
            })
    }

    fn level_chips(
        &mut self,
        levels: &crate::api::model_editor::LevelsView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        let mut chips: Vec<gpui::AnyElement> = Vec::new();
        for (index, level) in levels.values.iter().enumerate() {
            let name = level.name.clone();
            let dragged = DraggedLevel {
                index,
                name: name.clone(),
            };
            chips.push(
                self.chip(
                    format!("model-level-{index}"),
                    level.name.clone(),
                    level.default,
                    false,
                    levels.can_delete,
                    cx,
                    move |v, cx| v.act(Edit::LevelDefault { name: name.clone() }, cx),
                    move |v, cx| v.act(Edit::LevelDelete { index }, cx),
                )
                .cursor_grab()
                .on_drag(dragged, |dragged, _, _, cx| {
                    let name = dragged.name.clone();
                    cx.new(|_| LevelGhost(name))
                })
                .drag_over::<DraggedLevel>(|style, _, _, _| {
                    style.border_color(rgb(ui::FIELD_FOCUS_BORDER()))
                })
                .on_drop(cx.listener(move |v, dragged: &DraggedLevel, _, cx| {
                    v.act(
                        Edit::LevelMove {
                            from: dragged.index,
                            to: index,
                        },
                        cx,
                    )
                }))
                .automation(AutomationRole::Status, format!("档位 {}", level.name))
                .into_any_element(),
            );
        }
        let open = self.editor.as_ref().is_some_and(|e| e.level_open);
        let enabled = self.editor.as_ref().is_none_or(|e| !e.busy);
        let add_focus = focus_of("model-level-add", window, cx);
        let add = ui::quiet_button("model-level-add", "", enabled, ui::IconButtonSize::Small)
            .track_focus(&add_focus)
            .h(px(28.))
            .px(px(11.))
            .rounded_full()
            .border(px(zork_ui::design::BORDER_WIDTH))
            .border_color(rgb(zork_ui::design::FORM.outline))
            .text_size(px(12.5))
            .child("+ 档位")
            .on_click(cx.listener(|v, _, _, cx| {
                if let Some(editor) = v.editor.as_mut() {
                    editor.level_open = !editor.level_open;
                    editor.level_hl = None;
                    if editor.level_open {
                        editor.focus = Some(Target::Level);
                    }
                }
                v.editor_inputs
                    .level
                    .update(cx, |input, cx| input.reset_value(String::new(), cx));
                v.act(Edit::ClearAddErrors, cx);
            }))
            .automation_enabled(enabled, AutomationRole::Button, "+ 档位");
        let popup = open.then(|| {
            let highlight = self.editor.as_ref().and_then(|e| e.level_hl);
            let rows: Vec<gpui::AnyElement> = levels
                .addable
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let name = (*name).to_owned();
                    div()
                        .id(gpui::SharedString::from(format!("model-level-pick-{i}")))
                        .h(px(34.))
                        .px(px(12.))
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .rounded(px(17.))
                        .cursor_pointer()
                        .font_family(zork_ui::assets::CODE_FONT_FAMILY)
                        .text_size(px(12.5))
                        .when(highlight == Some(i), |v| v.bg(rgb(p.selected)))
                        .when(highlight != Some(i), |v| {
                            v.hover(|s| s.bg(rgb(zork_ui::design::INTERACTION.neutral_hover)))
                        })
                        .child(name.clone())
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |v, _, _, cx| {
                                cx.stop_propagation();
                                v.add_level(name.clone(), cx);
                            }),
                        )
                        .automation(
                            AutomationRole::Option,
                            (*levels.addable.get(i).unwrap()).to_owned(),
                        )
                        .into_any_element()
                })
                .collect();
            let error = levels.add_error.clone();
            let invalid = error.is_some();
            let content = div()
                .id("model-level-box")
                .flex()
                .flex_col()
                .gap(px(2.))
                .capture_action(cx.listener(|v, _: &FieldUp, _, cx| v.move_level_pick(false, cx)))
                .capture_action(cx.listener(|v, _: &FieldDown, _, cx| v.move_level_pick(true, cx)))
                .on_key_down(cx.listener(|v, event: &gpui::KeyDownEvent, _, cx| {
                    match event.keystroke.key.as_str() {
                        "escape" => {
                            if let Some(editor) = v.editor.as_mut() {
                                editor.level_open = false;
                                editor.focus = Some(Target::LevelAdd);
                            }
                            v.act(Edit::ClearAddErrors, cx);
                            cx.stop_propagation();
                        }
                        _ => {}
                    }
                }))
                .children(rows)
                .child(
                    div().pt(px(4.)).child(
                        ui::input_control(
                            "model-level-name",
                            &self.editor_inputs.level,
                            invalid,
                            cx,
                        )
                        .h(px(32.))
                        .font_family(zork_ui::assets::CODE_FONT_FAMILY)
                        .automation(AutomationRole::TextInput, "其他档位名称"),
                    ),
                )
                .when_some(error, |v, error| {
                    v.child(
                        div()
                            .id("model-level-error")
                            .px(px(8.))
                            .pt(px(4.))
                            .text_size(px(12.))
                            .text_color(rgb(p.danger))
                            .child(error.clone())
                            .automation(AutomationRole::Status, error),
                    )
                })
                .into_any_element();
            popover("model-level-popover", 240., content)
        });
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .children(chips)
                    .child(add),
            )
            .children(popup)
            .into_any_element()
    }

    fn move_level_pick(&mut self, down: bool, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let count = editor
            .view
            .thinking
            .levels
            .as_ref()
            .map(|l| l.addable.len())
            .unwrap_or(0);
        if count == 0 {
            return;
        }
        editor.level_hl = Some(match (editor.level_hl, down) {
            (None, true) => 0,
            (None, false) => count - 1,
            (Some(i), true) => (i + 1) % count,
            (Some(i), false) => (i + count - 1) % count,
        });
        self.editor_changed(cx);
    }

    fn budget_editor(
        &mut self,
        budget: &crate::api::model_editor::BudgetView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        let enabled = self.editor.as_ref().is_none_or(|e| !e.busy);
        let adding = self.editor.as_ref().is_some_and(|e| e.budget_open);
        let mut chips: Vec<gpui::AnyElement> = budget
            .presets
            .iter()
            .enumerate()
            .map(|(index, chip)| {
                let tokens = chip.tokens;
                self.chip(
                    format!("model-budget-{index}"),
                    chip.label.clone(),
                    chip.default,
                    chip.bad,
                    true,
                    cx,
                    move |v, cx| {
                        v.act(
                            Edit::BudgetDefault {
                                choice: BudgetChoice::Tokens(tokens),
                            },
                            cx,
                        )
                    },
                    move |v, cx| v.act(Edit::BudgetDelete { index }, cx),
                )
                .into_any_element()
            })
            .collect();
        let error = budget.add_error.clone();
        if adding {
            chips.push(
                div()
                    .id("model-budget-add-box")
                    .w(px(120.))
                    .on_key_down(cx.listener(|v, event: &gpui::KeyDownEvent, _, cx| {
                        match event.keystroke.key.as_str() {
                            "escape" => {
                                if let Some(editor) = v.editor.as_mut() {
                                    editor.budget_open = false;
                                    editor.focus = Some(Target::BudgetAdd);
                                }
                                v.act(Edit::ClearAddErrors, cx);
                                cx.stop_propagation();
                            }
                            _ => {}
                        }
                    }))
                    .child(
                        ui::input_control(
                            "model-budget-input",
                            &self.editor_inputs.budget,
                            error.is_some(),
                            cx,
                        )
                        .h(px(28.))
                        .automation(AutomationRole::TextInput, "新预算"),
                    )
                    .into_any_element(),
            );
        } else {
            let add_focus = focus_of("model-budget-add", window, cx);
            chips.push(
                ui::quiet_button("model-budget-add", "", enabled, ui::IconButtonSize::Small)
                    .track_focus(&add_focus)
                    .h(px(28.))
                    .px(px(11.))
                    .rounded_full()
                    .border(px(zork_ui::design::BORDER_WIDTH))
                    .border_color(rgb(zork_ui::design::FORM.outline))
                    .text_size(px(12.5))
                    .child("+ 预算")
                    .on_click(cx.listener(|v, _, _, cx| {
                        if let Some(editor) = v.editor.as_mut() {
                            editor.budget_open = true;
                            editor.focus = Some(Target::Budget);
                        }
                        v.editor_inputs
                            .budget
                            .update(cx, |input, cx| input.reset_value(String::new(), cx));
                        v.editor_changed(cx);
                    }))
                    .automation_enabled(enabled, AutomationRole::Button, "+ 预算")
                    .into_any_element(),
            );
        }
        let switch = |id: &'static str,
                      label: &'static str,
                      on: bool,
                      help: Option<&'static str>,
                      window: &mut Window,
                      cx: &mut Context<Self>,
                      set: fn(bool) -> Edit| {
            let focus = focus_of(id, window, cx);
            div()
                .min_h(px(32.))
                .flex()
                .items_center()
                .gap(px(10.))
                .child(ui::switch(
                    id,
                    label,
                    on,
                    enabled,
                    &focus,
                    cx,
                    move |v: &mut Self, value, cx| v.act(set(value), cx),
                ))
                .child(div().text_size(px(13.)).child(label))
                .when_some(help, |v, help| {
                    v.child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(p.muted))
                            .child(help),
                    )
                })
                .into_any_element()
        };
        let allow_off = switch(
            "model-budget-off",
            "可以关闭思考",
            budget.allow_off,
            None,
            window,
            cx,
            |on| Edit::BudgetAllowOff { on },
        );
        let dynamic = switch(
            "model-budget-dynamic",
            "可以让模型自己决定",
            budget.dynamic,
            Some(budget.dynamic_help),
            window,
            cx,
            |on| Edit::BudgetDynamic { on },
        );
        let choices = budget.defaults.clone();
        let selected = choices.iter().position(|c| c.selected);
        let labels = choices.iter().map(|c| c.label.clone()).collect();
        let default = (!choices.is_empty()).then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(rgb(p.muted))
                        .child("默认"),
                )
                .child(self.segmented(
                    "model-budget-default",
                    labels,
                    selected,
                    cx,
                    move |v, i, cx| {
                        if let Some(choice) = choices.get(i) {
                            v.act(
                                Edit::BudgetDefault {
                                    choice: choice.choice,
                                },
                                cx,
                            )
                        }
                    },
                ))
        });
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .children(chips),
            )
            .when_some(error, |v, error| {
                v.child(
                    div()
                        .id("model-budget-error")
                        .text_size(px(12.))
                        .text_color(rgb(p.danger))
                        .child(error.clone())
                        .automation(AutomationRole::Status, error),
                )
            })
            .child(allow_off)
            .child(dynamic)
            .children(default)
            .into_any_element()
    }

    fn token_field(
        &mut self,
        field: LengthField,
        view: &TokenFieldView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        let (id, input) = match field {
            LengthField::Context => ("profile-context-limit", self.editor_inputs.context.clone()),
            LengthField::Output => ("profile-output-limit", self.editor_inputs.output.clone()),
        };
        // Read focus directly: the quick picks show exactly while the field
        // has focus, whatever event ordering delivered it.
        let focused = input.read(cx).focus_handle().is_focused(window);
        let editor_input = input.clone();
        let control = ui::input_control(id, &input, view.error.is_some(), cx)
            .editor_slot(div().flex_1().min_w_0().child(editor_input))
            .when_some(view.exact.clone(), |v, exact| {
                v.child(
                    div()
                        .flex_shrink_0()
                        .pl(px(8.))
                        .text_size(px(12.))
                        .text_color(rgb(p.subtle))
                        .child(exact),
                )
            })
            .automation(AutomationRole::TextInput, view.label);
        let picks = focused.then(|| {
            div()
                .id(gpui::SharedString::from(format!("{id}-picks")))
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .children(view.picks.iter().enumerate().map(|(i, pick)| {
                    let value = pick.value;
                    div()
                        .id(gpui::SharedString::from(format!("{id}-pick-{i}")))
                        .h(px(24.))
                        .px(px(9.))
                        .flex()
                        .items_center()
                        .rounded_full()
                        .text_size(px(12.))
                        .when(pick.selected, |v| {
                            v.bg(rgb(p.text)).text_color(rgb(p.canvas))
                        })
                        .when(!pick.selected && !pick.disabled, |v| {
                            v.text_color(rgb(p.muted))
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(zork_ui::design::INTERACTION.neutral_hover)))
                        })
                        .when(pick.disabled, |v| {
                            v.text_color(rgb(zork_ui::design::FORM.disabled_text))
                        })
                        .child(pick.label.clone())
                        .when(!pick.disabled, |v| {
                            // Picking keeps focus (and the picks) in the field.
                            v.on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |v, _, _, cx| {
                                    cx.stop_propagation();
                                    v.act(Edit::PickLength { field, value }, cx);
                                }),
                            )
                        })
                        .automation_enabled(
                            !pick.disabled,
                            AutomationRole::Option,
                            pick.label.clone(),
                        )
                }))
                .automation(AutomationRole::Status, "常用值")
        });
        div()
            .id(gpui::SharedString::from(format!("{id}-box")))
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(6.))
            .capture_action(cx.listener(move |v, _: &FieldUp, _, cx| {
                v.act(Edit::StepLength { field, up: true }, cx)
            }))
            .capture_action(cx.listener(move |v, _: &FieldDown, _, cx| {
                v.act(Edit::StepLength { field, up: false }, cx)
            }))
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(rgb(p.muted))
                    .child(view.label),
            )
            .child(control)
            .children(picks)
            .when_some(view.error.clone(), |v, error| {
                v.child(
                    div()
                        .id(gpui::SharedString::from(format!("{id}-error")))
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(rgb(p.danger))
                        .child(error.clone())
                        .automation(AutomationRole::Status, error),
                )
            })
            .into_any_element()
    }

    fn length_body(
        &mut self,
        view: &View,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        let length = view.length.clone();
        let context = self.token_field(LengthField::Context, &length.context, window, cx);
        let output = self.token_field(LengthField::Output, &length.output, window, cx);
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(12.))
                    .child(context)
                    .child(output),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(rgb(p.muted))
                    .child(length.help),
            )
            .into_any_element()
    }

    fn image_body(
        &mut self,
        view: &View,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        let enabled = self.editor.as_ref().is_none_or(|e| !e.busy);
        let on = view.image.on;
        let focus = focus_of("model-image", window, cx);
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .min_h(px(32.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(ui::switch(
                        "model-image",
                        view.image.label,
                        on,
                        enabled,
                        &focus,
                        cx,
                        |v, value, cx| v.act(Edit::SetImage { on: value }, cx),
                    ))
                    .child(div().text_size(px(13.)).child(view.image.label)),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(rgb(p.muted))
                    .child(view.image.help),
            )
            .into_any_element()
    }

    fn api_body(
        &mut self,
        view: &View,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        let Some(api) = view.api.clone() else {
            return div().into_any_element();
        };
        let first = focus_of("model-api-0", window, cx);
        if let Some(editor) = self.editor.as_mut() {
            editor.api_focus = Some(first);
        }
        let rows: Vec<gpui::AnyElement> = api
            .options
            .iter()
            .enumerate()
            .map(|(i, option)| {
                let value = option.api.to_owned();
                self.radio_row(
                    format!("model-api-{i}"),
                    format!(
                        "{}{}",
                        option.title,
                        if option.default {
                            " · 连接默认"
                        } else {
                            ""
                        }
                    ),
                    option.description.into(),
                    option.selected,
                    cx,
                    window,
                    move |v, cx| v.act(Edit::SetApi { api: value.clone() }, cx),
                )
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(rows)
            .when_some(api.warning, |v, warning| {
                v.child(
                    div()
                        .id("model-api-warning")
                        .pl(px(10.))
                        .pt(px(4.))
                        .text_size(px(12.))
                        .text_color(rgb(p.warning))
                        .child(warning.clone())
                        .automation(AutomationRole::Status, warning),
                )
            })
            .into_any_element()
    }

    /// 取消 / 保存 (and 移除 when editing).
    pub(super) fn editor_footer(
        &mut self,
        in_dialog: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        let Some(editor) = self.editor.as_ref() else {
            return div().into_any_element();
        };
        let busy = editor.busy;
        let view = &editor.view;
        let remove = view.footer.remove.then(|| {
            ui::quiet_button(
                "profile-model-remove",
                "",
                !busy,
                ui::IconButtonSize::Compact,
            )
            .child(div().text_color(rgb(p.danger)).child("移除"))
            .on_click(cx.listener(|v, _, _, cx| v.remove_edited_model(cx)))
            .automation_enabled(!busy, AutomationRole::Button, "移除模型")
        });
        let save = view.footer.save;
        let _ = in_dialog;
        div()
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .children(remove)
            .child(div().flex_1())
            .child(
                ui::button("profile-model-cancel", "取消", false, !busy)
                    .on_click(cx.listener(|v, _, _, cx| v.close_editor(cx)))
                    .automation_enabled(!busy, AutomationRole::Button, "取消编辑模型"),
            )
            .child(
                ui::busy_button("profile-model-save", save, true, !busy, busy)
                    .on_click(cx.listener(|v, _, _, cx| v.save_editor(cx)))
                    .automation_enabled(!busy, AutomationRole::Button, save),
            )
            .into_any_element()
    }
}

#[cfg(feature = "headless-bench")]
impl ProfilesView {
    /// Which editor control holds keyboard focus, for headless checks.
    pub fn headless_editor_focus(
        &self,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> Option<&'static str> {
        let inputs = [
            ("id", &self.editor_inputs.id),
            ("context", &self.editor_inputs.context),
            ("output", &self.editor_inputs.output),
            ("query", &self.editor_inputs.query),
            ("level", &self.editor_inputs.level),
            ("budget", &self.editor_inputs.budget),
        ];
        for (name, input) in inputs {
            if input.read(cx).focus_handle().is_focused(window) {
                return Some(name);
            }
        }
        if let Some(editor) = &self.editor {
            for (name, focus) in [("thinking", &editor.kind_focus), ("api", &editor.api_focus)] {
                if focus.as_ref().is_some_and(|f| f.is_focused(window)) {
                    return Some(name);
                }
            }
        }
        window.focused(cx).map(|_| "other")
    }
    /// The text currently in an editor field.
    pub fn headless_editor_text(&self, field: &str, cx: &gpui::App) -> String {
        let input = match field {
            "id" => &self.editor_inputs.id,
            "context" => &self.editor_inputs.context,
            "output" => &self.editor_inputs.output,
            "query" => &self.editor_inputs.query,
            "level" => &self.editor_inputs.level,
            _ => &self.editor_inputs.budget,
        };
        input.read(cx).value().to_owned()
    }
}
