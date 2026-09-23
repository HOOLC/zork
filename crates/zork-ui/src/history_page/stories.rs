use super::*;
use crate::history_details::{Details, Presentation};
use std::rc::Rc;

#[derive(Clone)]
pub struct StoryText(pub Text);
impl Global for StoryText {}
pub struct Story {
    state: State,
    text: Text,
    paging: Paging,
    details: Entity<Details>,
    presentation: Option<Presentation>,
}
impl EventEmitter<HistoryChanged> for Story {}
impl Story {
    pub fn new(_: String, state: &str, cx: &mut Context<Self>) -> Self {
        let text = cx
            .try_global::<StoryText>()
            .map(|v| v.0.clone())
            .unwrap_or_else(|| Text(Rc::new(str::to_owned)));
        Self::with_text(state, text, cx)
    }
    pub fn with_text(state: &str, text: Text, cx: &mut Context<Self>) -> Self {
        let fixture = crate::stories::page_fixture();
        let records: Vec<model::Record> =
            serde_json::from_value(fixture["history"]["records"].clone()).unwrap();
        let entries: zork_observe::List<_> = if state == "empty" {
            vec![]
        } else {
            model::entries(&records)
        }
        .into();
        let projection = Projection::new(entries.iter());
        let mut expanded = HashSet::new();
        let mut output_expanded = HashSet::new();
        if state == "expanded" {
            if let Some(block) = projection.blocks.iter().find(|b| b.is_group()) {
                expanded.insert(entries[projection.activities[block.start].entry].id.clone());
            }
            // The expanded story reveals the complete shared Markdown body.
            if let Some(activity) = projection
                .activities
                .iter()
                .find(|a| a.kind == Kind::Output)
            {
                output_expanded.insert(entries[activity.entry].id.clone());
            }
        }
        let rows = projection.rows(entries.iter(), &expanded);
        let scroll = ListState::new(rows.len() + 1, ListAlignment::Top, px(200.))
            .with_uniform_item_height(px(26.));
        let details = cx.new(|cx| Details::new(text.clone(), cx));
        cx.subscribe(&details, |v, _, _: &crate::history_details::Closed, cx| {
            v.presentation = None;
            cx.notify();
        })
        .detach();
        Self {
            state: State {
                entries,
                projection,
                rows,
                expanded,
                output_expanded,
                scroll,
                fixed_now: fixture["history"]["now"].as_i64(),
                ..Default::default()
            }
            .with_metrics(),
            text,
            details,
            presentation: None,
            paging: Paging {
                loaded: state != "loading",
                busy: state == "loading",
                error: state == "error",
                ..Default::default()
            },
        }
    }
}
impl Host for Story {
    fn history(&self) -> &State {
        &self.state
    }
    fn history_mut(&mut self) -> &mut State {
        &mut self.state
    }
    fn history_text(&self) -> Text {
        self.text.clone()
    }
    fn history_subject(&self, a: &Activity, _: &Entry) -> (Option<String>, Option<Jump>) {
        use crate::history::activity::Subject;
        match &a.subject {
            Some(Subject::Agent(id)) => (Some("产品领队".into()), Some(Jump::Agent(id.clone()))),
            Some(Subject::Conversation) => {
                (Some("聊天".into()), Some(Jump::Conversation("demo".into())))
            }
            Some(Subject::Invocation(id)) => {
                (Some(id.clone()), Some(Jump::Entry(format!("tool:{id}"))))
            }
            Some(Subject::User) => (Some(self.text.text("history_user")), None),
            _ if a.kind == Kind::Input => (Some(self.text.text("history_source_unknown")), None),
            _ => (None, None),
        }
    }
    fn history_action(&mut self, action: Action, cx: &mut Context<Self>) {
        match action {
            Action::Scroll => {}
            Action::LoadOlder => {
                self.paging.error = false;
                self.paging.busy = false;
                self.paging.loaded = true;
            }
            Action::OpenEntry(id) | Action::Jump(Jump::File(id)) => {
                self.presentation = self
                    .state
                    .entries
                    .iter()
                    .find(|e| e.id == id)
                    .cloned()
                    .map(Presentation::Entry);
            }
            Action::Jump(Jump::Entry(id)) => {
                let index = self.state.entries.iter().position(|e| e.id == id);
                if let Some(index) = index {
                    self.history_select(index, cx);
                }
            }
            Action::Jump(Jump::Agent(id)) => {
                self.presentation = Some(Presentation::Agent {
                    id,
                    name: "产品领队".into(),
                    avatar: Some("fox".into()),
                    role: Some("整理产品需求与研究资料".into()),
                })
            }
            Action::Jump(Jump::Conversation(_)) => self.presentation = None,
        }
        crate::components::region::invalidate_all(cx);
    }
    fn history_paging(&self) -> Paging {
        self.paging.clone()
    }
    fn history_statistics(&mut self) -> Statistics {
        Statistics {
            usage: self.state.loaded_usage.clone(),
            calls: self.state.model_calls,
            models: self.state.models.clone(),
            runtime: Runtime {
                name: "产品领队".into(),
                avatar: Some("fox".into()),
                role: Some("领队".into()),
                environment: Some("Studio Mac".into()),
                provider: Some("Codex".into()),
                model: Some("gpt-5.4".into()),
                ..Default::default()
            },
        }
    }
}
impl Render for Story {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.details.update(cx, |v, cx| {
            v.configure(
                "demo".into(),
                self.presentation.clone(),
                None,
                self.text.clone(),
                cx,
            )
        });
        div()
            .size_full()
            .font_family("Inter Variable")
            .text_size(px(12.))
            .line_height(px(18.))
            .bg(rgb(ZORK_UI.palette.canvas))
            .child(self.render_history_page(window, cx))
            .child(self.details.clone())
    }
}
